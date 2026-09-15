use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use directories::ProjectDirs;

use crate::error::{CoreError, Result};
use crate::models::{
    now_string, new_id, AppConfig, BackupConfig, BackupRecord, DeployConfig, DeployRecord,
    DeployStatus, PagesDeployRecord, RepoConfig, ServerConfig,
};

/// 配置与部署记录的本地存储（JSON 文件）。
///
/// 目录结构：
/// ```text
/// <data_dir>/
///   config.json     # 服务器 / 仓库 / 设置
///   history.json    # 部署记录
///   temp/           # 临时打包文件
/// ```
pub struct Store {
    base_dir: PathBuf,
    /// 串行化写入，避免并发部署/保存设置时互相覆盖或写出损坏文件。
    write_lock: Mutex<()>,
}

/// 跨进程任务锁（GUI 与 CLI 抢占同一个任务时互斥）。
pub struct TaskLock {
    file: std::fs::File,
    path: PathBuf,
}

impl TaskLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TaskLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 同时持有进程内互斥锁与跨进程文件锁的写守卫，析构时一并释放。
struct WriteGuard<'a> {
    _mutex: MutexGuard<'a, ()>,
    _file: std::fs::File,
}

impl Store {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            write_lock: Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 写操作守卫：先拿进程内互斥锁，再阻塞获取跨进程文件锁。
    fn write_guard(&self) -> Result<WriteGuard<'_>> {
        let mutex = self.lock();
        let path = lock_file_path(&self.base_dir, "store")?;
        let file = open_lock_file(&path)?;
        file.lock().map_err(|e| CoreError::io_path(&path, e))?;
        Ok(WriteGuard {
            _mutex: mutex,
            _file: file,
        })
    }

    /// 尝试获取命名任务锁；已被其他进程持有时返回 `Ok(None)`。
    pub fn try_task_lock(&self, name: &str) -> Result<Option<TaskLock>> {
        let path = lock_file_path(&self.base_dir, name)?;
        let file = open_lock_file(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(TaskLock { file, path })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(err)) => Err(CoreError::io_path(&path, err)),
        }
    }

    /// 使用系统默认数据目录（GUI 与 CLI 共用同一份数据）。
    pub fn default_store() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "deploycode", "DeployCode")
            .ok_or_else(|| CoreError::config("无法定位系统数据目录"))?;
        Ok(Self::new(dirs.data_dir()))
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn config_path(&self) -> PathBuf {
        self.base_dir.join("config.json")
    }

    pub fn history_path(&self) -> PathBuf {
        self.base_dir.join("history.json")
    }

    pub fn backups_path(&self) -> PathBuf {
        self.base_dir.join("backups.json")
    }

    pub fn pages_path(&self) -> PathBuf {
        self.base_dir.join("pages.json")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.base_dir.join("temp")
    }

    /// 确保数据目录存在，并返回临时文件路径。
    pub fn prepare_temp_file(&self, name: &str) -> Result<PathBuf> {
        let dir = self.temp_dir();
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join(name))
    }

    pub fn load_config(&self) -> Result<AppConfig> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(AppConfig::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|e| CoreError::io_path(&path, e))?;
        if text.trim().is_empty() {
            return Ok(AppConfig::default());
        }
        serde_json::from_str(&text).map_err(|e| {
            CoreError::config(format!("配置文件解析失败 {}: {e}", path.display()))
        })
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        let _guard = self.write_guard()?;
        write_config(&self.config_path(), config)
    }

    /// 加锁执行「读取-修改-写回」，避免并发命令互相覆盖配置。
    pub fn mutate_config<T, F>(&self, mutate: F) -> Result<T>
    where
        F: FnOnce(&mut AppConfig) -> Result<T>,
    {
        let _guard = self.write_guard()?;
        let mut config = self.load_config()?;
        let output = mutate(&mut config)?;
        write_config(&self.config_path(), &config)?;
        Ok(output)
    }

    pub fn load_history(&self) -> Result<Vec<DeployRecord>> {
        load_history(&self.history_path())
    }

    pub fn save_history(&self, records: &[DeployRecord]) -> Result<()> {
        let _guard = self.write_guard()?;
        write_history(&self.history_path(), records)
    }

    /// 新增或更新一条部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_history(&self, record: &DeployRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        let mut records = load_history(&self.history_path())?;
        match records.iter_mut().find(|r| r.id == record.id) {
            Some(existing) => *existing = record.clone(),
            None => records.push(record.clone()),
        }
        if limit > 0 && records.len() > limit {
            let overflow = records.len() - limit;
            records.drain(0..overflow);
        }
        write_history(&self.history_path(), &records)
    }

    pub fn remove_history(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        let mut records = load_history(&self.history_path())?;
        let before = records.len();
        records.retain(|r| r.id != id);
        let removed = records.len() != before;
        if removed {
            write_history(&self.history_path(), &records)?;
        }
        Ok(removed)
    }

    pub fn clear_history(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_history(&self.history_path(), &[])
    }

    pub fn find_record(&self, id: &str) -> Result<DeployRecord> {
        self.load_history()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::not_found(format!("部署记录不存在: {id}")))
    }

    pub fn load_backups(&self) -> Result<Vec<BackupRecord>> {
        load_backups(&self.backups_path())
    }

    pub fn save_backups(&self, records: &[BackupRecord]) -> Result<()> {
        let _guard = self.write_guard()?;
        write_backups(&self.backups_path(), records)
    }

    /// 新增或更新一条备份记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_backup(&self, record: &BackupRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        let mut records = load_backups(&self.backups_path())?;
        match records.iter_mut().find(|r| r.id == record.id) {
            Some(existing) => *existing = record.clone(),
            None => records.push(record.clone()),
        }
        if limit > 0 && records.len() > limit {
            let overflow = records.len() - limit;
            records.drain(0..overflow);
        }
        write_backups(&self.backups_path(), &records)
    }

    pub fn find_backup(&self, id: &str) -> Result<BackupRecord> {
        let records = self.load_backups()?;
        if let Some(record) = records.iter().find(|r| r.id == id) {
            return Ok(record.clone());
        }
        let matches: Vec<_> = records.iter().filter(|r| r.id.starts_with(id)).collect();
        match matches.len() {
            0 => Err(CoreError::not_found(format!("备份记录不存在: {id}"))),
            1 => Ok(matches[0].clone()),
            _ => Err(CoreError::config("记录 ID 不唯一，请输入完整 ID")),
        }
    }

    pub fn remove_backup(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        let mut records = load_backups(&self.backups_path())?;
        let before = records.len();
        records.retain(|r| r.id != id);
        let removed = records.len() != before;
        if removed {
            write_backups(&self.backups_path(), &records)?;
        }
        Ok(removed)
    }

    pub fn clear_backups(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_backups(&self.backups_path(), &[])
    }

    pub fn load_pages_records(&self) -> Result<Vec<PagesDeployRecord>> {
        load_pages_records(&self.pages_path())
    }

    pub fn save_pages_records(&self, records: &[PagesDeployRecord]) -> Result<()> {
        let _guard = self.write_guard()?;
        write_pages_records(&self.pages_path(), records)
    }

    /// 新增或更新一条 Pages 部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_pages_record(&self, record: &PagesDeployRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        let mut records = load_pages_records(&self.pages_path())?;
        match records.iter_mut().find(|r| r.id == record.id) {
            Some(existing) => *existing = record.clone(),
            None => records.push(record.clone()),
        }
        if limit > 0 && records.len() > limit {
            let overflow = records.len() - limit;
            records.drain(0..overflow);
        }
        write_pages_records(&self.pages_path(), &records)
    }

    pub fn find_pages_record(&self, id: &str) -> Result<PagesDeployRecord> {
        let records = self.load_pages_records()?;
        if let Some(record) = records.iter().find(|r| r.id == id) {
            return Ok(record.clone());
        }
        let matches: Vec<_> = records.iter().filter(|r| r.id.starts_with(id)).collect();
        match matches.len() {
            0 => Err(CoreError::not_found(format!("Pages 部署记录不存在: {id}"))),
            1 => Ok(matches[0].clone()),
            _ => Err(CoreError::config("记录 ID 不唯一，请输入完整 ID")),
        }
    }

    pub fn remove_pages_record(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        let mut records = load_pages_records(&self.pages_path())?;
        let before = records.len();
        records.retain(|r| r.id != id);
        let removed = records.len() != before;
        if removed {
            write_pages_records(&self.pages_path(), &records)?;
        }
        Ok(removed)
    }

    pub fn clear_pages_records(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_pages_records(&self.pages_path(), &[])
    }

    /// 启动时把上次异常退出（崩溃/强杀）遗留的 Running 记录收敛为失败，
    /// 避免历史里永远显示"进行中"且无法重新部署。
    /// 全程持有写守卫，防止与并发写（如 CLI 完成任务）互相覆盖。
    pub fn mark_interrupted(&self) -> Result<usize> {
        let _guard = self.write_guard()?;
        let message = "任务被中断（应用退出或崩溃）";
        let mut converted = 0usize;

        let mut history = load_history(&self.history_path())?;
        let mut touched = false;
        for record in history.iter_mut() {
            if record.status == DeployStatus::Running {
                record.status = DeployStatus::Failed;
                record.error = Some(message.to_string());
                record.finished_at = Some(now_string());
                // 崩溃后无法得知真实结束时间，duration_ms 保持 0（界面显示为未知），
                // 不能用「下次启动时刻」计算，否则会把停机时间也算进耗时。
                converted += 1;
                touched = true;
            }
        }
        if touched {
            write_history(&self.history_path(), &history)?;
        }

        let mut backups = load_backups(&self.backups_path())?;
        let mut touched = false;
        for record in backups.iter_mut() {
            if record.status == DeployStatus::Running {
                record.status = DeployStatus::Failed;
                record.error = Some(message.to_string());
                record.finished_at = Some(now_string());
                converted += 1;
                touched = true;
            }
        }
        if touched {
            write_backups(&self.backups_path(), &backups)?;
        }

        let mut pages = load_pages_records(&self.pages_path())?;
        let mut touched = false;
        for record in pages.iter_mut() {
            if record.status == DeployStatus::Running {
                record.status = DeployStatus::Failed;
                record.error = Some(message.to_string());
                record.finished_at = Some(now_string());
                converted += 1;
                touched = true;
            }
        }
        if touched {
            write_pages_records(&self.pages_path(), &pages)?;
        }

        Ok(converted)
    }

    /// 仅当没有其他进程正在执行部署 / 备份 / Pages 任务时，才收敛遗留的 Running 记录。
    /// GUI 启动和 CLI 启动共用，避免误伤正在运行的任务。
    pub fn reconcile_interrupted(&self) -> Result<usize> {
        let deploy = self.try_task_lock("deploy")?;
        let backup = self.try_task_lock("backup")?;
        let pages = self.try_task_lock("pages")?;
        if deploy.is_none() || backup.is_none() || pages.is_none() {
            return Ok(0);
        }
        self.mark_interrupted()
    }

    /// 按 id / 名称 / 路径查找仓库。
    pub fn find_repo<'a>(config: &'a AppConfig, key: &str) -> Result<&'a RepoConfig> {
        config
            .repos
            .iter()
            .find(|r| r.id == key || r.name == key || paths_equal(&r.path, key))
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {key}")))
    }

    /// 按 id / 名称查找备份配置。
    pub fn find_backup_config<'a>(config: &'a AppConfig, key: &str) -> Result<&'a BackupConfig> {
        config
            .backup_configs
            .iter()
            .find(|item| item.id == key || item.name == key)
            .ok_or_else(|| CoreError::not_found(format!("备份配置不存在: {key}")))
    }

    /// 按 id / 名称查找部署配置；id 精确命中优先，避免与名称歧义。
    pub fn find_deploy_config<'a>(config: &'a AppConfig, key: &str) -> Result<&'a DeployConfig> {
        let key = key.trim();
        if let Some(item) = config.deploy_configs.iter().find(|item| item.id == key) {
            return Ok(item);
        }
        config
            .deploy_configs
            .iter()
            .find(|item| item.name == key)
            .ok_or_else(|| CoreError::not_found(format!("部署配置不存在: {key}")))
    }

    /// 新建或更新一条部署配置：规范化仓库 / 服务器引用并校验名称唯一。
    /// GUI 与 CLI 共用，保证两端保存行为一致。
    pub fn save_deploy_config(store: &Store, config: DeployConfig) -> Result<DeployConfig> {
        let mut config = config;
        config.name = config.name.trim().to_string();
        if config.name.is_empty() {
            return Err(CoreError::config("部署配置名称不能为空"));
        }
        config.target_dir = config.target_dir.trim().to_string();
        if config.target_dir.is_empty() {
            return Err(CoreError::config("部署目录不能为空"));
        }
        // 部署目标是 Linux 服务器上的绝对路径：相对路径 / ~ 会落到 SSH 登录目录，
        // 且与原子发布的 release 目录约定（要求绝对路径）冲突。
        if !config.target_dir.starts_with('/') {
            return Err(CoreError::config(
                "部署目录必须是服务器上的绝对路径（以 / 开头）",
            ));
        }
        config.rev = config.rev.trim().to_string();
        config.id = config.id.trim().to_string();
        config.repo_id = config.repo_id.trim().to_string();
        config.server_id = config.server_id.trim().to_string();
        config.script_dir = config.script_dir.trim().to_string();
        if config.script_dir.is_empty() {
            config.script_dir = "docker".to_string();
        }
        config.scripts = config
            .scripts
            .iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();

        store.mutate_config(|app| {
            // 仓库 / 服务器必须存在；名称与 host 也允许（兼容 CLI 习惯）。
            let repo = Store::find_repo(app, &config.repo_id)?.clone();
            config.repo_id = repo.id.clone();
            let server = Store::find_server(app, &config.server_id)?.clone();
            config.server_id = server.id.clone();

            let by_id = app.deploy_configs.iter().find(|item| item.id == config.id);
            if config.id.is_empty() {
                config.id = new_id();
            } else if by_id.is_none() {
                // 明确携带 id 却不存在（配置已被其它窗口 / CLI 删除）：报错而不是静默新建，
                // 避免「复活」已删除配置或用任意 id 注入条目。
                return Err(CoreError::not_found(format!(
                    "部署配置不存在（可能已被删除）: {}",
                    config.id
                )));
            }
            // 更新已有配置时以存储中的创建时间为准，避免调用方传入的旧快照覆盖。
            match by_id {
                Some(existing) if !existing.created_at.trim().is_empty() => {
                    config.created_at = existing.created_at.clone();
                }
                _ => {
                    if config.created_at.trim().is_empty() {
                        config.created_at = now_string();
                    }
                }
            }
            if app
                .deploy_configs
                .iter()
                .any(|item| item.id != config.id && item.name == config.name)
            {
                return Err(CoreError::config(format!(
                    "部署配置名称已存在: {}",
                    config.name
                )));
            }
            match app.deploy_configs.iter_mut().find(|item| item.id == config.id) {
                Some(existing) => *existing = config.clone(),
                None => app.deploy_configs.push(config.clone()),
            }
            Ok(config.clone())
        })
    }

    /// 删除一条部署配置（不影响已有部署记录，历史记录仍可重新部署）。
    /// 优先按 id 精确删除，避免出现「某配置名称恰好等于另一条配置 id」时误删两条。
    pub fn delete_deploy_config(store: &Store, key: &str) -> Result<bool> {
        let key = key.trim();
        store.mutate_config(|app| {
            let removed: Vec<String> = if let Some(item) =
                app.deploy_configs.iter().find(|item| item.id == key)
            {
                vec![item.id.clone()]
            } else {
                app.deploy_configs
                    .iter()
                    .filter(|item| item.name == key)
                    .map(|item| item.id.clone())
                    .collect()
            };
            if removed.is_empty() {
                return Ok(false);
            }
            app.deploy_configs.retain(|item| !removed.contains(&item.id));
            Ok(true)
        })
    }

    /// 按 id / 名称 / host 查找服务器。
    pub fn find_server<'a>(config: &'a AppConfig, key: &str) -> Result<&'a ServerConfig> {
        config
            .servers
            .iter()
            .find(|s| s.id == key || s.name == key || s.host == key)
            .ok_or_else(|| CoreError::not_found(format!("服务器不存在: {key}")))
    }

    pub fn upsert_server(config: &mut AppConfig, server: ServerConfig) -> Result<()> {
        match config.servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server,
            None => config.servers.push(server),
        }
        Ok(())
    }

    /// 把旧版「每台服务器一份 db_backup」迁移为全局备份配置（只执行一次）。
    /// 返回本次新建的配置数量。
    pub fn migrate_backup_configs(&self) -> Result<usize> {
        if self.load_config()?.backup_configs_migrated {
            return Ok(0);
        }
        self.mutate_config(|config| {
            if config.backup_configs_migrated {
                return Ok(0);
            }
            let mut additions: Vec<BackupConfig> = Vec::new();
            for server in &config.servers {
                let Some(source) = server.db_backup.clone() else {
                    continue;
                };
                if config
                    .backup_configs
                    .iter()
                    .chain(additions.iter())
                    .any(|item| item.server_id == server.id)
                {
                    continue;
                }
                let mut name = server.name.trim().to_string();
                if name.is_empty() {
                    name = format!("{}@{}", server.username, server.host);
                }
                if config
                    .backup_configs
                    .iter()
                    .chain(additions.iter())
                    .any(|item| item.name == name)
                {
                    let base = name.clone();
                    let mut index = 2;
                    loop {
                        let candidate = format!("{base} ({index})");
                        if !config
                            .backup_configs
                            .iter()
                            .chain(additions.iter())
                            .any(|item| item.name == candidate)
                        {
                            name = candidate;
                            break;
                        }
                        index += 1;
                    }
                }
                // 与服务器字段的优先级保持一致（绑定目标 > 自定义连接串）：两者都配置时只迁移赢家。
                // 否则 BackupConfig 中连接串优先于目标，会连到旧连接串指向的库。
                let target_id = server
                    .backup_target_id
                    .clone()
                    .filter(|value| !value.trim().is_empty());
                let supabase_url = if target_id.is_some() {
                    None
                } else {
                    server.supabase_url.clone()
                };
                additions.push(BackupConfig {
                    id: new_id(),
                    name,
                    server_id: server.id.clone(),
                    source,
                    target_id,
                    supabase_url,
                });
            }
            let created = additions.len();
            config.backup_configs.extend(additions);
            config.backup_configs_migrated = true;
            Ok(created)
        })
    }
}

