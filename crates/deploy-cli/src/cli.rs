use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "deploy-code-cli",
    version,
    about = "DeployCode CLI：多仓库分支管理与 SSH 部署"
)]
pub struct Cli {
    /// 数据目录（默认与客户端共用系统数据目录）
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,

    /// 以 JSON 格式输出
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 仓库管理
    #[command(subcommand)]
    Repo(RepoCommand),

    /// 分支与提交管理
    #[command(subcommand)]
    Branch(BranchCommand),

    /// 部署服务器管理
    #[command(subcommand)]
    Server(ServerCommand),

    /// 部署指定仓库的版本
    Deploy(DeployArgs),

    /// 原子发布的历史版本（releases + current 软链）
    #[command(subcommand)]
    Release(ReleaseCommand),

    /// 查看与管理部署记录
    #[command(subcommand)]
    History(HistoryCommand),

    /// 数据库备份（服务器 PostgreSQL -> 远端 PostgreSQL）
    #[command(subcommand)]
    Backup(BackupCommand),

    /// Cloudflare Pages 部署（wrangler）
    #[command(subcommand)]
    Pages(PagesCommand),

    /// 打印数据目录位置
    Where,
}

#[derive(Subcommand, Debug)]
pub enum RepoCommand {
    /// 添加仓库
    Add(RepoAddArgs),
    /// 列出所有仓库
    List,
    /// 移除仓库（不会删除本地代码）
    Remove {
        /// 仓库 id / 名称 / 路径
        repo: String,
    },
    /// 查看仓库详情
    Info {
        /// 仓库 id / 名称 / 路径
        repo: String,
    },
}

