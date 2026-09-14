//! DeployCode 核心库。
//!
//! 该 crate 不依赖任何 GUI 框架，GUI（Tauri）与 CLI 共用同一套实现：
//!
//! - [`models`]：数据模型与序列化结构
//! - [`store`]：配置与部署记录持久化
//! - [`git`]：本地仓库 Git 操作
//! - [`ssh`]：SSH 连接、命令执行与 SFTP 上传
//! - [`security`]：服务器安全检查（登录日志 / sshd 配置 / 防火墙）
//! - [`engine`]：部署流程编排
//! - [`release`]：原子发布（releases + current 软链）
//! - [`backup`]：数据库备份（服务器 PG -> Supabase）
//! - [`pages`]：Cloudflare Pages 部署（wrangler）
//! - [`process`]：本地命令执行与 shell 工具

pub mod backup;
pub mod engine;
pub mod error;
pub mod git;
pub mod models;
pub mod pages;
pub mod process;
pub mod release;
pub mod security;
pub mod ssh;
pub mod store;

pub use backup::{BackupEngine, BackupEventSender, PreparedBackup};
pub use engine::{repo_info, DeployEngine, EventSender};
pub use error::{CoreError, Result};
pub use git::Git;
pub use models::*;
pub use pages::{PagesEngine, PagesEventSender, PreparedPagesDeploy};
pub use security::SecurityReport;
pub use store::Store;
