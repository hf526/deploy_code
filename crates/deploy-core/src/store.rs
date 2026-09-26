use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use directories::ProjectDirs;

use crate::crypto::{decrypt_string, encrypt_string};
use crate::error::{CoreError, Result};
use crate::models::{
    now_string, new_id, AppConfig, BackupConfig, BackupRecord, ContainerConfig, ContainerRecord,
    DbBackupSource, DeployConfig, DeployRecord, DeployStatus, EnvFileConfig, ExportData,
    ImportCounts, ImportPreview, PagesConfigEntry, PagesDeployRecord, RepoConfig, ServerConfig,
    SshAuth,
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

    pub fn containers_path(&self) -> PathBuf {
        self.base_dir.join("containers.json")
    }

    /// 容器备份包的本机存放目录（`<数据目录>/containers`）。
    ///
    /// 免设置可跑：不需要用户先选目录，备份一律落在这里，界面只负责把它显示出来。
    pub fn container_bundle_dir(&self) -> PathBuf {
        self.base_dir.join("containers")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.base_dir.join("temp")
    }

    /// 设置主密码（首次使用时调用）
    pub fn set_master_password(&self, master_password: &str) -> Result<()> {
        let mut config = self.load_config()?;
        
        // 计算主密码哈希用于验证
        let hash = Self::sha256_hash(master_password);
        config.settings.master_password_hash = Some(hash);
        self.save_config(&config)?;
        Ok(())
    }

    /// 验证主密码是否正确
    pub fn verify_master_password(&self, master_password: &str) -> Result<bool> {
        let config = self.load_config()?;
        
        let stored_hash = config.settings.master_password_hash.as_ref()
            .ok_or_else(|| CoreError::config("未设置主密码"))?;
        
        let input_hash = Self::sha256_hash(master_password);
        Ok(input_hash == *stored_hash)
    }

    /// SHA256 哈希辅助函数
    fn sha256_hash(input: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let hash = hasher.finalize();
        format!("{:x}", hash)
    }

    /// 加密配置中的所有敏感字段
    fn encrypt_sensitive_fields(&self, config: &AppConfig, master_password: &str) -> Result<AppConfig> {
        if config.settings.master_password_hash.is_none() {
            return Err(CoreError::config("请先设置主密码"));
        }

        // 深度复制并加密
        let mut encrypted = config.clone();
        
        // 加密服务器密码
        for server in &mut encrypted.servers {
            if let SshAuth::Password { password } = &mut server.auth {
                let original_password = password.clone();
                *password = encrypt_string(&original_password, master_password)?;
            }
        }
        
        // 加密备份源密码
        for backup_config in &mut encrypted.backup_configs {
            let original_password = backup_config.source.password.clone();
            backup_config.source.password = encrypt_string(&original_password, master_password)?;
        }
        
        // 加密 Cloudflare API Token
        if !encrypted.settings.cloudflare_api_token.is_empty() {
            let original_token = encrypted.settings.cloudflare_api_token.clone();
            encrypted.settings.cloudflare_api_token = encrypt_string(&original_token, master_password)?;
        }
        
        // 加密 GitHub Token
        if !encrypted.settings.github_token.is_empty() {
            let original_token = encrypted.settings.github_token.clone();
            encrypted.settings.github_token = encrypt_string(&original_token, master_password)?;
        }

        // 加密 cron-job.org API Key
        if !encrypted.settings.cronjob_api_key.is_empty() {
            let original_key = encrypted.settings.cronjob_api_key.clone();
            encrypted.settings.cronjob_api_key = encrypt_string(&original_key, master_password)?;
        }
        
        Ok(encrypted)
    }

    /// 解密配置中的所有敏感字段
    fn decrypt_sensitive_fields(&self, config: &AppConfig, master_password: &str) -> Result<AppConfig> {
        let mut decrypted = config.clone();
        
        // 解密服务器密码
        for server in &mut decrypted.servers {
            if let SshAuth::Password { password } = &mut server.auth {
                // 检查是否是密文（base64 编码通常以字母开头，长度较长）
                if password.len() > 20 && password.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '=') {
                    match decrypt_string(password, master_password) {
                        Ok(plaintext) => *password = plaintext,
                        Err(_) => { /* 解密失败，保持原样 */ }
                    }
                }
            }
        }
        
        // 解密备份源密码
        for backup_config in &mut decrypted.backup_configs {
            if !backup_config.source.password.is_empty() {
                let encrypted_password = backup_config.source.password.clone();
                if encrypted_password.len() > 20 && encrypted_password.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '=') {
                    match decrypt_string(&encrypted_password, master_password) {
                        Ok(plaintext) => backup_config.source.password = plaintext,
                        Err(_) => { /* 解密失败，保持原样 */ }
                    }
                }
            }
        }
        
        // 解密 Cloudflare API Token
        if !decrypted.settings.cloudflare_api_token.is_empty() {
            let encrypted_token = decrypted.settings.cloudflare_api_token.clone();
            if encrypted_token.len() > 20 && encrypted_token.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '=') {
                match decrypt_string(&encrypted_token, master_password) {
                    Ok(plaintext) => decrypted.settings.cloudflare_api_token = plaintext,
                    Err(_) => { /* 解密失败，保持原样 */ }
                }
            }
        }
        
        // 解密 GitHub Token
        if !decrypted.settings.github_token.is_empty() {
            let encrypted_token = decrypted.settings.github_token.clone();
            if encrypted_token.len() > 20 && encrypted_token.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '=') {
                match decrypt_string(&encrypted_token, master_password) {
                    Ok(plaintext) => decrypted.settings.github_token = plaintext,
                    Err(_) => { /* 解密失败，保持原样 */ }
                }
            }
        }

        // 解密 cron-job.org API Key
        if !decrypted.settings.cronjob_api_key.is_empty() {
            let encrypted_key = decrypted.settings.cronjob_api_key.clone();
            if encrypted_key.len() > 20 && encrypted_key.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '=') {
                match decrypt_string(&encrypted_key, master_password) {
                    Ok(plaintext) => decrypted.settings.cronjob_api_key = plaintext,
                    Err(_) => { /* 解密失败，保持原样 */ }
                }
            }
        }

        Ok(decrypted)
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
        
        // 先加载配置
        let config: AppConfig = serde_json::from_str(&text).map_err(|e| {
            CoreError::config(format!("配置文件解析失败 {}: {e}", path.display()))
        })?;
        
        // 如果设置了主密码，尝试解密敏感字段
        if let Some(ref hash) = config.settings.master_password_hash {
            if !hash.is_empty() {
                // 这里我们假设用户已经设置了主密码，但实际验证需要在调用时进行
                // 为了向后兼容，我们先返回原始配置，让上层决定是否需要解密
                Ok(config)
            } else {
                Ok(config)
            }
        } else {
            // 旧版本配置（无主密码），直接返回
            Ok(config)
        }
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        // 注意：这里不自动加密，需要调用方显式调用 encrypt_sensitive_fields
        // 这样可以保持向后兼容性
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
        load_records(&self.history_path(), HISTORY_LABEL)
    }

    /// 新增或更新一条部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_history(&self, record: &DeployRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.history_path(), record, limit, HISTORY_LABEL)
    }

    pub fn remove_history(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<DeployRecord>(&self.history_path(), id, HISTORY_LABEL)
    }

    pub fn remove_history_many(&self, ids: &[String]) -> Result<usize> {
        let _guard = self.write_guard()?;
        remove_records_by_id::<DeployRecord>(&self.history_path(), ids, HISTORY_LABEL)
    }

    pub fn clear_history(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<DeployRecord>(&self.history_path(), &[])
    }

    pub fn find_record(&self, id: &str) -> Result<DeployRecord> {
        load_records::<DeployRecord>(&self.history_path(), HISTORY_LABEL)?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::not_found(format!("部署记录不存在: {id}")))
    }

    pub fn load_backups(&self) -> Result<Vec<BackupRecord>> {
        load_records(&self.backups_path(), BACKUP_LABEL)
    }

    /// 新增或更新一条备份记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_backup(&self, record: &BackupRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.backups_path(), record, limit, BACKUP_LABEL)
    }

    pub fn find_backup(&self, id: &str) -> Result<BackupRecord> {
        find_record_by_prefix(&self.backups_path(), id, BACKUP_LABEL, BACKUP_LABEL)
    }

    pub fn remove_backup(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<BackupRecord>(&self.backups_path(), id, BACKUP_LABEL)
    }

    pub fn remove_backups_many(&self, ids: &[String]) -> Result<usize> {
        let _guard = self.write_guard()?;
        remove_records_by_id::<BackupRecord>(&self.backups_path(), ids, BACKUP_LABEL)
    }

    pub fn clear_backups(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<BackupRecord>(&self.backups_path(), &[])
    }

    pub fn load_pages_records(&self) -> Result<Vec<PagesDeployRecord>> {
        load_records(&self.pages_path(), PAGES_LABEL)
    }

    /// 新增或更新一条 Pages 部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_pages_record(&self, record: &PagesDeployRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.pages_path(), record, limit, PAGES_LABEL)
    }

    pub fn find_pages_record(&self, id: &str) -> Result<PagesDeployRecord> {
        find_record_by_prefix(&self.pages_path(), id, PAGES_LABEL, PAGES_MISSING_LABEL)
    }

    pub fn remove_pages_record(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<PagesDeployRecord>(&self.pages_path(), id, PAGES_LABEL)
    }

    pub fn remove_pages_records_many(&self, ids: &[String]) -> Result<usize> {
        let _guard = self.write_guard()?;
        remove_records_by_id::<PagesDeployRecord>(&self.pages_path(), ids, PAGES_LABEL)
    }

    pub fn clear_pages_records(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<PagesDeployRecord>(&self.pages_path(), &[])
    }

    pub fn load_containers(&self) -> Result<Vec<ContainerRecord>> {
        load_records(&self.containers_path(), CONTAINER_LABEL)
    }

    /// 新增或更新一条容器任务记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_container(&self, record: &ContainerRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.containers_path(), record, limit, CONTAINER_LABEL)
    }

    pub fn find_container_record(&self, id: &str) -> Result<ContainerRecord> {
        find_record_by_prefix(&self.containers_path(), id, CONTAINER_LABEL, CONTAINER_LABEL)
    }

    pub fn remove_container_record(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<ContainerRecord>(&self.containers_path(), id, CONTAINER_LABEL)
    }

    /// 清空记录只删本地的任务历史，备份包文件留在原处（删记录不该带走数据）。
    pub fn clear_container_records(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<ContainerRecord>(&self.containers_path(), &[])
    }

    /// 导出配置（JSON 格式，敏感字段已脱敏）。
    pub fn export_config(&self) -> Result<String> {
        let config = self.load_config()?;
        let export_data = ExportData::new(&config);
        serde_json::to_string_pretty(&export_data).map_err(CoreError::Serde)
    }

    /// 导入前的只读比对：解析文件并演练一遍合并语义，不写盘。
    /// 导出文件必然已脱敏，所以调用方必须先把结果给用户确认，再调 `import_config`。
    pub fn preview_import(&self, json_str: &str) -> Result<ImportPreview> {
        let export_data = parse_export(json_str)?;
        let mut config = self.load_config()?;
        Ok(merge_export(&mut config, &export_data))
    }

    /// 导入配置（合并模式：同 ID 覆盖，新 ID 追加，被脱敏清空的凭据保留本机值）。
    pub fn import_config(&self, json_str: &str) -> Result<ImportPreview> {
        let export_data = parse_export(json_str)?;
        self.mutate_config(|config| Ok(merge_export(config, &export_data)))
    }

    /// 启动时把上次异常退出（崩溃/强杀）遗留的 Running 记录收敛为失败，
    /// 避免历史里永远显示"进行中"且无法重新部署。
    /// 全程持有写守卫，防止与并发写（如 CLI 完成任务）互相覆盖。
    pub fn mark_interrupted(&self) -> Result<usize> {
        let _guard = self.write_guard()?;
        let message = "任务被中断（应用退出或崩溃）";
        let mut converted = mark_running_as_interrupted::<DeployRecord>(
            &self.history_path(),
            HISTORY_LABEL,
            message,
        )?;
        converted += mark_running_as_interrupted::<BackupRecord>(
            &self.backups_path(),
            BACKUP_LABEL,
            message,
        )?;
        converted += mark_running_as_interrupted::<PagesDeployRecord>(
            &self.pages_path(),
            PAGES_LABEL,
            message,
        )?;
        converted += mark_running_as_interrupted::<ContainerRecord>(
            &self.containers_path(),
            CONTAINER_LABEL,
            message,
        )?;
        Ok(converted)
    }

    /// 仅当没有其他进程正在执行部署 / 备份 / Pages / 容器任务时，才收敛遗留的 Running 记录。
    /// GUI 启动和 CLI 启动共用，避免误伤正在运行的任务。
    pub fn reconcile_interrupted(&self) -> Result<usize> {
        let deploy = self.try_task_lock("deploy")?;
        let backup = self.try_task_lock("backup")?;
        let pages = self.try_task_lock("pages")?;
        let container = self.try_task_lock("container")?;
        if deploy.is_none() || backup.is_none() || pages.is_none() || container.is_none() {
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
    /// 服务器被删除后收敛部署配置：从目标列表里摘掉这一台，没有剩余目标的配置整条删除。
    /// GUI 与 CLI 共用，避免两条清理路径的语义分叉。
    pub fn detach_server_from_deploy_configs(config: &mut AppConfig, server_id: &str) {
        for saved in config.deploy_configs.iter_mut() {
            saved.server_ids.retain(|id| id != server_id);
        }
        config
            .deploy_configs
            .retain(|saved| !saved.server_ids.is_empty());
    }

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
        // 服务器列表：去空与去重，保留顺序（顺序即批量部署的执行顺序）。
        let mut server_keys = Vec::new();
        for key in &config.server_ids {
            let key = key.trim().to_string();
            if !key.is_empty() && !server_keys.contains(&key) {
                server_keys.push(key);
            }
        }
        config.server_ids = server_keys;
        if config.server_ids.is_empty() {
            return Err(CoreError::config("部署配置至少要选择一台服务器"));
        }
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
            // 逐台解析成真实 id：按名称 / host 写进来的、以及解析后撞同一台的都在这里收敛。
            let mut resolved_servers = Vec::with_capacity(config.server_ids.len());
            for key in &config.server_ids {
                let id = Store::find_server(app, key)?.id.clone();
                if !resolved_servers.contains(&id) {
                    resolved_servers.push(id);
                }
            }
            config.server_ids = resolved_servers;

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

    /// 按 id / 名称查找容器备份配置；id 精确命中优先，避免与名称歧义。
    pub fn find_container_config<'a>(
        config: &'a AppConfig,
        key: &str,
    ) -> Result<&'a ContainerConfig> {
        let key = key.trim();
        if let Some(item) = config
            .container_configs
            .iter()
            .find(|item| item.id == key)
        {
            return Ok(item);
        }
        config
            .container_configs
            .iter()
            .find(|item| item.name == key)
            .ok_or_else(|| CoreError::not_found(format!("容器备份配置不存在: {key}")))
    }

    /// 新建或更新一条容器备份配置：规范化服务器引用与迁移目标并校验名称唯一。
    /// 界面与调度器都从这里取参数，所以校验要在这里做完，别留到夜里执行时才报错。
    pub fn save_container_config(
        store: &Store,
        config: ContainerConfig,
    ) -> Result<ContainerConfig> {
        let mut config = config;
        config.name = config.name.trim().to_string();
        if config.name.is_empty() {
            return Err(CoreError::config("容器备份配置名称不能为空"));
        }
        config.project = config.project.trim().to_string();
        if config.project.is_empty() {
            return Err(CoreError::config("请选择要打包的 compose 项目"));
        }
        if !config.include_volumes && !config.include_images {
            return Err(CoreError::config("数据卷与镜像至少要勾选一项，否则备份包是空的"));
        }
        config.id = config.id.trim().to_string();
        config.server_id = config.server_id.trim().to_string();

        store.mutate_config(|app| {
            // 服务器必须以 id 形式存在；名称 / host 也允许（兼容 CLI）。
            let source = Store::find_server(app, &config.server_id)?.clone();
            config.server_id = source.id.clone();

            if let Some(target) = config.target.as_mut() {
                let key = target.server_id.trim().to_string();
                if key.is_empty() {
                    // 目标服务器留空 = 只备份到本机，不留一条指向来源机的空目标。
                    config.target = None;
                } else {
                    let server = Store::find_server(app, &key)?.clone();
                    if server.id == config.server_id {
                        return Err(CoreError::config("目标服务器不能与来源服务器相同"));
                    }
                    target.server_id = server.id.clone();
                    target.target_dir =
                        crate::container::validate_remote_dir(&target.target_dir)?;
                }
            }

            let by_id = app
                .container_configs
                .iter()
                .find(|item| item.id == config.id);
            if config.id.is_empty() {
                config.id = new_id();
            } else if by_id.is_none() {
                // 明确携带 id 却不存在（配置已被其它窗口删除）：报错而不是静默新建。
                return Err(CoreError::not_found(format!(
                    "容器备份配置不存在（可能已被删除）: {}",
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
                .container_configs
                .iter()
                .any(|item| item.id != config.id && item.name == config.name)
            {
                return Err(CoreError::config(format!(
                    "容器备份配置名称已存在: {}",
                    config.name
                )));
            }
            match app
                .container_configs
                .iter_mut()
                .find(|item| item.id == config.id)
            {
                Some(existing) => *existing = config.clone(),
                None => app.container_configs.push(config.clone()),
            }
            Ok(config.clone())
        })
    }

    /// 删除一条容器备份配置（不影响已有任务记录，本机备份包也留在原处）。
    pub fn delete_container_config(store: &Store, key: &str) -> Result<bool> {
        let key = key.trim();
        store.mutate_config(|app| {
            let removed: Vec<String> = if let Some(item) = app
                .container_configs
                .iter()
                .find(|item| item.id == key)
            {
                vec![item.id.clone()]
            } else {
                app.container_configs
                    .iter()
                    .filter(|item| item.name == key)
                    .map(|item| item.id.clone())
                    .collect()
            };
            if removed.is_empty() {
                return Ok(false);
            }
            app.container_configs.retain(|item| !removed.contains(&item.id));
            // 被删的配置若还在定时列表里，一并摘掉，避免每天到点报「配置不存在」。
            app.settings
                .scheduled_container_config_ids
                .retain(|id| !removed.contains(id));
            Ok(true)
        })
    }

    /// 服务器被删除后收敛容器备份配置：来源被删的整条删除（没有来源就无从打包），
    /// 迁移目标被删的降级成「只备份到本机」——定时任务不该因为一台机器没了就天天报错。
    pub fn detach_server_from_container_configs(config: &mut AppConfig, server_id: &str) {
        for saved in config.container_configs.iter_mut() {
            if saved
                .target
                .as_ref()
                .is_some_and(|target| target.server_id == server_id)
            {
                saved.target = None;
            }
        }
        config
            .container_configs
            .retain(|saved| saved.server_id != server_id);
        let alive: Vec<String> = config
            .container_configs
            .iter()
            .map(|item| item.id.clone())
            .collect();
        config
            .settings
            .scheduled_container_config_ids
            .retain(|id| alive.contains(id));
    }

    /// 列出所有 Pages 配置条目。
    pub fn list_pages_configs(config: &AppConfig) -> Vec<&PagesConfigEntry> {
        config.pages_configs.iter().collect()
    }

    /// 新建或更新一条 Pages 配置：仓库必须存在，同一仓库内名称唯一。
    /// id 留空表示新建（补 uuid），携带不存在的 id 则报错而不是静默新建。
    /// GUI 与 CLI 共用，保证两端保存行为一致。
    pub fn save_pages_config(store: &Store, entry: PagesConfigEntry) -> Result<PagesConfigEntry> {
        let mut entry = entry;
        entry.name = entry.name.trim().to_string();
        entry.repo_id = entry.repo_id.trim().to_string();
        entry.id = entry.id.trim().to_string();
        entry.config = entry.config.normalize();
        if entry.config.provider != "cloudflare" && entry.config.provider != "github" {
            return Err(CoreError::config(
                "不支持的 Pages 平台（可选 cloudflare / github）",
            ));
        }
        // GitHub 的远端地址要到部署时才验证得动，保存时只保证平台与分支齐备（normalize 已补默认值）。

        store.mutate_config(|app| {
            let repo = Store::find_repo(app, &entry.repo_id)?.clone();
            entry.repo_id = repo.id.clone();
            entry.repo_name = repo.name.clone();
            if entry.name.is_empty() {
                // 配置按仓库一份保存，界面没有单独的名称输入，沿用仓库名。
                entry.name = repo.name.clone();
            }

            let by_id = app.pages_configs.iter().find(|item| item.id == entry.id);
            if entry.id.is_empty() {
                entry.id = new_id();
            } else if by_id.is_none() {
                // 明确携带 id 却不存在（已被 CLI / 其它窗口删除）：报错而不是静默新建。
                return Err(CoreError::not_found(format!(
                    "Pages 配置不存在（可能已被删除）: {}",
                    entry.id
                )));
            }
            // 更新已有配置时以存储中的创建时间为准，避免调用方传入的旧快照覆盖。
            match by_id {
                Some(existing) if !existing.created_at.trim().is_empty() => {
                    entry.created_at = existing.created_at.clone();
                }
                _ => {
                    if entry.created_at.trim().is_empty() {
                        entry.created_at = now_string();
                    }
                }
            }
            if app
                .pages_configs
                .iter()
                .any(|item| {
                    item.id != entry.id && item.repo_id == entry.repo_id && item.name == entry.name
                })
            {
                return Err(CoreError::config(format!(
                    "该仓库下已存在同名 Pages 配置：{}",
                    entry.name
                )));
            }
            match app
                .pages_configs
                .iter_mut()
                .find(|item| item.id == entry.id)
            {
                Some(existing) => *existing = entry.clone(),
                None => app.pages_configs.push(entry.clone()),
            }
            // 保存即绑定：Pages 部署按仓库解析默认配置（`get_repo_default_pages`），
            // 不写这条引用的话，界面上保存成功的配置对部署永远是透明的。
            if let Some(repo) = app.repos.iter_mut().find(|item| item.id == entry.repo_id) {
                repo.default_pages_config_id = Some(entry.id.clone());
            }
            Ok(entry.clone())
        })
    }

    /// 删除一条 Pages 配置（按 id 精确删除）。
    pub fn delete_pages_config(config: &mut AppConfig, id: &str) -> Result<bool> {
        let initial_count = config.pages_configs.len();
        config.pages_configs.retain(|e| e.id != id);
        if config.pages_configs.len() == initial_count {
            return Ok(false);
        }
        // 如果该仓库的默认 Pages 配置被删除，清空默认引用
        for repo in &mut config.repos {
            if repo.default_pages_config_id.as_ref() == Some(&id.to_string()) {
                repo.default_pages_config_id = None;
            }
        }
        Ok(true)
    }

    /// 获取仓库的默认 Pages 配置（通过 default_pages_config_id 查找）。
    pub fn get_repo_default_pages<'a>(config: &'a AppConfig, repo_id: &str) -> Option<&'a PagesConfigEntry> {
        let repo = config.repos.iter().find(|r| r.id == repo_id)?;
        let id = repo.default_pages_config_id.as_ref()?;
        config.pages_configs.iter().find(|e| e.id == *id)
    }

    /// 启动时迁移旧版 repo.pages 到 pages_configs 列表（只执行一次）。
    pub fn migrate_pages_configs(&self) -> Result<usize> {
        if self.load_config()?.pages_configs_migrated {
            return Ok(0);
        }
        self.mutate_config(|config| {
            if config.pages_configs_migrated {
                return Ok(0);
            }
            let mut additions: Vec<PagesConfigEntry> = Vec::new();
            for repo in &mut config.repos {
                if let Some(pages_config) = repo.pages.take() {
                    let entry = PagesConfigEntry {
                        id: new_id(),
                        name: "default".to_string(),
                        repo_id: repo.id.clone(),
                        repo_name: repo.name.clone(),
                        config: pages_config,
                        created_at: now_string(),
                    };
                    additions.push(entry.clone());
                    repo.default_pages_config_id = Some(entry.id);
                }
            }
            config.pages_configs.extend(additions.clone());
            config.pages_configs_migrated = true;
            Ok(additions.len())
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

/// 判断两个本地路径是否指向同一处：统一分隔符、忽略结尾斜杠。
/// GUI 与 CLI 的仓库查重都走这里，避免各处自己写一套归一化而宽严不一。
pub fn paths_equal(a: &str, b: &str) -> bool {
    let normalize = |p: &str| {
        let normalized = p.replace('\\', "/");
        // 大小写不敏感只适用于 Windows；Linux 上 /srv/App 与 /srv/app 是两个不同仓库。
        #[cfg(windows)]
        let normalized = normalized.to_lowercase();
        normalized.trim_end_matches('/').to_string()
    };
    normalize(a) == normalize(b)
}

/// 三类任务记录共有的字段访问，供通用列表 CRUD 复用。
trait TaskRecord: Clone {
    fn id(&self) -> &str;
    fn status_mut(&mut self) -> &mut DeployStatus;
    fn error_mut(&mut self) -> &mut Option<String>;
    fn finished_at_mut(&mut self) -> &mut Option<String>;
}

impl TaskRecord for DeployRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

impl TaskRecord for BackupRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

impl TaskRecord for PagesDeployRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

impl TaskRecord for ContainerRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

const HISTORY_LABEL: &str = "部署记录";
const BACKUP_LABEL: &str = "备份记录";
const PAGES_LABEL: &str = "Pages 记录";
const PAGES_MISSING_LABEL: &str = "Pages 部署记录";
const CONTAINER_LABEL: &str = "容器记录";

/// 解析导出文件并校验版本。
fn parse_export(json_str: &str) -> Result<ExportData> {
    let data: ExportData = serde_json::from_str(json_str)
        .map_err(|e| CoreError::config(format!("配置文件格式错误：{}", e)))?;
    if data.version != "1.0" {
        return Err(CoreError::config(format!(
            "不支持的配置文件版本：{}",
            data.version
        )));
    }
    Ok(data)
}

/// 导出必然把敏感字段写成空串，所以「空」在这里的语义是「本次没有提供」，保留本机值。
fn keep_when_blank(incoming: &mut String, current: &str, kept: &mut usize) {
    if incoming.trim().is_empty() && !current.trim().is_empty() {
        *incoming = current.to_string();
        *kept += 1;
    }
}

/// 合并服务器凭据。逐字段判断，避免把导出的空值写进本机真实凭据。
fn merge_auth(incoming: &mut SshAuth, current: &SshAuth, kept: &mut usize) {
    // 本机用密钥、导入文件里却是一个空密码认证：整套保留本机，否则导入后必然连不上。
    let downgrade_to_blank_password = matches!(
        (&*incoming, current),
        (SshAuth::Password { password }, SshAuth::PrivateKey { .. }) if password.trim().is_empty()
    );
    if downgrade_to_blank_password {
        *incoming = current.clone();
        *kept += 1;
        return;
    }
    match (incoming, current) {
        (SshAuth::Password { password }, SshAuth::Password { password: saved }) => {
            keep_when_blank(password, saved, kept);
        }
        // 私钥口令在导出时统一被抹成 None，与「本来就没有口令」无法区分，只能按空值处理。
        (
            SshAuth::PrivateKey { passphrase, .. },
            SshAuth::PrivateKey { passphrase: Some(saved), .. },
        ) if passphrase.is_none() => {
            *passphrase = Some(saved.clone());
            *kept += 1;
        }
        _ => {}
    }
}

/// 数据库口令：导出时必然被抹成空串，空即「本次没有提供」，保留本机值。
/// 采集方式 / 容器名 / 库名 / schema 不是凭据，照文件走。
fn merge_db_source(incoming: &mut DbBackupSource, current: &DbBackupSource, kept: &mut usize) {
    keep_when_blank(&mut incoming.password, &current.password, kept);
}

/// 可空连接串：导出时统一抹成 `None`，与「本机没有这项」无法区分（同私钥口令）。
/// 本机存过就继续用本机的；换机导入时本机没有，留空让用户自己填，
/// 绝不能把 `Some("")` 落盘——那会被下游当成一条可用的连接串。
fn keep_option_when_absent(
    incoming: &mut Option<String>,
    current: &Option<String>,
    kept: &mut usize,
) {
    if incoming.is_none() && current.is_some() {
        *incoming = current.clone();
        *kept += 1;
    }
}

/// env 文件按数组下标对齐：导出保留了顺序与条数，脱敏后只剩空串，只能按序回填。
fn merge_env_files(incoming: &mut Vec<EnvFileConfig>, current: &[EnvFileConfig], kept: &mut usize) {
    for (index, file) in incoming.iter_mut().enumerate() {
        let Some(saved) = current.get(index) else { break };
        keep_when_blank(&mut file.local_path, &saved.local_path, kept);
        keep_when_blank(&mut file.remote_path, &saved.remote_path, kept);
    }
}

/// 按 id 合并一类配置：同 ID 覆盖（覆盖前用 `preserve` 把导出的空值换回本机值），新 ID 追加。
fn merge_by_id<T, K, P>(target: &mut Vec<T>, incoming: &[T], key: K, mut preserve: P) -> ImportCounts
where
    T: Clone,
    K: Fn(&T) -> &str,
    P: FnMut(&mut T, &T),
{
    let mut counts = ImportCounts::default();
    for item in incoming {
        match target.iter().position(|existing| key(existing) == key(item)) {
            Some(index) => {
                let mut merged = item.clone();
                preserve(&mut merged, &target[index]);
                target[index] = merged;
                counts.overwritten += 1;
            }
            None => {
                target.push(item.clone());
                counts.added += 1;
            }
        }
    }
    counts
}

/// 把导出内容合并进本机配置，返回本次的去向统计。
///
/// 预览与实际导入共用这一个函数，因此「确认框里看到的」与「真正落盘的」必然一致。
fn merge_export(config: &mut AppConfig, data: &ExportData) -> ImportPreview {
    let mut kept = 0usize;
    let mut preview = ImportPreview {
        exported_at: data.exported_at.clone(),
        ..Default::default()
    };

    preview.servers = merge_by_id(
        &mut config.servers,
        &data.servers,
        |item| item.id.as_str(),
        |incoming: &mut ServerConfig, current: &ServerConfig| {
            merge_auth(&mut incoming.auth, &current.auth, &mut kept);
            if let (Some(incoming), Some(current)) =
                (incoming.db_backup.as_mut(), current.db_backup.as_ref())
            {
                merge_db_source(incoming, current, &mut kept);
            }
            keep_option_when_absent(
                &mut incoming.supabase_url,
                &current.supabase_url,
                &mut kept,
            );
        },
    );
    preview.repos = merge_by_id(
        &mut config.repos,
        &data.repos,
        |item| item.id.as_str(),
        |incoming, current| merge_env_files(&mut incoming.env_files, &current.env_files, &mut kept),
    );
    preview.backup_targets = merge_by_id(
        &mut config.backup_targets,
        &data.backup_targets,
        |item| item.id.as_str(),
        // 目标连接串带口令，导出时整条抹空：本机有值就继续用本机的。
        |incoming, current| keep_when_blank(&mut incoming.url, &current.url, &mut kept),
    );
    preview.deploy_configs = merge_by_id(
        &mut config.deploy_configs,
        &data.deploy_configs,
        |item| item.id.as_str(),
        |_, _| {},
    );
    preview.backup_configs = merge_by_id(
        &mut config.backup_configs,
        &data.backup_configs,
        |item| item.id.as_str(),
        |incoming: &mut BackupConfig, current: &BackupConfig| {
            merge_db_source(&mut incoming.source, &current.source, &mut kept);
            keep_option_when_absent(
                &mut incoming.supabase_url,
                &current.supabase_url,
                &mut kept,
            );
        },
    );
    preview.pages_configs = merge_by_id(
        &mut config.pages_configs,
        &data.pages_configs,
        |item| item.id.as_str(),
        |_, _| {},
    );
    // 容器备份配置只引用服务器 id 与 compose 项目名，本身不含凭据，整体跟随导入文件。
    preview.container_configs = merge_by_id(
        &mut config.container_configs,
        &data.container_configs,
        |item| item.id.as_str(),
        |_, _| {},
    );

    // 设置整体跟随导入文件（换机迁移主要靠它），但导出的空凭据一律保留本机值。
    let mut settings = data.settings.clone();
    keep_when_blank(
        &mut settings.cloudflare_api_token,
        &config.settings.cloudflare_api_token,
        &mut kept,
    );
    keep_when_blank(
        &mut settings.cloudflare_account_id,
        &config.settings.cloudflare_account_id,
        &mut kept,
    );
    keep_when_blank(&mut settings.github_token, &config.settings.github_token, &mut kept);
    keep_when_blank(
        &mut settings.cronjob_api_key,
        &config.settings.cronjob_api_key,
        &mut kept,
    );
    if settings.master_password_hash.is_none() && config.settings.master_password_hash.is_some() {
        settings.master_password_hash = config.settings.master_password_hash.clone();
        kept += 1;
    }
    config.settings = settings;

    preview.kept_local_secrets = kept;
    preview
}

fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
    let text = serde_json::to_string_pretty(config)?;
    atomic_write(path, &text)
}

fn load_records<T: serde::de::DeserializeOwned>(path: &Path, label: &str) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| CoreError::io_path(path, e))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text)
        .map_err(|e| CoreError::config(format!("{label}解析失败 {}: {e}", path.display())))
}

fn write_records<T: serde::Serialize>(path: &Path, records: &[T]) -> Result<()> {
    let text = serde_json::to_string_pretty(records)?;
    atomic_write(path, &text)
}

/// 新增或更新一条记录（按 id 匹配），并按 `limit` 裁剪最早的多余记录。
fn upsert_record<T>(path: &Path, record: &T, limit: usize, label: &str) -> Result<()>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    let mut records = load_records::<T>(path, label)?;
    match records.iter_mut().find(|r| r.id() == record.id()) {
        Some(existing) => *existing = record.clone(),
        None => records.push(record.clone()),
    }
    if limit > 0 && records.len() > limit {
        let overflow = records.len() - limit;
        records.drain(0..overflow);
    }
    write_records(path, &records)
}

