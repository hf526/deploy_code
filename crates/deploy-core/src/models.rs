use serde::{Deserialize, Serialize};

/// 一次部署的结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeployStatus {
    Running,
    Success,
    Failed,
}

impl DeployStatus {
    pub fn is_failure(self) -> bool {
        matches!(self, DeployStatus::Failed)
    }
}

/// SSH 认证方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SshAuth {
    Password { password: String },
    PrivateKey {
        // alias 兼容 CLI/旧版本写出的 key_path 字段。
        #[serde(alias = "key_path")]
        key_path: String,
        #[serde(default)]
        passphrase: Option<String>,
    },
}

/// 部署服务器配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    /// 该服务器的默认部署目录，创建部署任务时自动填充。
    #[serde(default)]
    pub default_target_dir: String,
    #[serde(default)]
    pub created_at: String,
}

fn default_ssh_port() -> u16 {
    22
}

impl ServerConfig {
    pub fn new(name: String, host: String, username: String, auth: SshAuth) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            host,
            port: default_ssh_port(),
            username,
            auth,
            default_target_dir: String::new(),
            created_at: crate::models::now_string(),
        }
    }
}

/// 本地仓库配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoConfig {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub default_server_id: Option<String>,
    #[serde(default)]
    pub default_target_dir: String,
    #[serde(default)]
    pub added_at: String,
}

impl RepoConfig {
    pub fn new(name: String, path: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            path,
            default_server_id: None,
            default_target_dir: String::new(),
            added_at: now_string(),
        }
    }
}

/// 仓库运行时信息（配置 + 当前 Git 状态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_repo: bool,
    pub current_branch: String,
    pub remote: Option<String>,
    pub change_count: usize,
    pub default_server_id: Option<String>,
    pub default_target_dir: String,
}

/// 分支信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub last_commit: String,
    pub last_commit_subject: String,
    pub last_commit_date: String,
}

/// 提交记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// 提交图中的引用标记（本地分支 / 远程分支 / 标签）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRef {
    pub name: String,
    /// "local" | "remote" | "tag"
    pub kind: String,
    pub is_head: bool,
}

/// 提交图中的一条提交（含父提交，用于前端计算车道布局）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub refs: Vec<GraphRef>,
}

/// 内容搜索的一条命中结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// 批量替换的结果摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceSummary {
    pub files_replaced: u32,
    pub matches_replaced: u32,
}

/// 资源管理器目录条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// 相对仓库根目录、使用正斜杠的路径。
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// 文件预览内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

/// 工作区变动文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub code: String,
    pub status: String,
    pub path: String,
}

/// 仓库工作区状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
}

/// 解析后的版本号。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRev {
    pub rev: String,
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
}

/// 一条部署记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployRecord {
    pub id: String,
    pub repo_id: String,
    pub repo_name: String,
    /// 部署时指定的版本（分支名 / 提交号 / 标签）。
    pub rev: String,
    pub branch: String,
    pub commit: String,
    pub commit_short: String,
    pub commit_subject: String,
    pub server_id: String,
    pub server_name: String,
    pub target_dir: String,
    pub script_dir: String,
    pub script: Option<String>,
    pub run_scripts: bool,
    pub status: DeployStatus,
    pub error: Option<String>,
    pub log: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: u64,
}

/// 发起一次部署所需的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployRequest {
    pub repo_id: String,
    pub rev: String,
    pub server_id: String,
    pub target_dir: String,
    #[serde(default = "default_true")]
    pub run_scripts: bool,
    #[serde(default = "default_script_dir")]
    pub script_dir: String,
    #[serde(default)]
    pub script: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_script_dir() -> String {
    "docker".to_string()
}

/// 部署过程中推送的事件（GUI 通过 Tauri event 转发，CLI 直接打印）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DeployEvent {
    /// 正在处理哪个部署记录。
    Started { record_id: String },
    /// 一行日志。
    Log { level: LogLevel, message: String },
    /// 上传进度。
    Progress { percent: u8, message: String },
    /// 部署结束（成功或失败）。
    Finished { record: DeployRecord },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Command,
    Success,
    Warn,
    Error,
}

/// 全局设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// 部署后自动执行的脚本目录（相对项目根目录）。
    pub script_dir: String,
    /// 执行脚本的默认开关。
    pub run_scripts: bool,
    /// SSH 连接超时（秒）。
    pub connect_timeout_secs: u64,
    /// 单个脚本允许的最长执行时间（秒）。
    pub script_timeout_secs: u64,
    /// 是否在服务器上保留上传的压缩包。
    pub keep_remote_archive: bool,
    /// 部署记录保留条数。
    pub history_limit: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            script_dir: "docker".to_string(),
            run_scripts: true,
            connect_timeout_secs: 15,
            script_timeout_secs: 1800,
            keep_remote_archive: false,
            history_limit: 500,
        }
    }
}

/// 应用配置（服务器 + 仓库 + 设置）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub settings: Settings,
}

pub fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 生成一个新的唯一 ID。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_auth_private_key_serializes_camel_case() {
        let auth = SshAuth::PrivateKey {
            key_path: "/home/u/.ssh/id_ed25519".to_string(),
            passphrase: None,
        };
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("\"keyPath\""), "unexpected json: {json}");
        assert!(!json.contains("key_path"), "unexpected json: {json}");

        // 兼容旧数据里的 key_path 字段名。
        let parsed: SshAuth =
            serde_json::from_str(r#"{"type":"privateKey","key_path":"/k","passphrase":null}"#)
                .unwrap();
        match parsed {
            SshAuth::PrivateKey { key_path, .. } => assert_eq!(key_path, "/k"),
            _ => panic!("expected private key auth"),
        }
    }

    #[test]
    fn deploy_event_started_serializes_record_id_camel_case() {
        let event = DeployEvent::Started {
            record_id: "abc".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"recordId\""), "unexpected json: {json}");
    }
}
