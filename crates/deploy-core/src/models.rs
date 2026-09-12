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

/// 服务器上的 PostgreSQL 备份来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbBackupSource {
    /// 采集方式："docker"（默认，在容器内执行 pg_dump）或 "system"（服务器上直接执行 pg_dump）。
    #[serde(default = "default_db_mode")]
    pub mode: String,
    /// mode = docker 时对应的容器名。
    #[serde(default)]
    pub container: String,
    /// 数据库名。
    pub database: String,
    /// 数据库用户名。
    pub username: String,
    /// 数据库密码；为空时依赖容器/服务器的本地认证。
    #[serde(default)]
    pub password: String,
    /// 需要同步的 schema。
    #[serde(default = "default_backup_schema")]
    pub schema: String,
}

impl Default for DbBackupSource {
    fn default() -> Self {
        Self {
            mode: default_db_mode(),
            container: String::new(),
            database: String::new(),
            username: String::new(),
            password: String::new(),
            schema: default_backup_schema(),
        }
    }
}

fn default_db_mode() -> String {
    "docker".to_string()
}

fn default_backup_schema() -> String {
    "public".to_string()
}

/// 命名的数据库备份目标（Supabase / Aiven / Neon 等 PostgreSQL）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupTarget {
    pub id: String,
    pub name: String,
    /// postgres:// 或 postgresql:// 连接串。
    pub url: String,
}

impl BackupTarget {
    pub fn new(name: String, url: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            url,
        }
    }
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
    /// 该服务器的数据库备份来源。
    #[serde(default)]
    pub db_backup: Option<DbBackupSource>,
    /// 该服务器默认使用的备份目标 id（为空时使用全局默认）。
    #[serde(default)]
    pub backup_target_id: Option<String>,
    /// 该服务器专用的数据库连接串（旧字段，仍兼容；优先使用 backup_target_id）。
    #[serde(default)]
    pub supabase_url: Option<String>,
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
            db_backup: None,
            backup_target_id: None,
            supabase_url: None,
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
    /// Cloudflare Pages 部署配置。
    #[serde(default)]
    pub pages: Option<PagesConfig>,
    #[serde(default)]
    pub added_at: String,
}

/// Cloudflare Pages 部署配置（按仓库保存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesConfig {
    /// Cloudflare Pages 项目名。
    pub project_name: String,
    /// 构建命令（可为空，表示只上传已有产物）。
    #[serde(default)]
    pub build_command: String,
    /// 输出目录（相对仓库根目录）。
    #[serde(default = "default_pages_output")]
    pub output_dir: String,
    /// 生产分支名。
    #[serde(default = "default_pages_branch")]
    pub branch: String,
}

impl Default for PagesConfig {
    fn default() -> Self {
        Self {
            project_name: String::new(),
            build_command: String::new(),
            output_dir: default_pages_output(),
            branch: default_pages_branch(),
        }
    }
}

fn default_pages_output() -> String {
    "dist".to_string()
}

fn default_pages_branch() -> String {
    "main".to_string()
}

impl RepoConfig {
    pub fn new(name: String, path: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            path,
            default_server_id: None,
            default_target_dir: String::new(),
            pages: None,
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

/// 发起一次数据库备份所需的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRequest {
    /// 服务器 id / 名称 / host。
    pub server_id: String,
    /// 指定的备份目标 id / 名称（为空时使用服务器或全局默认）。
    #[serde(default)]
    pub target_id: Option<String>,
    /// 直接覆盖目标连接串（优先级最高）。
    #[serde(default)]
    pub supabase_url: Option<String>,
    /// 覆盖数据库名。
    #[serde(default)]
    pub database: Option<String>,
    /// 覆盖 schema。
    #[serde(default)]
    pub schema: Option<String>,
}

/// 一条数据库备份记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    /// 来源数据库名。
    pub database: String,
    /// 同步的 schema。
    pub schema: String,
    /// 目标连接串（已隐藏密码），仅用于展示。
    #[serde(default)]
    pub target_name: String,
    pub target: String,
    pub status: DeployStatus,
    pub error: Option<String>,
    pub log: String,
    /// 导出的压缩文件大小（字节）。
    pub dump_size: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: u64,
}

/// 备份过程中推送的事件（GUI 通过 Tauri event 转发，CLI 直接打印）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BackupEvent {
    /// 正在处理哪条备份记录。
    Started { record_id: String },
    /// 一行日志。
    Log { level: LogLevel, message: String },
    /// 备份进度。
    Progress { percent: u8, message: String },
    /// 备份结束（成功或失败）。
    Finished { record: BackupRecord },
}

/// 发起一次 Cloudflare Pages 部署所需的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesRequest {
    /// 仓库 id / 名称 / 路径。
    pub repo_id: String,
    /// 覆盖项目名。
    #[serde(default)]
    pub project_name: Option<String>,
    /// 覆盖构建命令。
    #[serde(default)]
    pub build_command: Option<String>,
    /// 覆盖输出目录。
    #[serde(default)]
    pub output_dir: Option<String>,
    /// 覆盖分支。
    #[serde(default)]
    pub branch: Option<String>,
    /// 跳过构建，直接上传现有产物。
    #[serde(default)]
    pub skip_build: bool,
}

/// 一条 Cloudflare Pages 部署记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesDeployRecord {
    pub id: String,
    pub repo_id: String,
    pub repo_name: String,
    pub project_name: String,
    pub branch: String,
    pub commit: String,
    pub commit_short: String,
    pub status: DeployStatus,
    pub error: Option<String>,
    pub log: String,
    /// 部署完成后解析出的访问地址。
    #[serde(default)]
    pub url: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: u64,
}

/// Pages 部署过程中推送的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PagesEvent {
    Started { record_id: String },
    Log { level: LogLevel, message: String },
    Finished { record: PagesDeployRecord },
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
    /// 全局 Supabase PostgreSQL 连接串（旧字段，作为最低优先级兜底）。
    #[serde(default)]
    pub supabase_url: String,
    /// 全局默认备份目标 id。
    #[serde(default)]
    pub default_backup_target_id: Option<String>,
    /// 数据库备份记录保留条数。
    #[serde(default = "default_backup_history_limit")]
    pub backup_history_limit: usize,
    /// 单次数据库备份超时（秒）。
    #[serde(default = "default_backup_timeout_secs")]
    pub backup_timeout_secs: u64,
    /// 全局 Cloudflare API Token（Pages 部署）。
    #[serde(default)]
    pub cloudflare_api_token: String,
    /// 全局 Cloudflare Account ID。
    #[serde(default)]
    pub cloudflare_account_id: String,
    /// Pages 部署记录保留条数。
    #[serde(default = "default_pages_history_limit")]
    pub pages_history_limit: usize,
}

fn default_backup_history_limit() -> usize {
    200
}

fn default_backup_timeout_secs() -> u64 {
    3600
}

fn default_pages_history_limit() -> usize {
    200
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
            supabase_url: String::new(),
            default_backup_target_id: None,
            backup_history_limit: default_backup_history_limit(),
            backup_timeout_secs: default_backup_timeout_secs(),
            cloudflare_api_token: String::new(),
            cloudflare_account_id: String::new(),
            pages_history_limit: default_pages_history_limit(),
        }
    }
}

/// 应用配置（服务器 + 仓库 + 备份目标 + 设置）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub backup_targets: Vec<BackupTarget>,
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