/// 按 id 查找记录：精确命中优先，其次允许唯一前缀匹配。
fn find_record_by_prefix<T>(
    path: &Path,
    id: &str,
    parse_label: &str,
    missing_label: &str,
) -> Result<T>
where
    T: TaskRecord + serde::de::DeserializeOwned,
{
    let records = load_records::<T>(path, parse_label)?;
    if let Some(record) = records.iter().find(|r| r.id() == id) {
        return Ok(record.clone());
    }
    let matches: Vec<_> = records.iter().filter(|r| r.id().starts_with(id)).collect();
    match matches.len() {
        0 => Err(CoreError::not_found(format!("{missing_label}不存在: {id}"))),
        1 => Ok(matches[0].clone()),
        _ => Err(CoreError::config("记录 ID 不唯一，请输入完整 ID")),
    }
}

fn remove_record<T>(path: &Path, id: &str, label: &str) -> Result<bool>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    Ok(remove_records_by_id::<T>(path, &[id.to_string()], label)? > 0)
}

/// 批量删除记录：一次加锁、一次写回，比逐条删除少掉 N-1 次读写。返回真正删掉的条数。
fn remove_records_by_id<T>(path: &Path, ids: &[String], label: &str) -> Result<usize>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    if ids.is_empty() {
        return Ok(0);
    }
    let mut records = load_records::<T>(path, label)?;
    let before = records.len();
    records.retain(|r| !ids.iter().any(|id| id.as_str() == r.id()));
    let removed = before - records.len();
    if removed > 0 {
        write_records(path, &records)?;
    }
    Ok(removed)
}

