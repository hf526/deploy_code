//! 配置与任务记录的本地存储（JSON 文件）。
//!
//! 文件分工：本文件是 `Store` 本体（构造、路径、锁原语、config.json 读写、
//! 原子写盘）；`records` 是四类任务记录的 CRUD 与孤儿备份包；`import_export` 是导出 /
//! 导入的合并语义；`configs` 是部署 / 容器 / Pages 配置的校验保存；`backup_configs`
//! 是备份配置 / 目标 / 服务器查找与旧版迁移。
//!
//! ⚠️ **凭据在本文件中一律按明文读写**（SSH 密码 / 私钥口令 / 备份目标连接串 / 各平台
//! Token）：这是 README「数据存储」一节记录的已知取舍，不是遗漏。加密会让已有的
//! config.json 全部读不出来，所以不要「顺手」在这里加解密；确有必要时按新版本 +
//! 迁移路径单独设计。

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
use std::sync::{Mutex, MutexGuard};

use directories::ProjectDirs;

use crate::error::{CoreError, Result};
use crate::models::AppConfig;

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

        serde_json::from_str(&text)
            .map_err(|e| CoreError::config(format!("配置文件解析失败 {}: {e}", path.display())))
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
