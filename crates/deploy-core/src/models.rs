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
    /// 数据库密码；为空时依赖容器/服务器的本地认证。（存储时为密文）
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

/// 保存的数据库备份配置（名称 + 服务器 + 来源 + 目标）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    pub id: String,
    pub name: String,
    /// 备份来源所在的服务器 id。
    pub server_id: String,
    /// 数据库备份来源（docker 容器或服务器本机）。
    pub source: DbBackupSource,
    /// 备份目标 id（为空时回退到服务器绑定 / 全局默认目标）。
    #[serde(default)]
    pub target_id: Option<String>,
    /// 该配置直接指定的目标连接串（优先级高于 target_id）。
    #[serde(default)]
    pub supabase_url: Option<String>,
}

impl BackupConfig {
    pub fn new(name: String, server_id: String, source: DbBackupSource) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            server_id,
            source,
            target_id: None,
            supabase_url: None,
        }
    }
}

/// 保存的服务器部署配置（名称 + 仓库 + 服务器 + 目录 + 脚本）。
/// 各字段都允许缺省：单条坏数据不应导致整份 config.json 解析失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 要部署的仓库 id。
    #[serde(default)]
    pub repo_id: String,
    /// 目标服务器 id。
    #[serde(default)]
    pub server_id: String,
    /// 服务器上的绝对部署目录。
    #[serde(default)]
    pub target_dir: String,
    /// 部署版本（分支 / 标签 / 提交）；为空表示打包当前工作区。
    #[serde(default)]
    pub rev: String,
    /// 是否执行项目脚本。
    #[serde(default = "default_true")]
    pub run_scripts: bool,
    /// 脚本目录（相对项目根目录）。
    #[serde(default = "default_script_dir")]
    pub script_dir: String,
    /// 指定执行的脚本列表（按顺序执行）；为空表示自动执行脚本目录下的全部 .sh。
    #[serde(default)]
    pub scripts: Vec<String>,
    /// 是否上传并替换仓库配置的环境文件。
    #[serde(default = "default_true")]
    pub upload_env: bool,
    #[serde(default)]
    pub created_at: String,
}

/// 列表展示用的 Pages 配置条目（独立管理，可复用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesConfigEntry {
    pub id: String,
    pub name: String,
    pub repo_id: String,
    pub repo_name: String,
    pub config: PagesConfig,
    pub created_at: String,
}

impl PagesConfigEntry {
    pub fn new(repo_id: String, repo_name: String, config: PagesConfig) -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            repo_id,
            repo_name,
            config,
            created_at: now_string(),
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

/// 部署时要上传覆盖的环境文件（本地文件 → 部署目录下的相对路径）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EnvFileConfig {
    /// 本地文件路径。
    #[serde(default)]
    pub local_path: String,
    /// 远端相对部署目录的路径，如 `.env` 或 `docker/.env`。
    #[serde(default)]
    pub remote_path: String,
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
    /// [DEPRECATED] 已废弃，改用 pages_configs 列表管理。保留此字段用于向后兼容。
    #[serde(default, skip_serializing)]
    pub pages: Option<PagesConfig>,
    /// 默认 Pages 配置 ID（指向 pages_configs 列表中的某一项，为空表示无默认配置）。
    #[serde(default)]
    pub default_pages_config_id: Option<String>,
    /// 部署时上传覆盖的环境文件列表。
    #[serde(default)]
    pub env_files: Vec<EnvFileConfig>,
    #[serde(default)]
    pub added_at: String,
}

/// Pages 部署配置（按仓库保存，支持 Cloudflare / GitHub 两种平台）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesConfig {
    /// 部署平台：cloudflare | github
    #[serde(default = "default_pages_provider")]
    pub provider: String,
    /// Cloudflare Pages 项目名（GitHub 平台可留空）。
    #[serde(default)]
    pub project_name: String,
    /// 构建命令（可为空，表示只上传已有产物）。
    #[serde(default)]
    pub build_command: String,
    /// 输出目录（相对仓库根目录）。
    #[serde(default = "default_pages_output")]
    pub output_dir: String,
    /// Cloudflare 生产分支名。
    #[serde(default = "default_pages_branch")]
    pub branch: String,
    /// GitHub Pages 发布分支名（推送构建产物的分支）。
    #[serde(default = "default_pages_publish_branch")]
    pub publish_branch: String,
}

