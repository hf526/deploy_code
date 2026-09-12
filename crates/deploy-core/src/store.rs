use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use directories::ProjectDirs;

use crate::error::{CoreError, Result};
use crate::models::{AppConfig, DeployRecord, RepoConfig, ServerConfig};

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
        let _guard = self.lock();
        write_config(&self.config_path(), config)
    }

    /// 加锁执行「读取-修改-写回」，避免并发命令互相覆盖配置。
    pub fn mutate_config<T, F>(&self, mutate: F) -> Result<T>
    where
        F: FnOnce(&mut AppConfig) -> Result<T>,
    {
        let _guard = self.lock();
        let mut config = self.load_config()?;
        let output = mutate(&mut config)?;
        write_config(&self.config_path(), &config)?;
        Ok(output)
    }

    pub fn load_history(&self) -> Result<Vec<DeployRecord>> {
        load_history(&self.history_path())
    }

    pub fn save_history(&self, records: &[DeployRecord]) -> Result<()> {
        let _guard = self.lock();
        write_history(&self.history_path(), records)
    }

    /// 新增或更新一条部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_history(&self, record: &DeployRecord, limit: usize) -> Result<()> {
        let _guard = self.lock();
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
        let _guard = self.lock();
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
        let _guard = self.lock();
        write_history(&self.history_path(), &[])
    }

    pub fn find_record(&self, id: &str) -> Result<DeployRecord> {
        self.load_history()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::not_found(format!("部署记录不存在: {id}")))
    }

    /// 按 id / 名称 / 路径查找仓库。
    pub fn find_repo<'a>(config: &'a AppConfig, key: &str) -> Result<&'a RepoConfig> {
        config
            .repos
            .iter()
            .find(|r| r.id == key || r.name == key || paths_equal(&r.path, key))
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {key}")))
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
