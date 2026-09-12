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

    /// 查看与管理部署记录
    #[command(subcommand)]
    History(HistoryCommand),

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
pub enum ServerCommand {
    /// 添加或更新服务器
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
    /// SSH 端口
    #[arg(short, long, default_value_t = 22)]
    pub port: u16,
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
    /// 只执行指定脚本（如 deploy.sh）
    #[arg(long)]
    pub script: Option<String>,
    /// 跳过脚本执行
    #[arg(long)]
    pub no_scripts: bool,
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