impl Default for PagesConfig {
    fn default() -> Self {
        Self {
            provider: default_pages_provider(),
            project_name: String::new(),
            build_command: String::new(),
            output_dir: default_pages_output(),
            branch: default_pages_branch(),
            publish_branch: default_pages_publish_branch(),
        }
    }
}

fn default_pages_provider() -> String {
    "cloudflare".to_string()
}

fn default_pages_output() -> String {
    "dist".to_string()
}

fn default_pages_branch() -> String {
    "main".to_string()
}

fn default_pages_publish_branch() -> String {
    "gh-pages".to_string()
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
            default_pages_config_id: None,
            env_files: Vec::new(),
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
    /// 本地目录是否存在。
    pub path_exists: bool,
    pub is_repo: bool,
    pub current_branch: String,
    pub remote: Option<String>,
    pub change_count: usize,
    pub default_server_id: Option<String>,
    pub default_target_dir: String,
    /// 部署时上传覆盖的环境文件列表。
    #[serde(default)]
    pub env_files: Vec<EnvFileConfig>,
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
    /// 是否为「当前工作区」部署：未指定版本时打包本地工作区（含未提交改动）。
    #[serde(default)]
    pub worktree: bool,
    /// 是否使用 releases + current 软链的原子发布方式。
    #[serde(default)]
    pub atomic_release: bool,
    /// 原子发布成功切换后记录的版本目录名（releases/ 下的名称）。
    #[serde(default)]
    pub release_dir: Option<String>,
    pub commit: String,
    pub commit_short: String,
    pub commit_subject: String,
    pub server_id: String,
    pub server_name: String,
    pub target_dir: String,
    pub script_dir: String,
    /// 指定执行的脚本列表（按顺序执行）；为空表示自动执行脚本目录下的全部 .sh。
    #[serde(default, alias = "script", deserialize_with = "deserialize_script_list")]
    pub scripts: Vec<String>,
    pub run_scripts: bool,
    /// 本次部署实际替换的环境文件列表。
    #[serde(default)]
    pub env_files: Vec<EnvFileConfig>,
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
    /// 指定执行的脚本列表（按顺序执行）；为空表示自动执行脚本目录下的全部 .sh。
    #[serde(default, alias = "script", deserialize_with = "deserialize_script_list")]
    pub scripts: Vec<String>,
    /// 是否在解压后、执行脚本前上传并替换 `env_files` 中配置的环境文件。
    #[serde(default = "default_true")]
    pub upload_env: bool,
}

/// 兼容旧数据：`script` 字段可能是单个字符串，新字段 `scripts` 是数组。
fn deserialize_script_list<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<String>),
    }

    let raw = Option::<Raw>::deserialize(deserializer)?;
    Ok(match raw {
        None => Vec::new(),
        Some(Raw::One(value)) => vec![value],
        Some(Raw::Many(values)) => values,
    })
}

fn default_true() -> bool {
    true
}

fn default_script_dir() -> String {
    "docker".to_string()
}

/// 服务器上的一个历史发布版本（`releases/<name>`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRelease {
    /// 版本目录名（relative to `releases/`）。
    pub name: String,
    /// 是否为 `current` 软链指向的版本。
    pub current: bool,
    /// 目录修改时间（服务器本地时间）。
    pub modified: String,
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
    /// 已保存的备份配置 id / 名称（提供时服务器、来源与目标优先取自配置）。
    #[serde(default)]
    pub backup_config_id: Option<String>,
    /// 直接覆盖来源配置（用于测试尚未保存的表单）。
    #[serde(default)]
    pub source: Option<DbBackupSource>,
    /// 指定的备份目标 id / 名称（为空时使用配置、服务器或全局默认）。
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

/// 发起一次 Pages 部署所需的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesRequest {
    /// 仓库 id / 名称 / 路径。
    pub repo_id: String,
    /// 覆盖部署平台（cloudflare | github）。
    #[serde(default)]
    pub provider: Option<String>,
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
    /// 覆盖 GitHub Pages 发布分支。
    #[serde(default)]
    pub publish_branch: Option<String>,
    /// 跳过构建，直接上传现有产物。
    #[serde(default)]
    pub skip_build: bool,
}

