//! 数据库备份：把服务器上的 PostgreSQL schema 导出并全量恢复到远端 PostgreSQL。
//!
//! 导出与导入都在目标服务器上完成，本机默认只收一个大小数字：
//! 1. 通过 `pg_dump`（docker exec 或本机命令）导出为压缩的 SQL 文件；
//! 2. 校验压缩文件完整性；
//! 3. 清空并重建目标 schema（默认 public，保留默认角色授权）；
//! 4. `gunzip | psql` 全量导入目标数据库。
//!
//! `settings.db_bundle_keep` 非 0 时多一步：脚本导出后不删远端文件，由本机下载留档
//! （`<数据目录>/backups/`），下载完再把远端那份删掉 —— 源服务器仍然不留副本，
//! 所有备份包集中在执行机上，轮转和水位只需管一处。
//!
//! 文件分工：`execute` 是执行体（脚本上传/执行、下载、轮转），`resolve` 是来源与目标的
//! 校验解析，`db_url` 是连接串解析与脱敏，`scripts` 是远端脚本模板与渲染。

mod db_url;
mod execute;
mod resolve;
mod scripts;
#[cfg(test)]
mod testutil;

pub use db_url::{mask_database_url, split_database_url};
pub(crate) use scripts::render_template;

use execute::clean_lines;
use resolve::resolve_backup;
use scripts::{build_test_script, remote_dump_path};

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::models::{
    now_string, BackupEvent, BackupRecord, BackupRequest, DbBackupSource, DeployStatus,
    ServerConfig,
};
use crate::process::shell_quote;
use crate::ssh::SshClient;
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::{format_duration, human_size};

/// 一次备份的执行结果。
pub struct BackupOutcome {
    pub dump_size: u64,
    /// 导出包在本机的路径；未开启留存时为空串。
    pub bundle_path: String,
}

/// 备份事件发送端（GUI 转发为 Tauri 事件，CLI 直接打印）。
pub type BackupEventSender = UnboundedSender<BackupEvent>;

/// 已校验并解析好的备份任务。
pub struct PreparedBackup {
    pub record: BackupRecord,
    pub source: DbBackupSource,
    /// 实际使用的目标连接串（含密码，仅用于执行，不写日志）。
    pub target: String,
}

/// 数据库备份引擎。
pub struct BackupEngine {
    store: Arc<Store>,
}