fn paths_equal(a: &str, b: &str) -> bool {
    let normalize = |p: &str| {
        let normalized = p.replace('\\', "/");
        // 大小写不敏感只适用于 Windows；Linux 上 /srv/App 与 /srv/app 是两个不同仓库。
        #[cfg(windows)]
        let normalized = normalized.to_lowercase();
        normalized.trim_end_matches('/').to_string()
    };
    normalize(a) == normalize(b)
}

fn load_history(path: &Path) -> Result<Vec<DeployRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| CoreError::io_path(path, e))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text)
        .map_err(|e| CoreError::config(format!("部署记录解析失败 {}: {e}", path.display())))
}

fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
    let text = serde_json::to_string_pretty(config)?;
    atomic_write(path, &text)
}

fn write_history(path: &Path, records: &[DeployRecord]) -> Result<()> {
    let text = serde_json::to_string_pretty(records)?;
    atomic_write(path, &text)
}

fn load_backups(path: &Path) -> Result<Vec<BackupRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| CoreError::io_path(path, e))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text)
        .map_err(|e| CoreError::config(format!("备份记录解析失败 {}: {e}", path.display())))
}

fn write_backups(path: &Path, records: &[BackupRecord]) -> Result<()> {
    let text = serde_json::to_string_pretty(records)?;
    atomic_write(path, &text)
}