/// 一条 Pages 部署记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesDeployRecord {
    pub id: String,
    /// 部署平台：cloudflare | github。
    #[serde(default = "default_pages_provider")]
    pub provider: String,
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
    /// 全局 Cloudflare API Token（Pages 部署）。（存储时为密文）
    #[serde(default)]
    pub cloudflare_api_token: String,
    /// 全局 Cloudflare Account ID。
    #[serde(default)]
    pub cloudflare_account_id: String,
    /// 主密码哈希（用于验证用户记忆的主密码是否正确）。
    #[serde(default)]
    pub master_password_hash: Option<String>,
    /// GitHub Personal Access Token（可选，用于自动配置 Pages；留空则尝试本机 gh CLI）。
    #[serde(default)]
    pub github_token: String,
    /// Pages 部署记录保留条数。
    #[serde(default = "default_pages_history_limit")]
    pub pages_history_limit: usize,
    /// 界面语言偏好（空字符串表示跟随系统）。
    #[serde(default)]
    pub language: String,
    /// 原子发布：部署到 `{部署目录}/releases/<版本>` 并切换 `current` 软链。
    #[serde(default)]
    pub atomic_release: bool,
    /// 原子发布保留的历史版本数（1-50，超出后清理最旧的，当前版本不删）。
    #[serde(default = "default_release_keep")]
    pub release_keep: usize,
    /// 定时备份开关：应用运行期间（含托盘后台）到点自动执行一次备份。
    #[serde(default)]
    pub scheduled_backup_enabled: bool,
    /// 定时备份时间（HH:MM，24 小时制，本机时区）。
    #[serde(default = "default_scheduled_backup_time")]
    pub scheduled_backup_time: String,
    /// 定时备份使用的备份配置 id（为空表示未选择，跳过执行）。
    #[serde(default)]
    pub scheduled_backup_config_id: Option<String>,
}

fn default_scheduled_backup_time() -> String {
    "03:00".to_string()
}

fn default_release_keep() -> usize {
    5
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
            master_password_hash: None,
            github_token: String::new(),
            pages_history_limit: default_pages_history_limit(),
            language: String::new(),
            atomic_release: false,
            release_keep: default_release_keep(),
            scheduled_backup_enabled: false,
            scheduled_backup_time: default_scheduled_backup_time(),
            scheduled_backup_config_id: None,
        }
    }
}

/// 应用配置（服务器 + 仓库 + 部署配置 + 备份目标 + 备份配置 + 设置）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub backup_targets: Vec<BackupTarget>,
    /// 保存的服务器部署配置列表。
    #[serde(default)]
    pub deploy_configs: Vec<DeployConfig>,
    /// 保存的数据库备份配置列表。
    #[serde(default)]
    pub backup_configs: Vec<BackupConfig>,
    /// 是否已把服务器上旧版的单份 db_backup 迁移为备份配置（只迁移一次）。
    #[serde(default)]
    pub backup_configs_migrated: bool,
    /// Pages 部署配置列表（独立管理，按仓库分组）。
    #[serde(default)]
    pub pages_configs: Vec<PagesConfigEntry>,
    /// 是否已将旧版 repo.pages 迁移到 pages_configs（只迁移一次）。
    #[serde(default)]
    pub pages_configs_migrated: bool,
    #[serde(default)]
    pub settings: Settings,
}

pub fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 从 `started_at`（`now_string` 格式）到现在的毫秒数；解析失败返回 0。
pub fn elapsed_ms_since(started_at: &str) -> u64 {
    chrono::NaiveDateTime::parse_from_str(started_at, "%Y-%m-%d %H:%M:%S")
        .map(|started| {
            let delta = chrono::Local::now().naive_local() - started;
            delta.num_milliseconds().max(0) as u64
        })
        .unwrap_or(0)
}

/// 生成一个新的唯一 ID。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 导出配置的数据结构（敏感字段已脱敏）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportData {
    pub version: String,
    pub exported_at: String,
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub backup_targets: Vec<BackupTarget>,
    #[serde(default)]
    pub deploy_configs: Vec<DeployConfig>,
    #[serde(default)]
    pub backup_configs: Vec<BackupConfig>,
    #[serde(default)]
    pub pages_configs: Vec<PagesConfigEntry>,
    #[serde(default)]
    pub settings: Settings,
}

/// 导入配置的统计信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSummary {
    pub servers_imported: usize,
    pub repos_imported: usize,
    pub backup_targets_imported: usize,
    pub deploy_configs_imported: usize,
    pub backup_configs_imported: usize,
    pub pages_configs_imported: usize,
}