#[derive(Args, Debug)]
pub struct RepoAddArgs {
    /// 仓库本地路径
    pub path: PathBuf,
    /// 显示名称（默认使用目录名）
    #[arg(short, long)]
    pub name: Option<String>,
    /// 默认部署服务器（id / 名称 / host）
    #[arg(short, long)]
    pub server: Option<String>,
    /// 默认部署目录
    #[arg(short, long)]
    pub dir: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum BranchCommand {
    /// 列出分支
    List {
        repo: String,
        /// 同时列出远程分支
        #[arg(short, long)]
        all: bool,
    },
    /// 切换分支
    Switch { repo: String, branch: String },
    /// 创建分支
    Create {
        repo: String,
        name: String,
        /// 起点版本（分支 / 提交 / 远程分支）
        #[arg(long)]
        from: Option<String>,
        /// 只创建不切换
        #[arg(long)]
        no_checkout: bool,
    },
    /// 删除分支
    Delete {
        repo: String,
        branch: String,
        /// 强制删除未合并分支
        #[arg(short, long)]
        force: bool,
    },
    /// 查看提交记录
    Log {
        repo: String,
        /// 条数
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 查看工作区状态
    Status { repo: String },
    /// 提交全部改动
    Commit {
        repo: String,
        /// 提交信息
        #[arg(short, long)]
        message: String,
        /// 允许提交疑似敏感文件（.env / *.pem / id_rsa 等）
        #[arg(long)]
        allow_sensitive: bool,
    },
    /// 回退到指定版本（会丢弃工作区改动）
    Reset {
        repo: String,
        rev: String,
        /// 确认执行
        #[arg(long)]
        yes: bool,
    },
    /// 拉取远端更新
    Fetch { repo: String },
    /// 拉取并合并当前分支
    Pull { repo: String },
}

#[derive(Subcommand, Debug)]
pub enum ReleaseCommand {
    /// 列出服务器上的历史发布版本
    List {
        /// 服务器 id / 名称 / host
        server: String,
        /// 部署目录（服务器上的绝对路径）
        #[arg(short, long)]
        dir: String,
    },
    /// 把 current 软链切换到指定历史版本
    Switch {
        /// 服务器 id / 名称 / host
        server: String,
        /// 部署目录（服务器上的绝对路径）
        #[arg(short, long)]
        dir: String,
        /// 版本目录名（releases/ 下的名称）
        release: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServerCommand {    /// 添加或更新服务器
    Add(ServerAddArgs),
    /// 列出服务器
    List,
    /// 删除服务器
    Remove { server: String },
    /// 测试连接
    Test { server: String },
}

#[derive(Args, Debug)]
pub struct ServerAddArgs {
    /// 服务器名称
    pub name: String,
    /// 主机地址
    #[arg(long)]
    pub host: String,
    /// SSH 端口（默认 22；更新已有服务器时省略则保留原值）
    #[arg(short, long)]
    pub port: Option<u16>,
    /// SSH 用户名
    #[arg(short, long)]
    pub user: String,
    /// 密码认证
    #[arg(long)]
    pub password: Option<String>,
    /// 私钥文件路径（与 --password 二选一）
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// 私钥口令
    #[arg(long)]
    pub passphrase: Option<String>,
    /// 默认部署目录
    #[arg(short, long)]
    pub dir: Option<String>,
    /// 数据库采集方式：docker / system
    #[arg(long)]
    pub db_mode: Option<String>,
    /// 数据库所在容器名（docker 模式）
    #[arg(long)]
    pub db_container: Option<String>,
    /// 数据库名
    #[arg(long)]
    pub db_name: Option<String>,
    /// 数据库用户名
    #[arg(long)]
    pub db_user: Option<String>,
    /// 数据库密码
    #[arg(long)]
    pub db_password: Option<String>,
    /// 备份的 schema（默认 public）
    #[arg(long)]
    pub db_schema: Option<String>,
    /// 该服务器默认使用的备份目标（id / 名称）
    #[arg(long)]
    pub backup_target: Option<String>,
    /// 该服务器专用的数据库连接串（旧字段，优先使用 --backup-target）
    #[arg(long)]
    pub supabase_url: Option<String>,
}

#[derive(Args, Debug)]
pub struct DeployArgs {
    /// 仓库 id / 名称 / 路径
    pub repo: String,
    /// 要部署的版本（分支 / 标签 / 提交），默认当前分支
    #[arg(short, long)]
    pub rev: Option<String>,
    /// 目标服务器（id / 名称 / host），默认使用仓库配置
    #[arg(short, long)]
    pub server: Option<String>,
    /// 目标目录，默认使用仓库或服务器配置
    #[arg(short, long)]
    pub dir: Option<String>,
    /// 脚本目录（相对项目根目录）
    #[arg(long)]
    pub script_dir: Option<String>,
    /// 只执行指定脚本（可重复或逗号分隔，按给定顺序执行）
    #[arg(long, value_delimiter = ',')]
    pub script: Vec<String>,
    /// 跳过脚本执行
    #[arg(long)]
    pub no_scripts: bool,
    /// 跳过上传仓库配置的环境文件（默认会在解压后替换）
    #[arg(long)]
    pub no_env: bool,
}

#[derive(Subcommand, Debug)]
pub enum BackupCommand {
    /// 执行一次备份（pg_dump -> 全量覆盖到目标数据库）
    Run {
        /// 服务器 id / 名称 / host（使用 --config 时可省略）
        server: Option<String>,
        /// 已保存的备份配置 id / 名称（服务器、来源与目标取自配置）
        #[arg(short, long)]
        config: Option<String>,
        /// 备份目标 id / 名称（默认使用配置、服务器或全局默认目标）
        #[arg(short, long)]
        target: Option<String>,
        /// 直接指定目标连接串（优先级最高）
        #[arg(long)]
        supabase_url: Option<String>,
        /// 覆盖数据库名
        #[arg(long)]
        database: Option<String>,
        /// 覆盖 schema
        #[arg(long)]
        schema: Option<String>,
    },
    /// 管理保存的备份配置（名称 + 服务器 + 来源 + 目标）
    #[command(subcommand)]
    Config(BackupConfigCommand),
    /// 管理备份目标（Supabase / Aiven / Neon 等）
    #[command(subcommand)]
    Target(TargetCommand),
    /// 列出备份记录
    List {
        /// 只查看某台服务器
        #[arg(short, long)]
        server: Option<String>,
        /// 条数
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 查看某条备份记录详情（含日志）
    Show { record: String },
    /// 清空全部备份记录
    Clear {
        /// 确认执行
        #[arg(long)]
        yes: bool,
    },
    /// 检查备份环境（pg_dump 与目标连通性）
    Test {
        /// 服务器 id / 名称 / host（使用 --config 时可省略）
        server: Option<String>,
        /// 已保存的备份配置 id / 名称
        #[arg(short, long)]
        config: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum BackupConfigCommand {
    /// 添加或更新备份配置
    Add(BackupConfigAddArgs),
    /// 列出备份配置
    List,
    /// 删除备份配置
    Remove {
        /// 配置 id / 名称
        config: String,
    },
}

#[derive(Args, Debug)]
pub struct BackupConfigAddArgs {
    /// 配置名称（已存在时更新该配置）
    pub name: String,
    /// 服务器 id / 名称 / host
    #[arg(short, long)]
    pub server: String,
    /// 采集方式：docker / system
    #[arg(long)]
    pub mode: Option<String>,
    /// 数据库所在容器名（docker 模式）
    #[arg(long)]
    pub container: Option<String>,
    /// 数据库名
    #[arg(long)]
    pub database: Option<String>,
    /// 数据库用户名
    #[arg(long)]
    pub username: Option<String>,
    /// 数据库密码
    #[arg(long)]
    pub password: Option<String>,
    /// 备份的 schema（默认 public）
    #[arg(long)]
    pub schema: Option<String>,
    /// 备份目标 id / 名称（默认使用服务器或全局默认目标）
    #[arg(short, long)]
    pub target: Option<String>,
    /// 直接指定目标连接串（优先级最高）
    #[arg(long)]
    pub supabase_url: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum TargetCommand {
    /// 添加备份目标
    Add {
        /// 目标名称（如 Supabase / Aiven / Neon）
        name: String,
        /// postgres:// 或 postgresql:// 连接串
        url: String,
    },
    /// 列出备份目标
    List,
    /// 删除备份目标
    Remove { target: String },
}

#[derive(Subcommand, Debug)]
pub enum PagesCommand {
    /// 配置某个仓库的 Pages 部署参数
    Config {
        /// 仓库 id / 名称 / 路径
        repo: String,
        /// 部署平台：cloudflare（默认）或 github
        #[arg(long)]
        provider: Option<String>,
        /// Cloudflare Pages 项目名
        #[arg(short, long)]
        project: Option<String>,
        /// 构建命令（如 npm run build）
        #[arg(short, long)]
        build: Option<String>,
        /// 输出目录（默认 dist）
        #[arg(short, long)]
        output: Option<String>,
        /// Cloudflare 生产分支（默认 main）
        #[arg(long)]
        branch: Option<String>,
        /// GitHub Pages 发布分支（默认 gh-pages）
        #[arg(long)]
        publish_branch: Option<String>,
    },
    /// 构建并部署到 Pages（Cloudflare / GitHub）
    Run {
        /// 仓库 id / 名称 / 路径
        repo: String,
        /// 覆盖部署平台（cloudflare / github）
        #[arg(long)]
        provider: Option<String>,
        /// 覆盖项目名
        #[arg(short, long)]
        project: Option<String>,
        /// 覆盖构建命令
        #[arg(short, long)]
        build: Option<String>,
        /// 覆盖输出目录
        #[arg(short, long)]
        output: Option<String>,
        /// 覆盖分支
        #[arg(long)]
        branch: Option<String>,
        /// 覆盖 GitHub Pages 发布分支
        #[arg(long)]
        publish_branch: Option<String>,
        /// 跳过构建，直接上传现有产物
        #[arg(long)]
        skip_build: bool,
    },
    /// 列出 Pages 部署记录
    List {
        /// 只查看某个仓库
        #[arg(short, long)]
        repo: Option<String>,
        /// 条数
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 查看某条部署记录详情（含日志）
    Show { record: String },
    /// 清空全部 Pages 部署记录
    Clear {
        /// 确认执行
        #[arg(long)]
        yes: bool,
    },
    /// 检查 wrangler / Token / Account 是否可用
    Test { repo: String },
}

#[derive(Subcommand, Debug)]
pub enum HistoryCommand {
    /// 列出部署记录
    List {
        /// 只查看某个仓库
        #[arg(short, long)]
        repo: Option<String>,
        /// 条数
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 查看某条记录详情（含日志）
    Show { record: String },
    /// 按原参数重新部署某条记录
    Redeploy { record: String },
    /// 清空全部记录
    Clear {
        /// 确认执行
        #[arg(long)]
        yes: bool,
    },
}