fn load_pages_records(path: &Path) -> Result<Vec<PagesDeployRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| CoreError::io_path(path, e))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text)
        .map_err(|e| CoreError::config(format!("Pages 记录解析失败 {}: {e}", path.display())))
}

fn write_pages_records(path: &Path, records: &[PagesDeployRecord]) -> Result<()> {
    let text = serde_json::to_string_pretty(records)?;
    atomic_write(path, &text)
}

/// 锁文件路径（`<base_dir>/locks/<name>.lock`），并确保目录存在。
fn lock_file_path(base_dir: &Path, name: &str) -> Result<PathBuf> {
    let dir = base_dir.join("locks");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{name}.lock")))
}

fn open_lock_file(path: &Path) -> Result<std::fs::File> {
    // 锁文件只创建不截断（截断会干扰其他进程对同一文件的锁定语义）。
    #[allow(clippy::suspicious_open_options)]
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .map_err(|e| CoreError::io_path(path, e))?;
    Ok(file)
}

/// 原子写：写入同目录下的唯一临时文件后 rename，避免并发写共享临时名导致内容撕裂。
fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data.json");
    let tmp = path.with_file_name(format!("{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let write_result = (|| -> Result<()> {
        let mut file = std::fs::File::create(&tmp).map_err(|e| CoreError::io_path(&tmp, e))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| CoreError::io_path(&tmp, e))?;
        file.sync_all().map_err(|e| CoreError::io_path(&tmp, e))?;
        // 配置文件含密码 / Token，权限收紧为仅属主可读写。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| CoreError::io_path(&tmp, e))?;
        }
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }

    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::io_path(path, err));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_interrupted_skips_when_task_lock_held() {
        let dir = std::env::temp_dir()
            .join(format!("deploycode-store-reconcile-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let record: DeployRecord = serde_json::from_str(
            r#"{
                "id":"r1","repoId":"repo","repoName":"demo","rev":"main","branch":"main",
                "commit":"abc","commitShort":"abc","commitSubject":"init","serverId":"srv",
                "serverName":"prod","targetDir":"/srv/app","scriptDir":"docker","scripts":[],
                "runScripts":false,"envFiles":[],"status":"running","error":null,"log":"",
                "startedAt":"2026-01-01 00:00:00","finishedAt":null,"durationMs":0
            }"#,
        )
        .unwrap();
        store.upsert_history(&record, 0).unwrap();

        // 其他进程 / 任务仍在运行（持有任务锁）时不得收敛。
        let held = store.try_task_lock("deploy").unwrap().unwrap();
        assert_eq!(store.reconcile_interrupted().unwrap(), 0);
        assert_eq!(
            store.load_history().unwrap()[0].status,
            DeployStatus::Running
        );
        drop(held);

        // 三种任务锁都空闲时才把遗留的 Running 记录收敛为失败。
        assert_eq!(store.reconcile_interrupted().unwrap(), 1);
        assert_eq!(
            store.load_history().unwrap()[0].status,
            DeployStatus::Failed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_task_lock_excludes_concurrent_holders() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-store-lock-{}", uuid::Uuid::new_v4()));
        let first_store = Store::new(&dir);
        let second_store = Store::new(&dir);

        let held = first_store.try_task_lock("backup").unwrap();
        assert!(held.is_some());
        // 同一把锁的第二个持有者拿不到；不同名字的锁互不影响。
        assert!(second_store.try_task_lock("backup").unwrap().is_none());
        assert!(second_store.try_task_lock("pages").unwrap().is_some());
        // 释放后可再次获取。
        drop(held);
        assert!(second_store.try_task_lock("backup").unwrap().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_config_creates_locks_dir_and_reloads() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-store-save-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        config.settings.supabase_url = "postgresql://u@h/db".to_string();
        store.save_config(&config).unwrap();
        assert_eq!(
            store.load_config().unwrap().settings.supabase_url,
            "postgresql://u@h/db"
        );
        assert!(dir.join("locks").join("store.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_and_delete_deploy_config_normalizes_and_validates() {
        use crate::models::SshAuth;

        let dir =
            std::env::temp_dir().join(format!("deploycode-store-deploycfg-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        let repo = RepoConfig::new("demo".to_string(), "/tmp/demo".to_string());
        let repo_id = repo.id.clone();
        config.repos.push(repo);
        let server = ServerConfig::new(
            "prod".to_string(),
            "h1".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let server_id = server.id.clone();
        config.servers.push(server);
        store.save_config(&config).unwrap();

        // 仓库 / 服务器可以按名称传入，保存时规范化为 id；空字段回落到默认值。
        let saved = Store::save_deploy_config(
            &store,
            DeployConfig {
                id: String::new(),
                name: " 生产部署 ".to_string(),
                repo_id: "demo".to_string(),
                server_id: "prod".to_string(),
                target_dir: " /opt/app ".to_string(),
                rev: " main ".to_string(),
                run_scripts: true,
                script_dir: "  ".to_string(),
                scripts: vec![" deploy.sh ".to_string(), "".to_string()],
                upload_env: true,
                created_at: String::new(),
            },
        )
        .unwrap();
        assert_eq!(saved.repo_id, repo_id);
        assert_eq!(saved.server_id, server_id);
        assert_eq!(saved.name, "生产部署");
        assert_eq!(saved.target_dir, "/opt/app");
        assert_eq!(saved.rev, "main");
        assert_eq!(saved.script_dir, "docker");
        assert_eq!(saved.scripts, vec!["deploy.sh".to_string()]);
        assert!(!saved.created_at.is_empty());

        // 不带 id 的新建配置重名时拒绝（GUI 的新增入口靠这条避免误覆盖已有配置）。
        let mut duplicate = saved.clone();
        duplicate.id = String::new();
        assert!(Store::save_deploy_config(&store, duplicate).is_err());

        // 引用不存在的服务器拒绝。
        let mut missing = saved.clone();
        missing.server_id = "nope".to_string();
        assert!(Store::save_deploy_config(&store, missing).is_err());

        // 更新同一条配置不会重复插入，且保留原创建时间。
        let created_at = saved.created_at.clone();
        let mut renamed = saved.clone();
        renamed.name = "生产部署 2".to_string();
        renamed.created_at = String::new();
        let renamed = Store::save_deploy_config(&store, renamed).unwrap();
        assert_eq!(renamed.created_at, created_at);
        assert_eq!(store.load_config().unwrap().deploy_configs.len(), 1);

        // id 前后空白会被规范掉。
        let mut padded = saved.clone();
        padded.id = format!("  {}  ", saved.id);
        padded.target_dir = "/opt/app3".to_string();
        let padded = Store::save_deploy_config(&store, padded).unwrap();
        assert_eq!(padded.id, saved.id);
        assert_eq!(padded.target_dir, "/opt/app3");

        // 明确携带不存在的 id 拒绝（避免配置被删除后又被静默复活）。
        let mut unknown = saved.clone();
        unknown.id = "no-such-id".to_string();
        assert!(Store::save_deploy_config(&store, unknown).is_err());

        // 相对部署目录拒绝：会落到 SSH 登录目录，且与原子发布的绝对路径约定冲突。
        let mut relative = saved.clone();
        relative.id = String::new();
        relative.name = "相对目录".to_string();
        relative.target_dir = "opt/app".to_string();
        assert!(Store::save_deploy_config(&store, relative).is_err());

        assert!(Store::delete_deploy_config(&store, &saved.id).unwrap());
        assert!(store.load_config().unwrap().deploy_configs.is_empty());
        assert!(!Store::delete_deploy_config(&store, &saved.id).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_backup_configs_copies_server_sources_once() {
        use crate::models::{DbBackupSource, SshAuth};

        fn server(name: &str, host: &str) -> ServerConfig {
            let mut server = ServerConfig::new(
                name.to_string(),
                host.to_string(),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            );
            server.db_backup = Some(DbBackupSource {
                mode: "docker".to_string(),
                container: "postgres".to_string(),
                database: "app".to_string(),
                username: "postgres".to_string(),
                password: "p".to_string(),
                schema: "public".to_string(),
            });
            server
        }

        let dir =
            std::env::temp_dir().join(format!("deploycode-store-migrate-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        let mut first = server("prod", "h1");
        first.backup_target_id = Some("t1".to_string());
        first.supabase_url = Some("postgresql://legacy@h/db".to_string());
        config.servers.push(first);
        // 同名服务器迁移时要生成不冲突的配置名；没有绑定目标时旧连接串照常迁移。
        let mut second = server("prod", "h2");
        second.supabase_url = Some("postgresql://legacy@h2/db".to_string());
        config.servers.push(second);
        store.save_config(&config).unwrap();

        assert_eq!(store.migrate_backup_configs().unwrap(), 2);
        let saved = store.load_config().unwrap();
        assert!(saved.backup_configs_migrated);
        assert_eq!(saved.backup_configs.len(), 2);
        assert_eq!(saved.backup_configs[0].name, "prod");
        assert_eq!(saved.backup_configs[1].name, "prod (2)");
        assert_eq!(saved.backup_configs[0].source.database, "app");
        // 服务器同时配置了绑定目标与旧连接串时，以绑定目标为准（与服务器字段优先级一致），
        // 旧的连接串不能一起迁移，否则会因配置中 URL 优先而备份到错误的库。
        assert_eq!(saved.backup_configs[0].target_id.as_deref(), Some("t1"));
        assert_eq!(saved.backup_configs[0].supabase_url, None);
        assert_eq!(saved.backup_configs[1].target_id, None);
        assert_eq!(
            saved.backup_configs[1].supabase_url.as_deref(),
            Some("postgresql://legacy@h2/db")
        );

        // 只迁移一次：即使配置被删除也不会在下次启动时重建。
        let removed_id = saved.backup_configs[0].id.clone();
        let mut pruned = saved;
        pruned.backup_configs.retain(|item| item.id != removed_id);
        store.save_config(&pruned).unwrap();
        assert_eq!(store.migrate_backup_configs().unwrap(), 0);
        assert_eq!(store.load_config().unwrap().backup_configs.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            std::env::temp_dir().join(format!("deploycode-store-perm-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        store.save_config(&AppConfig::default()).unwrap();
        let mode = std::fs::metadata(store.config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