impl ExportData {
    pub fn new(config: &AppConfig) -> Self {
        // 脱敏：清空敏感字段
        let mut servers = config.servers.clone();
        for server in &mut servers {
            match &mut server.auth {
                SshAuth::Password { .. } => {
                    server.auth = SshAuth::Password { password: String::new() };
                }
                SshAuth::PrivateKey { key_path: _, passphrase } => {
                    *passphrase = None;
                }
            }
        }

        let mut repos = config.repos.clone();
        for repo in &mut repos {
            for file in &mut repo.env_files {
                file.local_path = String::new();
                file.remote_path = String::new();
            }
        }

        let mut settings = config.settings.clone();
        settings.cloudflare_api_token = String::new();
        settings.cloudflare_account_id = String::new();
        settings.github_token = String::new();

        Self {
            version: "1.0".to_string(),
            exported_at: now_string(),
            servers,
            repos,
            backup_targets: config.backup_targets.clone(),
            deploy_configs: config.deploy_configs.clone(),
            backup_configs: config.backup_configs.clone(),
            pages_configs: config.pages_configs.clone(),
            settings,
        }
    }
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

    #[test]
    fn legacy_single_script_field_deserializes_into_scripts() {
        let request: DeployRequest = serde_json::from_str(
            r#"{"repoId":"r","rev":"main","serverId":"s","targetDir":"/opt/x","scriptDir":"docker","script":"deploy.sh"}"#,
        )
        .unwrap();
        assert_eq!(request.scripts, vec!["deploy.sh".to_string()]);

        let request: DeployRequest = serde_json::from_str(
            r#"{"repoId":"r","rev":"main","serverId":"s","targetDir":"/opt/x","scripts":["a.sh","b.sh"]}"#,
        )
        .unwrap();
        assert_eq!(request.scripts, vec!["a.sh".to_string(), "b.sh".to_string()]);

        let request: DeployRequest = serde_json::from_str(
            r#"{"repoId":"r","rev":"main","serverId":"s","targetDir":"/opt/x","scripts":null}"#,
        )
        .unwrap();
        assert!(request.scripts.is_empty());
    }

    #[test]
    fn legacy_deploy_record_script_field_deserializes() {
        let record: DeployRecord = serde_json::from_str(
            r#"{
                "id":"1","repoId":"r","repoName":"repo","rev":"main","branch":"main",
                "commit":"abc","commitShort":"abc","commitSubject":"s","serverId":"srv",
                "serverName":"prod","targetDir":"/opt/x","scriptDir":"docker",
                "script":"docker/deploy.sh","runScripts":true,"status":"success",
                "error":null,"log":"","startedAt":"2026-01-01 00:00:00",
                "finishedAt":null,"durationMs":0
            }"#,
        )
        .unwrap();
        assert_eq!(record.scripts, vec!["docker/deploy.sh".to_string()]);
    }

    #[test]
    fn legacy_config_without_deploy_configs_deserializes() {
        let config: AppConfig =
            serde_json::from_str(r#"{"servers":[],"repos":[]}"#).unwrap();
        assert!(config.deploy_configs.is_empty());

        // 缺省字段要有可用默认值：旧配置升级后直接新增配置不会写出空目录 / 关闭脚本。
        let saved: DeployConfig =
            serde_json::from_str(r#"{"id":"c1","name":"prod","repoId":"r","serverId":"s","targetDir":"/opt/app"}"#)
                .unwrap();
        assert!(saved.run_scripts);
        assert_eq!(saved.script_dir, "docker");
        assert!(saved.upload_env);
        assert!(saved.rev.is_empty());
    }

    #[test]
    fn legacy_config_without_env_files_deserializes() {
        let repo: RepoConfig = serde_json::from_str(r#"{"id":"r1","name":"demo","path":"/tmp/demo"}"#)
            .unwrap();
        assert!(repo.env_files.is_empty());

        let record: DeployRecord = serde_json::from_str(
            r#"{
                "id":"1","repoId":"r","repoName":"repo","rev":"main","branch":"main",
                "commit":"abc","commitShort":"abc","commitSubject":"s","serverId":"srv",
                "serverName":"prod","targetDir":"/opt/x","scriptDir":"docker",
                "scripts":[],"runScripts":true,"status":"success",
                "error":null,"log":"","startedAt":"2026-01-01 00:00:00",
                "finishedAt":null,"durationMs":0
            }"#,
        )
        .unwrap();
        assert!(record.env_files.is_empty());

        // 旧请求没有 uploadEnv 字段时默认开启上传。
        let request: DeployRequest = serde_json::from_str(
            r#"{"repoId":"r","rev":"main","serverId":"s","targetDir":"/opt/x"}"#,
        )
        .unwrap();
        assert!(request.upload_env);
    }
}