/// 把列表中遗留的 Running 记录收敛为失败；返回转换数量。
///
/// 崩溃后无法得知真实结束时间，`duration_ms` 保持 0（界面显示为未知），
/// 不能用「下次启动时刻」计算，否则会把停机时间也算进耗时。
fn mark_running_as_interrupted<T>(path: &Path, label: &str, message: &str) -> Result<usize>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    let mut records = load_records::<T>(path, label)?;
    let mut converted = 0usize;
    for record in records.iter_mut() {
        if *record.status_mut() == DeployStatus::Running {
            *record.status_mut() = DeployStatus::Failed;
            *record.error_mut() = Some(message.to_string());
            *record.finished_at_mut() = Some(now_string());
            converted += 1;
        }
    }
    if converted > 0 {
        write_records(path, &records)?;
    }
    Ok(converted)
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
    use crate::models::Settings;

    fn temp_store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-import-{}",
            uuid::Uuid::new_v4()
        ));
        (Store::new(&dir), dir)
    }

    /// 带真实凭据的配置：密码认证服务器 + env 文件仓库 + 两个 API Token。
    fn config_with_secrets() -> AppConfig {
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: "s3cret".to_string(),
            },
        );
        server.id = "s1".to_string();
        let mut repo = RepoConfig::new("web".to_string(), "D:/web".to_string());
        repo.id = "r1".to_string();
        repo.env_files.push(EnvFileConfig {
            local_path: "D:/web/.env".to_string(),
            remote_path: ".env".to_string(),
        });
        let mut settings = Settings::default();
        settings.github_token = "gh_secret".to_string();
        settings.cronjob_api_key = "cj_secret".to_string();
        AppConfig {
            servers: vec![server],
            repos: vec![repo],
            settings,
            ..Default::default()
        }
    }

    /// 只替换认证信息的导出文件，其它字段用来验证「非敏感字段跟随文件」。
    fn export_with_server(server: ServerConfig) -> String {
        let data = ExportData {
            version: "1.0".to_string(),
            exported_at: "2026-01-01 00:00:00".to_string(),
            servers: vec![server],
            repos: Vec::new(),
            backup_targets: Vec::new(),
            deploy_configs: Vec::new(),
            backup_configs: Vec::new(),
            pages_configs: Vec::new(),
            container_configs: Vec::new(),
            settings: Settings::default(),
        };
        serde_json::to_string(&data).unwrap()
    }

    #[test]
    fn export_then_import_preserves_local_credentials() {
        let (store, dir) = temp_store();
        store.save_config(&config_with_secrets()).unwrap();
        let exported = store.export_config().unwrap();

        // 导出之后本机改过主机名：非敏感字段应跟随文件，凭据不能被导出的空值冲掉。
        let mut current = store.load_config().unwrap();
        current.servers[0].host = "10.0.0.9".to_string();
        store.save_config(&current).unwrap();

        let preview = store.import_config(&exported).unwrap();
        let after = store.load_config().unwrap();

        assert_eq!(after.servers[0].host, "10.0.0.1");
        assert!(
            matches!(&after.servers[0].auth, SshAuth::Password { password } if password == "s3cret"),
            "导出的空密码覆盖了本机密码"
        );
        assert_eq!(after.repos[0].env_files[0].local_path, "D:/web/.env");
        assert_eq!(after.settings.github_token, "gh_secret");
        assert_eq!(after.settings.cronjob_api_key, "cj_secret");
        assert_eq!(preview.servers.overwritten, 1);
        // 密码 1 + env 本地/远端路径 2 + Token 2
        assert_eq!(preview.kept_local_secrets, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_blankout_does_not_erase_local_db_credentials() {
        let (store, dir) = temp_store();
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "D:/id_ed25519".to_string(),
                passphrase: None,
            },
        );
        server.id = "s1".to_string();
        server.db_backup = Some(DbBackupSource {
            container: "pg".to_string(),
            database: "appdb".to_string(),
            username: "postgres".to_string(),
            password: "db_local".to_string(),
            ..DbBackupSource::default()
        });
        server.supabase_url = Some("postgres://u:db_local@h/db".to_string());
        let mut target =
            crate::models::BackupTarget::new("目标库".to_string(), "postgres://u:tgt_local@h:5432/db".to_string());
        target.id = "t1".to_string();
        let mut backup = BackupConfig::new(
            "每晚".to_string(),
            "s1".to_string(),
            DbBackupSource {
                database: "appdb".to_string(),
                password: "src_local".to_string(),
                ..DbBackupSource::default()
            },
        );
        backup.id = "b1".to_string();
        backup.supabase_url = Some("postgres://u:cfg_local@h/db".to_string());
        store
            .save_config(&AppConfig {
                servers: vec![server],
                backup_targets: vec![target],
                backup_configs: vec![backup],
                ..Default::default()
            })
            .unwrap();

        let exported = store.export_config().unwrap();
        for secret in ["db_local", "tgt_local", "src_local", "cfg_local"] {
            assert!(!exported.contains(secret), "导出内容泄露了 {secret}");
        }

        // 导回本机：被抹掉的凭据必须原样留着，否则一次导入就把备份功能打废。
        let preview = store.import_config(&exported).unwrap();
        let after = store.load_config().unwrap();
        assert_eq!(after.servers[0].db_backup.as_ref().unwrap().password, "db_local");
        assert_eq!(
            after.servers[0].supabase_url.as_deref(),
            Some("postgres://u:db_local@h/db")
        );
        assert_eq!(after.backup_targets[0].url, "postgres://u:tgt_local@h:5432/db");
        assert_eq!(after.backup_configs[0].source.password, "src_local");
        assert_eq!(
            after.backup_configs[0].supabase_url.as_deref(),
            Some("postgres://u:cfg_local@h/db")
        );
        // 服务器 2 + 目标 1 + 备份配置 2
        assert_eq!(preview.kept_local_secrets, 5);

        // 换机导入（本机没有对应值）：不能落进 Some("")，那会被下游当成一条可用连接串。
        let (fresh, fresh_dir) = temp_store();
        fresh.import_config(&exported).unwrap();
        let moved = fresh.load_config().unwrap();
        assert_eq!(moved.servers[0].supabase_url, None);
        assert!(moved.backup_targets[0].url.is_empty());
        assert_eq!(moved.servers[0].db_backup.as_ref().unwrap().database, "appdb");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh_dir);
    }

    #[test]
    fn preview_import_counts_added_and_overwritten_without_writing() {
        let (src, src_dir) = temp_store();
        src.save_config(&config_with_secrets()).unwrap();
        let mut two = src.load_config().unwrap();
        let mut extra = ServerConfig::new(
            "stage".to_string(),
            "10.0.0.2".to_string(),
            "deploy".to_string(),
            SshAuth::Password {
                password: "p2".to_string(),
            },
        );
        extra.id = "s2".to_string();
        two.servers.push(extra);
        src.save_config(&two).unwrap();
        let exported = src.export_config().unwrap();

        let (store, dir) = temp_store();
        store.save_config(&config_with_secrets()).unwrap();

        let preview = store.preview_import(&exported).unwrap();
        assert_eq!(preview.servers.added, 1);
        assert_eq!(preview.servers.overwritten, 1);
        assert_eq!(
            store.load_config().unwrap().servers.len(),
            1,
            "预览不得写盘"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn import_keeps_passphrase_but_follows_file_for_key_path() {
        let (store, dir) = temp_store();
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "C:/keys/id_ed25519".to_string(),
                passphrase: Some("pp".to_string()),
            },
        );
        server.id = "s1".to_string();
        store
            .save_config(&AppConfig {
                servers: vec![server],
                ..Default::default()
            })
            .unwrap();

        // 导出把口令抹成 None，这与「本来就没有口令」无法区分，只能按未提供处理。
        let mut incoming = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.8".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "C:/keys/new_path".to_string(),
                passphrase: None,
            },
        );
        incoming.id = "s1".to_string();
        store.import_config(&export_with_server(incoming)).unwrap();

        let after = store.load_config().unwrap();
        match &after.servers[0].auth {
            SshAuth::PrivateKey { key_path, passphrase } => {
                assert_eq!(key_path, "C:/keys/new_path");
                assert_eq!(passphrase.as_deref(), Some("pp"));
            }
            other => panic!("认证方式被改写: {other:?}"),
        }
        assert_eq!(after.servers[0].host, "10.0.0.8");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_does_not_downgrade_key_auth_to_blank_password() {
        let (store, dir) = temp_store();
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "C:/keys/id_ed25519".to_string(),
                passphrase: None,
            },
        );
        server.id = "s1".to_string();
        store
            .save_config(&AppConfig {
                servers: vec![server],
                ..Default::default()
            })
            .unwrap();

        let mut incoming = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.8".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: String::new(),
            },
        );
        incoming.id = "s1".to_string();
        let preview = store.import_config(&export_with_server(incoming)).unwrap();

        let after = store.load_config().unwrap();
        assert!(
            matches!(&after.servers[0].auth, SshAuth::PrivateKey { key_path, .. } if key_path == "C:/keys/id_ed25519"),
            "空密码导入把密钥认证降级了"
        );
        assert_eq!(preview.kept_local_secrets, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_history_many_drops_only_selected_ids() {
        let (store, dir) = temp_store();
        for id in ["a", "b", "c"] {
            let record: DeployRecord = serde_json::from_str(&format!(
                r#"{{
                    "id":"{id}","repoId":"repo","repoName":"demo","rev":"main","branch":"main",
                    "commit":"abc","commitShort":"abc","commitSubject":"init","serverId":"srv",
                    "serverName":"prod","targetDir":"/srv/app","scriptDir":"docker","scripts":[],
                    "runScripts":false,"envFiles":[],"status":"success","error":null,"log":"",
                    "startedAt":"2026-01-01 00:00:00","finishedAt":null,"durationMs":0
                }}"#
            ))
            .unwrap();
            store.upsert_history(&record, 0).unwrap();
        }

        // 混入不存在的 id：返回值只计真正删掉的条数。
        let removed = store
            .remove_history_many(&["a".to_string(), "c".to_string(), "gone".to_string()])
            .unwrap();
        assert_eq!(removed, 2);
        let left = store.load_history().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "b");
        assert_eq!(store.remove_history_many(&[]).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

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

        // 四类任务锁（部署 / 备份 / Pages / 容器）都空闲时，才把遗留的 Running 记录收敛为失败。
        assert_eq!(store.reconcile_interrupted().unwrap(), 1);
        assert_eq!(
            store.load_history().unwrap()[0].status,
            DeployStatus::Failed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 容器记录走同一套 CRUD：按 id 覆盖、超限裁剪，崩溃后一起收敛为失败。
    #[test]
    fn container_records_upsert_trim_and_mark_interrupted() {
        let dir = std::env::temp_dir()
            .join(format!("deploycode-store-containers-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let record = |id: &str, status: DeployStatus| ContainerRecord {
            id: id.to_string(),
            kind: crate::models::ContainerRecordKind::Backup,
            project: "blog".to_string(),
            server_id: "s1".to_string(),
            server_name: "prod".to_string(),
            target_server_id: String::new(),
            target_server_name: String::new(),
            target_dir: String::new(),
            bundle_path: format!("/tmp/{id}.tar"),
            bundle_size: 10,
            services: vec!["web".to_string()],
            volumes: vec!["blog_data".to_string()],
            images: Vec::new(),
            include_volumes: true,
            include_images: true,
            status,
            error: None,
            log: String::new(),
            started_at: "2026-01-01 00:00:00".to_string(),
            finished_at: None,
            duration_ms: 0,
        };

        store
            .upsert_container(&record("c1", DeployStatus::Running), 5)
            .unwrap();
        store
            .upsert_container(&record("c2", DeployStatus::Success), 5)
            .unwrap();
        // 同 id 再写一次是覆盖而不是追加。
        store
            .upsert_container(&record("c1", DeployStatus::Success), 5)
            .unwrap();
        assert_eq!(store.load_containers().unwrap().len(), 2);
        assert_eq!(
            store.find_container_record("c1").unwrap().status,
            DeployStatus::Success
        );

        // 超出保留条数时丢最旧的。
        store
            .upsert_container(&record("c3", DeployStatus::Success), 2)
            .unwrap();
        assert_eq!(store.load_containers().unwrap().len(), 2);

        store
            .upsert_container(&record("c4", DeployStatus::Running), 5)
            .unwrap();
        assert_eq!(store.mark_interrupted().unwrap(), 1);
        let running_left = store
            .load_containers()
            .unwrap()
            .into_iter()
            .filter(|item| item.status == DeployStatus::Running)
            .count();
        assert_eq!(running_left, 0);

        store.remove_container_record("c3").unwrap();
        assert!(store.find_container_record("c3").is_err());
        store.clear_container_records().unwrap();
        assert!(store.load_containers().unwrap().is_empty());
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
                server_ids: vec!["prod".to_string()],
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
        assert_eq!(saved.server_ids, vec![server_id.clone()]);
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

        // 任意一台服务器不存在就整条拒绝。
        let mut missing = saved.clone();
        missing.server_ids = vec![server_id.clone(), "nope".to_string()];
        assert!(Store::save_deploy_config(&store, missing).is_err());

        // 一台都没选时拒绝，避免留下永远发不出去的配置。
        let mut empty = saved.clone();
        empty.server_ids = Vec::new();
        assert!(Store::save_deploy_config(&store, empty).is_err());

        // 多台可以按名称 / host 传入：统一换成 id、保持执行顺序，指向同一台的重复项收敛成一条。
        let second = ServerConfig::new(
            "stage".to_string(),
            "h2".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let second_id = second.id.clone();
        store
            .mutate_config(|app| {
                app.servers.push(second);
                Ok(())
            })
            .unwrap();
        let mut many = saved.clone();
        // 新建走空 id：带未知 id 会被「配置不存在」守卫拒绝。
        many.id = String::new();
        many.name = "多机部署".to_string();
        many.server_ids = vec!["stage".to_string(), "prod".to_string(), "h2".to_string()];
        let many = Store::save_deploy_config(&store, many).unwrap();
        assert_eq!(many.server_ids, vec![second_id, server_id.clone()]);

        // 更新同一条配置不会重复插入，且保留原创建时间。
        let created_at = saved.created_at.clone();
        let mut renamed = saved.clone();
        renamed.name = "生产部署 2".to_string();
        renamed.created_at = String::new();
        let renamed = Store::save_deploy_config(&store, renamed).unwrap();
        assert_eq!(renamed.created_at, created_at);
        // 只有「多机部署」那一条是新增的：更新原配置没有产生重复条目。
        assert_eq!(store.load_config().unwrap().deploy_configs.len(), 2);

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
        // 只剩「多机部署」那一条：删除按 id 精确命中，不牵连其它配置。
        let left = store.load_config().unwrap().deploy_configs;
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "多机部署");
        assert!(!Store::delete_deploy_config(&store, &saved.id).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_container_config_normalizes_target_and_prunes_schedule() {
        use crate::models::{ContainerConfig, ContainerTarget};

        let (store, dir) = temp_store();
        let mut config = AppConfig::default();
        for name in ["prod", "stage"] {
            config.servers.push(ServerConfig::new(
                name.to_string(),
                format!("h-{name}"),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            ));
        }
        let ids: Vec<String> = config.servers.iter().map(|item| item.id.clone()).collect();
        store.save_config(&config).unwrap();

        // 服务器按名称传入也认：保存时换成 id，目标目录顺带规范掉结尾斜杠。
        let saved = Store::save_container_config(
            &store,
            ContainerConfig {
                id: String::new(),
                name: " 博客 ".to_string(),
                server_id: "prod".to_string(),
                project: " lf-blog ".to_string(),
                pause_source: false,
                include_volumes: true,
                include_images: false,
                target: Some(ContainerTarget {
                    server_id: "stage".to_string(),
                    target_dir: "/opt/blog/".to_string(),
                    start_services: true,
                }),
                created_at: String::new(),
            },
        )
        .unwrap();
        assert_eq!(saved.name, "博客");
        assert_eq!(saved.project, "lf-blog");
        assert_eq!(saved.server_id, ids[0]);
        let target = saved.target.clone().expect("迁移目标应保留");
        assert_eq!(target.server_id, ids[1]);
        assert_eq!(target.target_dir, "/opt/blog");

        // 卷和镜像都不勾 = 空备份包，保存时就该拒绝，而不是等夜里跑出一个空包。
        let mut empty_bundle = saved.clone();
        empty_bundle.id = String::new();
        empty_bundle.name = "空包".to_string();
        empty_bundle.include_volumes = false;
        assert!(Store::save_container_config(&store, empty_bundle).is_err());

        // 目标与来源同一台：compose 项目名在单机上会撞车，直接拒绝。
        let mut same = saved.clone();
        same.id = String::new();
        same.name = "同机".to_string();
        if let Some(target) = same.target.as_mut() {
            target.server_id = ids[0].clone();
        }
        assert!(Store::save_container_config(&store, same).is_err());

        // 目标服务器留空 = 降级成只备份到本机，不留一条指向来源机的空目标。
        let mut no_target = saved.clone();
        no_target.id = String::new();
        no_target.name = "只备份".to_string();
        no_target.target = Some(ContainerTarget {
            server_id: "  ".to_string(),
            target_dir: String::new(),
            start_services: true,
        });
        let no_target = Store::save_container_config(&store, no_target).unwrap();
        assert!(no_target.target.is_none());

        // 定时列表指向被删配置时一起清掉，避免到点报「配置不存在」。
        store
            .mutate_config(|app| {
                app.settings
                    .scheduled_container_config_ids
                    .push(saved.id.clone());
                Ok(())
            })
            .unwrap();
        assert!(Store::delete_container_config(&store, &saved.id).unwrap());
        let after = store.load_config().unwrap();
        assert!(
            after
                .settings
                .scheduled_container_config_ids
                .iter()
                .all(|id| id != &saved.id),
            "已删除的容器配置仍留在定时列表里"
        );

        // 删掉来源服务器：整条配置失去意义，连同定时引用一起消失；
        // 只当过迁移目标的那条则降级为纯备份，任务照跑。
        let mut config = store.load_config().unwrap();
        config.container_configs.clear();
        config.container_configs.push(no_target.clone());
        config.container_configs.push(saved.clone());
        config.settings.scheduled_container_config_ids =
            vec![no_target.id.clone(), saved.id.clone()];
        store.save_config(&config).unwrap();
        Store::detach_server_from_container_configs(&mut config, &ids[1]);
        assert_eq!(config.container_configs.len(), 2);
        assert!(config.container_configs[1].target.is_none());
        Store::detach_server_from_container_configs(&mut config, &ids[0]);
        assert!(config.container_configs.is_empty());
        assert!(config.settings.scheduled_container_config_ids.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 保存的配置是「手动一键执行」与「定时执行」唯一的参数来源：
    /// `request()` 少带一个字段，夜里跑的就不是白天存的那次操作，而且不会报错。
    /// 所以这里把 存配置 -> request() -> 引擎 整条链钉住。
    #[test]
    fn saved_container_config_feeds_the_engine_unchanged() {
        use crate::container::{ContainerEngine, ContainerJob};
        use crate::models::{ContainerConfig, ContainerRecordKind, ContainerTarget};

        let (store, dir) = temp_store();
        // 引擎按 `Arc<Store>` 持有存储，与 src-tauri 里的用法保持一致。
        let store = std::sync::Arc::new(store);
        let mut config = AppConfig::default();
        for name in ["prod", "stage"] {
            config.servers.push(ServerConfig::new(
                name.to_string(),
                format!("h-{name}"),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            ));
        }
        store.save_config(&config).unwrap();

        let migrate = Store::save_container_config(
            &store,
            ContainerConfig {
                id: String::new(),
                name: "博客迁移".to_string(),
                server_id: "prod".to_string(),
                project: "lf-blog".to_string(),
                pause_source: true,
                include_volumes: true,
                include_images: false,
                target: Some(ContainerTarget {
                    server_id: "stage".to_string(),
                    target_dir: "/opt/blog".to_string(),
                    start_services: false,
                }),
                created_at: String::new(),
            },
        )
        .unwrap();

        let engine = ContainerEngine::new(store.clone());
        let (record, job) = engine.prepare(&migrate.request()).unwrap();
        assert_eq!(record.kind, ContainerRecordKind::Migrate);
        assert_eq!(record.server_name, "prod");
        assert_eq!(record.target_server_name, "stage");
        assert_eq!(record.target_dir, "/opt/blog");
        match job {
            ContainerJob::Snapshot(plan) => {
                assert_eq!(plan.project, "lf-blog");
                assert!(plan.pause_source, "暂停来源机的选项必须传到引擎");
                assert!(plan.include_volumes);
                assert!(!plan.include_images, "只勾卷时不该带上镜像");
                let (server, target_dir, start) = plan.target.as_ref().expect("迁移目标应传到引擎");
                assert_eq!(server.name, "stage");
                assert_eq!(target_dir, "/opt/blog");
                assert!(!*start, "配置里没勾启动服务，到点不该 compose up");
            }
            ContainerJob::Restore(_) => panic!("快照配置不该产出恢复任务"),
        }

        // 同一份配置去掉目标：降级成纯备份， kinds 与目标字段都要跟着变。
        let mut backup_only = migrate.clone();
        backup_only.id = String::new();
        backup_only.name = "博客只备份".to_string();
        backup_only.target = None;
        let backup_only = Store::save_container_config(&store, backup_only).unwrap();
        let (record, job) = engine.prepare(&backup_only.request()).unwrap();
        assert_eq!(record.kind, ContainerRecordKind::Backup);
        assert!(record.target_server_id.is_empty());
        match job {
            ContainerJob::Snapshot(plan) => assert!(plan.target.is_none()),
            ContainerJob::Restore(_) => panic!("快照配置不该产出恢复任务"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_pages_config_resolves_repo_and_keeps_one_entry_per_id() {
        use crate::models::PagesConfig;

        let dir = std::env::temp_dir().join(format!(
            "deploycode-store-pagescfg-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        let repo = RepoConfig::new("demo".to_string(), "/tmp/demo".to_string());
        let repo_id = repo.id.clone();
        config.repos.push(repo);
        store.save_config(&config).unwrap();

        let draft = PagesConfigEntry {
            id: String::new(),
            name: String::new(),
            repo_id: "demo".to_string(),
            repo_name: String::new(),
            config: PagesConfig {
                provider: " Cloudflare ".to_string(),
                project_name: " site ".to_string(),
                build_command: String::new(),
                output_dir: String::new(),
                branch: String::new(),
                publish_branch: String::new(),
            },
            created_at: String::new(),
        };

        // 新建：id / 创建时间由后端补齐，仓库按名称解析成 id 并回填名称与 repo_name。
        let saved = Store::save_pages_config(&store, draft).unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.repo_id, repo_id);
        assert_eq!(saved.repo_name, "demo");
        assert_eq!(saved.name, "demo");
        assert!(!saved.created_at.is_empty());
        assert_eq!(saved.config.provider, "cloudflare");
        assert_eq!(saved.config.project_name, "site");
        // normalize 补的回填默认值：输出目录与两个分支名不能留空。
        assert_eq!(saved.config.output_dir, "dist");
        assert_eq!(saved.config.branch, "main");
        assert_eq!(saved.config.publish_branch, "gh-pages");
        assert_eq!(store.load_config().unwrap().pages_configs.len(), 1);
        // 保存会把这个仓库的默认 Pages 配置指过来，否则部署侧永远读不到这条配置。
        assert_eq!(
            store.load_config().unwrap().repos[0].default_pages_config_id,
            Some(saved.id.clone())
        );

        // 同一 id 再保存是覆盖，不产生第二条，创建时间以存储里的为准。
        let mut edited = saved.clone();
        edited.name = "官网".to_string();
        edited.created_at = "旧快照".to_string();
        let edited = Store::save_pages_config(&store, edited).unwrap();
        assert_eq!(edited.id, saved.id);
        assert_eq!(edited.name, "官网");
        assert_eq!(edited.created_at, saved.created_at);
        assert_eq!(store.load_config().unwrap().pages_configs.len(), 1);

        // 明确携带不存在的 id 拒绝；未列出的平台也拒绝。
        let mut unknown = saved.clone();
        unknown.id = "no-such-id".to_string();
        assert!(Store::save_pages_config(&store, unknown).is_err());
        let mut bad_provider = saved.clone();
        bad_provider.id = String::new();
        bad_provider.name = "其它平台".to_string();
        bad_provider.config.provider = "vercel".to_string();
        assert!(Store::save_pages_config(&store, bad_provider).is_err());
        // 仓库不存在时不会留下半条配置。
        let mut no_repo = saved.clone();
        no_repo.id = String::new();
        no_repo.repo_id = "missing".to_string();
        assert!(Store::save_pages_config(&store, no_repo).is_err());
        assert_eq!(store.load_config().unwrap().pages_configs.len(), 1);

        // 删除按 id 命中，并清掉指向它的默认配置引用。
        store
            .mutate_config(|app| {
                app.repos[0].default_pages_config_id = Some(saved.id.clone());
                Ok(())
            })
            .unwrap();
        assert!(store
            .mutate_config(|app| Store::delete_pages_config(app, &saved.id))
            .unwrap());
        let after = store.load_config().unwrap();
        assert!(after.pages_configs.is_empty());
        assert_eq!(after.repos[0].default_pages_config_id, None);

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

    fn history_record(id: &str, status: DeployStatus) -> DeployRecord {
        let status = serde_json::to_string(&status).unwrap();
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","repoId":"repo","repoName":"demo","rev":"main","branch":"main",
            "commit":"abc","commitShort":"abc","commitSubject":"init","serverId":"srv",
            "serverName":"prod","targetDir":"/srv/app","scriptDir":"docker","scripts":[],
            "runScripts":false,"envFiles":[],"status":{status},"error":null,"log":"",
            "startedAt":"2026-01-01 00:00:00","finishedAt":null,"durationMs":0}}"#
        ))
        .unwrap()
    }

    fn backup_record(id: &str, status: DeployStatus) -> BackupRecord {
        BackupRecord {
            id: id.to_string(),
            server_id: "srv".to_string(),
            server_name: "prod".to_string(),
            database: "app".to_string(),
            schema: "public".to_string(),
            target_name: String::new(),
            target: "postgresql://u@h/db".to_string(),
            status,
            error: None,
            log: String::new(),
            dump_size: 0,
            started_at: "2026-01-01 00:00:00".to_string(),
            finished_at: None,
            duration_ms: 0,
        }
    }

    fn pages_record(id: &str, status: DeployStatus) -> PagesDeployRecord {
        PagesDeployRecord {
            id: id.to_string(),
            provider: "cloudflare".to_string(),
            repo_id: "repo".to_string(),
            repo_name: "demo".to_string(),
            project_name: "demo".to_string(),
            branch: "main".to_string(),
            commit: "abc".to_string(),
            commit_short: "abc".to_string(),
            status,
            error: None,
            log: String::new(),
            url: None,
            started_at: "2026-01-01 00:00:00".to_string(),
            finished_at: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn record_crud_keeps_upsert_limit_and_prefix_semantics() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-store-records-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);

        // upsert 按 id 更新而不是追加，limit 只裁剪最早的多余记录。
        store
            .upsert_history(&history_record("h1", DeployStatus::Success), 2)
            .unwrap();
        store
            .upsert_history(&history_record("h2", DeployStatus::Success), 2)
            .unwrap();
        store
            .upsert_history(&history_record("h1", DeployStatus::Failed), 2)
            .unwrap();
        let history = store.load_history().unwrap();
        assert_eq!(history.len(), 2);
        // 更新是原位替换，不改变记录顺序。
        assert_eq!(history[0].id, "h1");
        assert_eq!(history[1].id, "h2");
        assert_eq!(history[0].status, DeployStatus::Failed);
        store
            .upsert_history(&history_record("h3", DeployStatus::Success), 2)
            .unwrap();
        let ids: Vec<String> = store
            .load_history()
            .unwrap()
            .iter()
            .map(|record| record.id.clone())
            .collect();
        assert_eq!(ids, vec!["h2".to_string(), "h3".to_string()]);

        // limit 为 0 表示不裁剪（CLI / 迁移场景）。
        store
            .upsert_history(&history_record("h4", DeployStatus::Success), 0)
            .unwrap();
        assert_eq!(store.load_history().unwrap().len(), 3);

        assert!(store.remove_history("h2").unwrap());
        assert!(!store.remove_history("h2").unwrap());
        store.clear_history().unwrap();
        assert!(store.load_history().unwrap().is_empty());

        // 查找：精确命中优先，唯一前缀可命中，多义前缀报错。
        store
            .upsert_backup(&backup_record("aaaa-1111", DeployStatus::Success), 0)
            .unwrap();
        store
            .upsert_backup(&backup_record("bbbb-2222", DeployStatus::Success), 0)
            .unwrap();
        assert_eq!(store.find_backup("aaaa-1111").unwrap().id, "aaaa-1111");
        assert_eq!(store.find_backup("aaaa").unwrap().id, "aaaa-1111");
        assert!(matches!(
            store.find_backup("nope"),
            Err(CoreError::NotFound(_))
        ));
        store
            .upsert_backup(&backup_record("aaaa-3333", DeployStatus::Success), 0)
            .unwrap();
        assert!(matches!(
            store.find_backup("aaaa"),
            Err(CoreError::Config(_))
        ));
        assert_eq!(store.find_backup("aaaa-3333").unwrap().id, "aaaa-3333");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_interrupted_converts_running_records_across_kinds() {
        let dir = std::env::temp_dir()
            .join(format!("deploycode-store-interrupted-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        store
            .upsert_history(&history_record("h1", DeployStatus::Running), 0)
            .unwrap();
        store
            .upsert_history(&history_record("h2", DeployStatus::Success), 0)
            .unwrap();
        store
            .upsert_backup(&backup_record("b1", DeployStatus::Running), 0)
            .unwrap();
        store
            .upsert_pages_record(&pages_record("p1", DeployStatus::Running), 0)
            .unwrap();

        assert_eq!(store.mark_interrupted().unwrap(), 3);
        let converted = store.find_record("h1").unwrap();
        assert_eq!(converted.status, DeployStatus::Failed);
        assert_eq!(
            converted.error.as_deref(),
            Some("任务被中断（应用退出或崩溃）")
        );
        assert!(converted.finished_at.is_some());
        // 崩溃后无法得知真实结束时间，耗时保持 0（界面显示为未知）。
        assert_eq!(converted.duration_ms, 0);
        assert_eq!(
            store.find_record("h2").unwrap().status,
            DeployStatus::Success
        );
        assert_eq!(
            store.find_backup("b1").unwrap().status,
            DeployStatus::Failed
        );
        assert_eq!(
            store.find_pages_record("p1").unwrap().status,
            DeployStatus::Failed
        );
        // 收敛过之后再次调用不应产生变更。
        assert_eq!(store.mark_interrupted().unwrap(), 0);

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
