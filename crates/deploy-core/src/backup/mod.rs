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
    now_string, BackupEvent, BackupRecord, BackupRequest, DbBackupSource, DeployStatus, LogLevel,
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

/// 一批备份的结果：每条配置一条记录，按执行顺序排列。
///
/// `blocked` 只在「一条都没跑起来」时有值：此时没有任何记录写盘、也没有 started/finished
/// 事件发出，界面等不到收敛信号，必须由调用方把错误带回给请求方（与批量部署同一约定）。
pub struct BackupBatch {
    pub records: Vec<BackupRecord>,
    pub succeeded: usize,
    /// 准备阶段就被拒绝的配置同样算失败，只是它没有自己的记录。
    pub failed: usize,
    pub blocked: Option<CoreError>,
}

/// 直接向事件通道写一行日志（批次边界上的行不属于任何一条记录，借不了 `TaskLogger`）。
fn emit_log(events: &Option<BackupEventSender>, level: LogLevel, message: String) {
    if let Some(sender) = events {
        let _ = sender.send(BackupEvent::Log { level, message });
    }
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

    /// 串行执行多条已保存的备份配置：每条一份记录，**某一条失败不影响后面的继续跑**。
    ///
    /// 与批量部署的「失败即停」相反：这一批里每条配置是互相独立的库（甚至不同服务器），
    /// 一条用户名写错不该把另外两个库一起掐掉；每条各自的成败由它自己的记录表达，
    /// 整批的性质由末尾那条 `BatchFinished` 汇总。准备阶段就被拒绝的配置没有记录，
    /// 也就不会有 `Finished`，只能以日志行 + 汇总计数记下来。
    ///
    /// `on_begin` / `on_end` 在每条记录开始与结束时各回调一次：整批只有一个任务句柄，
    /// 调用方要能随时知道「现在跑的是哪一条」，退出清理才不会去删已经结束的远端脚本。
    /// 整个批次共用一个发送端并持有到最后一条结束，接收端才不会在中途认为任务已收尾。
    pub async fn run_configs(
        &self,
        config_ids: &[String],
        events: Option<BackupEventSender>,
        mut on_begin: impl FnMut(&BackupRecord),
        mut on_end: impl FnMut(&BackupRecord),
    ) -> BackupBatch {
        let total = config_ids.len();
        // 名称只用于日志归属（报错要点名是哪条配置）；执行参数仍由 prepare 重新读盘解析，
        // 免得这份快照与 CLI 刚改过的配置不一致时，批次照着旧名字跑。
        let names: Vec<String> = self
            .store
            .load_config()
            .map(|config| {
                config_ids
                    .iter()
                    .map(|id| {
                        Store::find_backup_config(&config, id)
                            .map(|item| item.name.clone())
                            .unwrap_or_else(|_| id.clone())
                    })
                    .collect()
            })
            .unwrap_or_else(|_| config_ids.to_vec());

        let mut batch = BackupBatch {
            records: Vec::with_capacity(total),
            succeeded: 0,
            failed: 0,
            blocked: None,
        };
        let mut first_error: Option<CoreError> = None;

        for (index, config_id) in config_ids.iter().enumerate() {
            let name = names
                .get(index)
                .cloned()
                .unwrap_or_else(|| config_id.clone());
            emit_log(
                &events,
                LogLevel::Info,
                format!("[批量 {}/{}] {}", index + 1, total, name),
            );
            let request = BackupRequest {
                server_id: String::new(),
                backup_config_id: Some(config_id.clone()),
                source: None,
                target_id: None,
                supabase_url: None,
                database: None,
                schema: None,
            };
            let prepared = match self.prepare(&request) {
                Ok(prepared) => prepared,
                Err(err) => {
                    batch.failed += 1;
                    emit_log(
                        &events,
                        LogLevel::Error,
                        format!("配置「{name}」未执行: {err}"),
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                    continue;
                }
            };
            on_begin(&prepared.record);
            let record = self
                .run(
                    prepared.record,
                    prepared.source,
                    prepared.target,
                    events.clone(),
                )
                .await;
            on_end(&record);
            if record.status == DeployStatus::Success {
                batch.succeeded += 1;
            } else {
                batch.failed += 1;
            }
            batch.records.push(record);
        }

        if batch.records.is_empty() {
            // 一条都没跑起来：界面收不到任何事件，由调用方像单条备份那样同步报错。
            batch.blocked = first_error;
            return batch;
        }
        if total > 1 {
            let level = if batch.failed == 0 {
                LogLevel::Success
            } else {
                LogLevel::Warn
            };
            emit_log(
                &events,
                level,
                format!(
                    "批量备份结束：{} 成功 / {} 失败（共 {total} 条）",
                    batch.succeeded, batch.failed
                ),
            );
            if let Some(sender) = &events {
                let _ = sender.send(BackupEvent::BatchFinished {
                    total,
                    succeeded: batch.succeeded,
                    failed: batch.failed,
                });
            }
        }
        batch
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::testutil::source;
    use crate::models::{BackupConfig, ServerConfig, SshAuth};

    use tokio::sync::mpsc::unbounded_channel;

    fn store_with_configless_source() -> Arc<Store> {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-db-batch-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::new(&dir));
        store
            .mutate_config(|config| {
                let server = ServerConfig::new(
                    "prod".to_string(),
                    "h".to_string(),
                    "u".to_string(),
                    SshAuth::Password {
                        password: "x".to_string(),
                    },
                );
                config.servers.push(server.clone());
                // 故意不绑目标：解析到「这条配置自己的目标」时就该被拒绝，一步网络都不碰。
                config
                    .backup_configs
                    .push(BackupConfig::new("no-target".to_string(), server.id, source()));
                Ok(())
            })
            .unwrap();
        store
    }

    /// 一批里每条都在准备阶段被拒（没有目标 / 配置不存在）：既没有记录也就没有任何
    /// started/finished 事件，界面等不到收敛信号，必须走 `blocked` 让命令层同步报错。
    #[tokio::test]
    async fn run_configs_blocks_and_counts_when_nothing_can_start() {
        let store = store_with_configless_source();
        let engine = BackupEngine::new(store.clone());
        let (sender, mut receiver) = unbounded_channel::<BackupEvent>();
        let ids = vec!["no-target".to_string(), "missing".to_string()];

        let batch = engine.run_configs(&ids, Some(sender), |_| {}, |_| {}).await;

        assert!(batch.records.is_empty());
        assert_eq!((batch.succeeded, batch.failed), (0, 2));
        assert!(batch.blocked.is_some(), "一条都没跑起来时要带回错误");
        // 没有记录写盘，也就不会在历史列表里留下「进行中」的悬空条目。
        assert!(store.load_backups().unwrap().is_empty());

        let mut lines = Vec::new();
        let mut finished = 0;
        while let Ok(event) = receiver.try_recv() {
            match event {
                BackupEvent::Log { message, .. } => lines.push(message),
                BackupEvent::Finished { .. } => finished += 1,
                BackupEvent::BatchFinished { .. } => panic!("一条都没跑起来时不该发汇总事件"),
                _ => {}
            }
        }
        assert_eq!(finished, 0);
        // 报错要点名是哪条配置，且按列表顺序给批次行。
        assert!(
            lines.iter().any(|line| line.contains("[批量 1/2] no-target")),
            "{lines:?}"
        );
        assert!(
            lines.iter()
                .any(|line| line.contains("配置「no-target」未执行")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("配置「missing」未执行")),
            "{lines:?}"
        );
    }

    /// Rust ↔ TS 是手工镜像（没有代码生成），而 `src/lib/types/backup.ts` 写的是
    /// `{ type: "batchFinished", total, succeeded, failed }`。字段名或 tag 一改，
    /// 前端只会「收不到汇总、界面停在最后一条记录的颜色上」，编译器一声不响。
    #[test]
    fn batch_finished_serializes_the_shape_the_frontend_reads() {
        let json = serde_json::to_value(BackupEvent::BatchFinished {
            total: 3,
            succeeded: 1,
            failed: 2,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "batchFinished", "total": 3, "succeeded": 1, "failed": 2 }),
        );
    }
}
