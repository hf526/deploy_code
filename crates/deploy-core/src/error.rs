use std::path::PathBuf;

/// 核心库统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("Git 操作失败: {0}")]
    Git(String),

    #[error("SSH 操作失败: {0}")]
    Ssh(String),

    #[error("命令执行失败: {0}")]
    Process(String),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("未找到: {0}")]
    NotFound(String),

    /// 有互斥任务正在进行（部署 / 备份 / Pages 抢占失败）。
    /// 调用方（如定时调度器）依赖该类型区分「稍后重试」与真实失败，不要只用字符串匹配。
    #[error("任务冲突: {0}")]
    Busy(String),

    #[error("部署失败: {0}")]
    Deploy(String),

    #[error("备份失败: {0}")]
    Backup(String),

    #[error("Pages 部署失败: {0}")]
    Pages(String),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serde(#[from] serde_json::Error),
}

impl CoreError {
    pub fn git(msg: impl Into<String>) -> Self {
        Self::Git(msg.into())
    }

    pub fn ssh(msg: impl Into<String>) -> Self {
        Self::Ssh(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn busy(msg: impl Into<String>) -> Self {
        Self::Busy(msg.into())
    }

    pub fn deploy(msg: impl Into<String>) -> Self {
        Self::Deploy(msg.into())
    }

    pub fn backup(msg: impl Into<String>) -> Self {
        Self::Backup(msg.into())
    }

    pub fn pages(msg: impl Into<String>) -> Self {
        Self::Pages(msg.into())
    }

    pub fn io_path(path: impl Into<PathBuf>, err: std::io::Error) -> Self {
        let path = path.into();
        Self::Io(std::io::Error::new(
            err.kind(),
            format!("{}: {}", path.display(), err),
        ))
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// 让 GUI 的 Tauri 命令可以直接返回 `Result<T, CoreError>`，
/// 错误会以字符串形式传给前端。
impl serde::Serialize for CoreError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
