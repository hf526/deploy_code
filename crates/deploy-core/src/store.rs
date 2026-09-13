use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use directories::ProjectDirs;

use crate::error::{CoreError, Result};
use crate::models::{
    now_string, new_id, AppConfig, BackupConfig, BackupRecord, DeployRecord, DeployStatus,
    PagesDeployRecord, RepoConfig, ServerConfig,
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
                additions.push(BackupConfig {
                    id: new_id(),
                    name,
                    server_id: server.id.clone(),
                    source,
                    target_id: server.backup_target_id.clone(),
                    supabase_url: server.supabase_url.clone(),
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
    let normalize = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
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
        // 同名服务器迁移时要生成不冲突的配置名。
        config.servers.push(server("prod", "h2"));
        store.save_config(&config).unwrap();

        assert_eq!(store.migrate_backup_configs().unwrap(), 2);
        let saved = store.load_config().unwrap();
        assert!(saved.backup_configs_migrated);
        assert_eq!(saved.backup_configs.len(), 2);
        assert_eq!(saved.backup_configs[0].name, "prod");
        assert_eq!(saved.backup_configs[1].name, "prod (2)");
        assert_eq!(saved.backup_configs[0].source.database, "app");
        assert_eq!(saved.backup_configs[0].target_id.as_deref(), Some("t1"));
        assert_eq!(
            saved.backup_configs[0].supabase_url.as_deref(),
            Some("postgresql://legacy@h/db")
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
