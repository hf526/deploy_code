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
//! - [`nginx`]：服务器 Nginx 容器配置管理
//! - [`release`]：原子发布（releases + current 软链）
//! - [`backup`]：数据库备份（服务器 PG -> Supabase）
//! - [`container`]：容器备份与迁移（docker-compose 项目经本机中转到另一台服务器）
//! - [`pages`]：Cloudflare Pages 部署（wrangler）
//! - [`cronjob`]：cron-job.org 云端定时请求（REST 客户端 + cron 表达式互转）
//! - [`shutdown`]：本机定时关机（下发 Windows 关机请求）
//! - [`process`]：本地命令执行与 shell 工具
//! - [`tasklog`]：三类任务共用的日志器
//! - [`util`]：展示用格式化工具
//! - [`crypto`]：敏感数据加密（AES-256-GCM + HKDF）

pub mod backup;
pub mod container;
pub mod cronjob;
pub mod crypto;
pub mod engine;
pub mod error;
pub mod git;
pub mod models;
pub mod nginx;
pub mod pages;
pub mod process;
pub mod release;
pub mod security;
pub mod shutdown;
pub mod ssh;
pub mod store;
pub mod tasklog;
pub mod util;

pub use backup::{BackupEngine, BackupEventSender, PreparedBackup};
pub use container::{
    BundleManifest, ComposeService, ComposeStack, ComposeStackDetail, ComposeVolume,
    ContainerEngine, ContainerEventSender, ContainerJob, ContainerPlan, read_bundle_manifest,
};
pub use cronjob::{CronHeader, CronJob, CronJobDraft, CronJobRun, CronSchedule, CRON_METHODS};
pub use engine::{repo_info, DeployEngine, EventSender};
pub use error::{CoreError, Result};
pub use git::Git;
pub use models::*;
pub use nginx::{NginxConfigContent, NginxConfigFile, NginxContainerInfo, NginxEngine};
pub use pages::{PagesEngine, PagesEventSender, PreparedPagesDeploy};
pub use security::SecurityReport;
pub use shutdown::{
    checked_delay_minutes, request_shutdown, shutdown_args, CANCEL_WINDOW_SECS,
    MAX_DELAY_MINUTES, MIN_DELAY_MINUTES, OS_GRACE_SECS,
};
pub use store::Store;
pub use tasklog::TaskLogger;
pub use util::{format_duration, human_size};