impl BackupEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    /// 校验参数、解析来源与目标，并生成一条「运行中」的备份记录（写入历史）。
    pub fn prepare(&self, req: &BackupRequest) -> Result<PreparedBackup> {
        let config = self.store.load_config()?;
        let resolved = resolve_backup(&config, req)?;

        let record = BackupRecord {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: resolved.server.id.clone(),
            server_name: resolved.server.name.clone(),
            database: resolved.source.database.clone(),
            schema: resolved.source.schema.clone(),
            target_name: resolved.target.name.clone(),
            target: mask_database_url(&resolved.target.url),
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            dump_size: 0,
            bundle_path: String::new(),
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };
        self.store
            .upsert_backup(&record, config.settings.backup_history_limit)?;
        Ok(PreparedBackup {
            record,
            source: resolved.source,
            target: resolved.target.url,
        })
    }

    /// 执行备份流程。无论成功失败都会返回带有最终状态的记录。
    pub async fn run(
        &self,
        mut record: BackupRecord,
        source: DbBackupSource,
        target: String,
        events: Option<BackupEventSender>,
    ) -> BackupRecord {
        let started = Instant::now();
        let mut logger = TaskLogger::new(
            events,
            |level, message| BackupEvent::Log { level, message },
            Some(|percent, message| BackupEvent::Progress { percent, message }),
        );

        logger.send(BackupEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始备份 {} · {} @ {}",
            record.server_name, record.database, record.target
        ));

        let result = self.execute(&record, &source, &target, &mut logger).await;

        match result {
            Ok(outcome) => {
                record.dump_size = outcome.dump_size;
                record.bundle_path = outcome.bundle_path.clone();
                record.status = DeployStatus::Success;
                logger.success(format!(
                    "备份成功（{}，耗时 {}）",
                    human_size(outcome.dump_size),
                    format_duration(started.elapsed().as_millis() as u64)
                ));
                if !outcome.bundle_path.is_empty() {
                    logger.info(format!("导出包已留存: {}", outcome.bundle_path));
                }
            }
            Err(err) => {
                record.status = DeployStatus::Failed;
                record.error = Some(err.to_string());
                logger.error(format!("备份失败: {err}"));
            }
        }

        let settings = self
            .store
            .load_config()
            .map(|config| config.settings)
            .unwrap_or_default();
        let limit = settings.backup_history_limit;

        // 只有成功收尾才轮转：失败那次的半截包不该占住窗口，更不该顺手把上一晚的好包挤掉。
        if record.status == DeployStatus::Success {
            match self.prune_bundles(&record, settings.db_bundle_keep, limit) {
                Ok(removed) => {
                    for (name, _) in &removed {
                        logger.info(format!("清理旧导出包 {name}"));
                    }
                    if !removed.is_empty() {
                        let freed: u64 = removed.iter().map(|(_, size)| size).sum();
                        logger.success(format!(
                            "本轮清理 {} 个旧导出包，释放 {}",
                            removed.len(),
                            human_size(freed)
                        ));
                    }
                }
                Err(err) => logger.warn(format!("导出包轮转失败: {err}")),
            }
        }

        record.log = logger.joined();
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        if let Err(err) = self.store.upsert_backup(&record, limit) {
            logger.error(format!("保存备份记录失败: {err}"));
            record.log = logger.joined();
        }

        if let Some(sender) = logger.take_events() {
            let _ = sender.send(BackupEvent::Finished {
                record: record.clone(),
            });
        }
        record
    }

    /// 检查服务器侧备份环境：pg_dump 可用性与 Supabase 连通性。
    /// 与真实备份使用同一套来源/目标解析优先级。
    pub async fn test(&self, req: &BackupRequest) -> Result<String> {
        let config = self.store.load_config()?;
        let resolved = resolve_backup(&config, req)?;
        let server = resolved.server;
        let source = resolved.source;
        let (target_url, target_password) = split_database_url(&resolved.target.url)?;

        let settings = &config.settings;
        let client = SshClient::connect(&server, settings.connect_timeout_secs).await?;
        let script = build_test_script(&source, &target_url, target_password.as_deref());
        let remote = format!("/tmp/deploycode-backup-test-{}.sh", uuid::Uuid::new_v4());

        let result = async {
            self.put_script(&client, &remote, &script).await?;
            let run = client
                .exec_capture(&format!("bash {}", shell_quote(&remote)), 120)
                .await;
            let _ = client
                .exec_capture(&format!("rm -f {}", shell_quote(&remote)), 30)
                .await;
            let (code, output) = run?;
            if code != 0 {
                return Err(CoreError::backup(format!(
                    "环境检查失败: {}",
                    clean_lines(&output)
                )));
            }
            Ok(clean_lines(&output))
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 应用退出时清理仍在执行的远端备份脚本（重新连接并终止进程，删除脚本与导出文件）。
    pub async fn cleanup_remote_script(
        &self,
        server: &ServerConfig,
        record_id: &str,
    ) -> Result<()> {
        let settings = self.store.load_config()?.settings;
        let pidfile = format!("/tmp/deploycode-backup-{record_id}.pid");
        let script = format!("/tmp/deploycode-backup-{record_id}.sh");
        // 导出路径只认 remote_dump_path 一处：它必须与脚本模板里的 OUT= 同规则，
        // 否则善后删的就是别的文件，源机上留一份没人收的全库导出。
        let dump = remote_dump_path(record_id);
        let command = format!(
            "{}; rm -f {} {}",
            crate::engine::kill_script(&pidfile),
            shell_quote(&script),
            shell_quote(&dump)
        );
        let client = SshClient::connect(server, settings.connect_timeout_secs).await?;
        let result = client.exec_capture(&command, 15).await;
        client.disconnect().await;
        result.map(|_| ())
    }
}
