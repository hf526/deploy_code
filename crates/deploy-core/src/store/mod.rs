//! 配置与任务记录的本地存储（JSON 文件）。
//!
//! 文件分工：本文件是 `Store` 本体（构造、路径、锁原语、config.json 读写、主密码、
//! agent 同步指纹持久化、调度状态、原子写盘）；`records` 是四类任务记录的 CRUD 与
//! 孤儿备份包；`import_export` 是导出 / 导入的合并语义；`configs` 是部署 / 容器 / Pages
//! 配置的校验保存；`backup_configs` 是备份配置 / 目标 / 服务器查找与旧版迁移。

mod backup_configs;
mod configs;
mod import_export;
mod records;
#[cfg(test)]
mod testutil;

pub use records::OrphanBundle;
pub use backup_configs::paths_equal;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use directories::ProjectDirs;

use crate::agent::AgentSyncState;
use crate::crypto::{decrypt_string, encrypt_string};
use crate::error::{CoreError, Result};
use crate::schedule::ScheduleState;
use crate::models::{AppConfig, RunLocation, SshAuth};

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
    /// 进程身份：控制机上的 agent 启动时置真。见 [`Store::set_agent_mode`]。
    agent_mode: AtomicBool,
    /// 安装包内置的 agent 可执行文件。见 [`Store::set_bundled_agent_binary`]。
    bundled_agent_binary: OnceLock<PathBuf>,
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
            agent_mode: AtomicBool::new(false),
            bundled_agent_binary: OnceLock::new(),
        }
    }

    /// 声明「这个进程是控制机上的 agent」：磁盘水位闸只在这种进程上拦任务。
    ///
    /// 刻意不是配置字段：它是进程身份，跟着二进制走。写在 `Settings` 里的话，
    /// 客户端一次「保存设置」的整体回写、或导入别人那份配置，都会把笔电也变成
    /// 「剩余不足 max(总量 10%, 5GB) 就拒绝备份」——升级后原本能跑的定时备份
    /// 会直接启动即失败，而这道闸的设计对象只有控制机那块攒了所有备份包的盘。
    pub fn set_agent_mode(&self, enabled: bool) {
        self.agent_mode.store(enabled, Ordering::Relaxed);
    }

    pub fn agent_mode(&self) -> bool {
        self.agent_mode.load(Ordering::Relaxed)
    }

    /// 登记安装包内置的 agent 可执行文件（`<资源目录>/agent/deploy-agent`）。
    ///
    /// 与 `agent_mode` 同理，这也是进程/安装属性而不是配置字段：资源目录跟着这台机器上的
    /// 安装位置走，写进 config.json 就会被导出/导入带走，换台机器指到不存在的路径。
    /// 只有 Tauri 壳问得到资源目录（`deploy-core` 不许依赖 tauri），所以由 `lib.rs` 启动时登记；
    /// CLI 与单测不登记，`agent::locate_binary` 自然跳过这一档。
    pub fn set_bundled_agent_binary(&self, path: impl Into<PathBuf>) {
        let _ = self.bundled_agent_binary.set(path.into());
    }

    pub fn bundled_agent_binary(&self) -> Option<&Path> {
        self.bundled_agent_binary.get().map(PathBuf::as_path)
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

    /// 数据库导出包的本机存放目录（`<数据目录>/backups`）。
    ///
    /// 与容器包分目录：一个可能是几十 GB 的 tar，另一个是几百 MB 的 sql.gz，
    /// 混在一起既看不清各自占了多少盘，轮转也不好按目录整体核对。
    pub fn db_bundle_dir(&self) -> PathBuf {
        self.base_dir.join("backups")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.base_dir.join("temp")
    }

    /// 调度器触发日期所在的文件（控制机上的 agent 用，见 [`crate::schedule::ScheduleState`]）。
    pub fn schedule_state_path(&self) -> PathBuf {
        self.base_dir.join("schedule-state.json")
    }

    /// 读调度状态；文件不存在或写坏了都按「从未触发」处理，不能让调度循环起不来。
    pub fn load_schedule_state(&self) -> ScheduleState {
        let path = self.schedule_state_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save_schedule_state(&self, state: &ScheduleState) -> Result<()> {
        let _guard = self.write_guard()?;
        atomic_write(
            &self.schedule_state_path(),
            &serde_json::to_string_pretty(state)?,
        )
    }

    /// 上次成功下发给控制机的配置指纹所在的文件。
    pub fn agent_sync_path(&self) -> PathBuf {
        self.base_dir.join("agent-sync.json")
    }

    /// 读同步指纹；文件不存在或写坏了都按「从未同步」处理。
    pub fn load_agent_sync(&self) -> AgentSyncState {
        let path = self.agent_sync_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 记同步指纹。单独一份文件（而不是塞进 `settings`）的理由与 [`Store::schedule_state_path`]
    /// 同：`save_settings` 是前端整体回写 `Settings` 的，漏带这个字段就会把指纹冲没，
    /// 而指纹是定时循环判断「这条该不该让给控制机」的唯一依据。
    pub fn save_agent_sync(&self, state: &AgentSyncState) -> Result<()> {
        let _guard = self.write_guard()?;
        atomic_write(
            &self.agent_sync_path(),
            &serde_json::to_string_pretty(state)?,
        )
    }

    /// 还有哪些台机器「已经不是控制机、但上面可能还装着 agent」需要收回。
    pub fn orphan_agent_servers(&self) -> Vec<String> {
        self.load_agent_sync().orphan_server_ids
    }

    /// 记一台收回失败的旧控制机。它还在自行执行定时，盘上也还留着全部源机的明文口令，
    /// 这笔待办要一直挂在界面上，直到 [`Store::forget_orphan_agent`] 把它撤掉。
    pub fn remember_orphan_agent(&self, server_id: &str) -> Result<()> {
        let mut state = self.load_agent_sync();
        if state.orphan_server_ids.iter().any(|item| item == server_id) {
            return Ok(());
        }
        state.add_orphan(server_id);
        self.save_agent_sync(&state)
    }

    /// 撤掉一笔待收回（那台的 agent 真的卸掉了，或这台服务器被删了）。
    pub fn forget_orphan_agent(&self, server_id: &str) -> Result<()> {
        let mut state = self.load_agent_sync();
        if !state.orphan_server_ids.iter().any(|item| item == server_id) {
            return Ok(());
        }
        state.remove_orphan(server_id);
        self.save_agent_sync(&state)
    }

    /// 收回指向某台控制机的本机状态：执行位退回本机、指向清空、同步指纹作废；返回改回本机的条数。
    ///
    /// 「卸载 agent」与「删除这台服务器」共用这一条。两条判据都要有：
    /// ① 动的不是当前控制机时一概不动 —— 顺手清掉另一台上的旧 agent，不该掐死现役那条的定时；
    /// ② 指针对得上却不收回执行位，留下的就是「配置写着控制机、控制机已经不在了」的死局：
    ///    定时循环照旧让位、本机又撒手，那一晚两头都不跑。
    pub fn forget_agent_server(&self, server_id: &str) -> Result<usize> {
        let mut moved = 0usize;
        let mut mine = false;
        self.mutate_config(|config| {
            mine = config.settings.agent_server_id.trim() == server_id;
            if !mine {
                return Ok(());
            }
            for item in &mut config.backup_configs {
                if item.run_location.is_remote() {
                    item.run_location = RunLocation::Local;
                    moved += 1;
                }
            }
            for item in &mut config.container_configs {
                if item.run_location.is_remote() {
                    item.run_location = RunLocation::Local;
                    moved += 1;
                }
            }
            config.settings.agent_server_id = String::new();
            Ok(())
        })?;
        if mine {
            let mut state = self.load_agent_sync();
            state.clear();
            state.remove_orphan(server_id);
            self.save_agent_sync(&state)?;
        } else if self
            .load_agent_sync()
            .orphan_server_ids
            .iter()
            .any(|item| item == server_id)
        {
            // 待收回那台被卸载成功、或整台服务器被删掉了：这条待办到此为止，
            // 留着只会指着一个不存在（或已经干净）的目标一直红着。
            self.forget_orphan_agent(server_id)?;
        }
        Ok(moved)
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
}

fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
    let text = serde_json::to_string_pretty(config)?;
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
