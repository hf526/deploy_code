//! 在控制机上跑一次备份 / 恢复，并把过程当成事件流打出去。
//!
//! 业务实现全部来自 `deploy-core`（`BackupEngine` / `ContainerEngine`），与客户端手动点
//! 「执行」、与 CLI 跑的是同一条代码路径 —— 区别只是执行机换了、备份包落在控制机上。
//!
//! 事件有两条出口（[`Sink`]）：
//!
//! - `Journal`：常驻模式的定时任务，一行一人读文本进 journald，进度不打（一条任务几十分钟，
//!   每几步一行会把有用的行冲干净）。
//! - `Stream`：客户端经 SSH 即时发起的任务，一条一 JSON 行进 stdout。**这条模式下 stdout
//!   一个字都不能多打**，客户端按行反序列化；写失败说明对端已经不听了，于是中止任务并清理远端脚本。

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use deploy_core::models::{
    BackupEvent, BackupRecord, BackupRequest, ContainerEvent, ContainerRecord,
    ContainerRestoreRequest, ContainerTarget, DeployStatus, LogLevel,
};
use deploy_core::{BackupEngine, ContainerEngine, ContainerJob, CoreError, Result, Store};
use tokio::sync::mpsc;

use crate::EXIT_FAILED;

/// 对端断开之后收拾远端脚本的时间上限：这一步不该把退出拖住。
const CLEANUP_BUDGET: Duration = Duration::from_secs(30);

/// 哪一类任务：决定用哪个引擎、哪把任务锁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Backup,
    Container,
}

impl Kind {
    /// `--kind` 必填：默认成 backup 会让一次容器任务静默变成数据库备份。
    pub fn from_args(args: &[String]) -> Result<Self> {
        let value = crate::flag(args, "--kind")
            .filter(|item| !item.trim().is_empty())
            .ok_or_else(|| CoreError::config("需要 --kind backup|container"))?;
        match value.trim().to_ascii_lowercase().as_str() {
            "backup" | "db" => Ok(Kind::Backup),
            "container" => Ok(Kind::Container),
            other => Err(CoreError::config(format!(
                "--kind 只接受 backup 或 container，收到: {other}"
            ))),
        }
    }

    /// 与 GUI / CLI 同名的任务锁：同一台机器上三种入口共用一个名额。
    pub fn lock_name(self) -> &'static str {
        match self {
            Kind::Backup => "backup",
            Kind::Container => "container",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Backup => "数据库备份",
            Kind::Container => "容器备份",
        }
    }
}

/// 事件的两条出口。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    Journal,
    Stream,
}

/// 一次任务的下场。分成三种是为了让退出码说话：跑完但失败 = 2，没能开始或中途中止 = 1。
pub enum TaskOutcome {
    Succeeded,
    Failed(String),
}

impl From<&BackupRecord> for TaskOutcome {
    fn from(record: &BackupRecord) -> Self {
        outcome(record.status, record.error.clone())
    }
}

impl From<&ContainerRecord> for TaskOutcome {
    fn from(record: &ContainerRecord) -> Self {
        outcome(record.status, record.error.clone())
    }
}

fn outcome(status: DeployStatus, error: Option<String>) -> TaskOutcome {
    if status == DeployStatus::Failed {
        TaskOutcome::Failed(error.unwrap_or_else(|| "远端没有给出原因".to_string()))
    } else {
        TaskOutcome::Succeeded
    }
}

/// 即时执行一条配置：`deploy-agent trigger --kind <kind> --config <id 或名称>`。
pub async fn trigger(store: Arc<Store>, args: &[String]) -> Result<()> {
    let kind = Kind::from_args(args)?;
    let key = crate::flag(args, "--config")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CoreError::config("trigger 需要 --config <备份配置 id 或名称>"))?;
    let status = execute(&store, kind, key.trim(), Sink::Stream).await;
    finish(status, kind)
}

/// 用控制机上已有的备份包恢复到另一台服务器。
pub async fn restore(store: Arc<Store>, args: &[String]) -> Result<()> {
    let record_id = crate::flag(args, "--record")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CoreError::config("restore 需要 --record <容器备份记录 id>"))?;
    let target = crate::flag(args, "--target")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CoreError::config("restore 需要 --target <服务器 id>"))?;
    let dir = crate::flag(args, "--dir").unwrap_or("").to_string();
    let start = crate::bool_flag(args, "--start");
    // 恢复动的就是容器那台机器，占的也是 container 这把锁。
    let status = restore_bundle(
        &store,
        record_id.trim(),
        target.trim(),
        &dir,
        start,
        Sink::Stream,
    )
    .await;
    finish(status, Kind::Container)
}

/// 把结果翻译成退出码。报错只走 stderr：Stream 模式下 stdout 是事件通道。
fn finish(status: Result<TaskOutcome>, kind: Kind) -> Result<()> {
    match status {
        Ok(TaskOutcome::Succeeded) => Ok(()),
        Ok(TaskOutcome::Failed(message)) => {
            eprintln!("{}失败: {message}", kind.label());
            std::process::exit(EXIT_FAILED);
        }
        // 不在这里 exit(1)：那会把 Busy 一起压成 1，客户端就分不出「名额被占、等会儿再试」
        // 和真失败。原样交回 main，由它按 CoreError 的类型映射退出码（Busy → 3）。
        Err(err) => Err(err),
    }
}

/// 抢锁并跑一条配置（定时循环与 `trigger` 共用这一个入口）。
pub async fn execute(store: &Arc<Store>, kind: Kind, key: &str, sink: Sink) -> Result<TaskOutcome> {
    // 先抢锁再 prepare：prepare 会登记一条 Running 记录，抢不到锁却先登记就留下悬空记录。
    let Some(_lock) = store.try_task_lock(kind.lock_name())? else {
        return Err(CoreError::busy(format!(
            "控制机上已有{}任务在运行，请等它结束",
            kind.label()
        )));
    };
    match kind {
        Kind::Backup => run_backup(store, key, sink).await.map(|record| TaskOutcome::from(&record)),
        Kind::Container => run_container(store, key, sink).await.map(|record| TaskOutcome::from(&record)),
    }
}

/// 抢锁并从控制机上的包恢复。
pub async fn restore_bundle(
    store: &Arc<Store>,
    record_id: &str,
    target_server: &str,
    dir: &str,
    start: bool,
    sink: Sink,
) -> Result<TaskOutcome> {
    let Some(_lock) = store.try_task_lock(Kind::Container.lock_name())? else {
        return Err(CoreError::busy("控制机上已有容器任务在运行，请等它结束"));
    };
    let source = store.find_container_record(record_id).map_err(|_| {
        CoreError::not_found(format!("控制机上没有这条容器备份记录: {record_id}"))
    })?;
    // 轮转会抹空 bundle_path（包已删）：这种情况要说人话，而不是报「文件不存在」。
    if source.bundle_path.trim().is_empty() {
        return Err(CoreError::config(
            "这条记录在控制机上已经没有备份包了（可能已被轮转清理），换一条记录再恢复",
        ));
    }
    let engine = ContainerEngine::new(store.clone());
    let (record, job) = engine.prepare_restore(&ContainerRestoreRequest {
        bundle_path: source.bundle_path.clone(),
        target: ContainerTarget {
            server_id: target_server.to_string(),
            target_dir: dir.to_string(),
            start_services: start,
        },
    })?;
    let done = run_job(store, engine, record, job, sink).await?;
    Ok((&done).into())
}

/// 跑一条数据库备份配置。调用方需已持有 `backup` 任务锁。
async fn run_backup(store: &Arc<Store>, key: &str, sink: Sink) -> Result<BackupRecord> {
    let config = store.load_config()?;
    let saved = Store::find_backup_config(&config, key)?;
    let engine = BackupEngine::new(store.clone());
    let prepared = engine.prepare(&BackupRequest {
        server_id: String::new(),
        backup_config_id: Some(saved.id.clone()),
        source: None,
        target_id: None,
        supabase_url: None,
        database: None,
        schema: None,
    })?;
    let server_id = prepared.record.server_id.clone();
    let record_id = prepared.record.id.clone();
    let (sender, receiver) = mpsc::unbounded_channel::<BackupEvent>();
    let (gone_tx, mut gone_rx) = mpsc::channel::<()>(1);
    let runner = engine.run(prepared.record, prepared.source, prepared.target, Some(sender));
    tokio::pin!(runner);
    // pump 自己成一个任务：它不在场（原来的写法）就没人写事件，客户端只能等任务整个结束才看到
    // 输出，「写不出去 = 客户端断开」这个判定也永远轮不到发生。
    let pump = tokio::spawn(pump(receiver, sink, gone_tx));
    let outcome = loop {
        // biased：引擎结束会顺手 drop 掉 sender，于是两条分支在同一次轮询里双双就绪；
        // 随机挑会把刚跑完的任务报成「客户端断开」，连带把成品当半成品清掉。
        tokio::select! {
            biased;
            record = &mut runner => {
                // 等 pump 把队列里剩下的事件倒完，客户端才算拿到一条完整的任务。
                let _ = pump.await;
                break Ok(record);
            }
            Some(()) = gone_rx.recv(), if sink == Sink::Stream => {
                break Err(CoreError::ssh("客户端已断开，控制机中止这次备份"));
            }
        }
    };
    match outcome {
        Ok(record) => Ok(record),
        Err(err) => {
            // 控制机自己登记的那条 Running 先收掉：这一把任务锁还在我们手上，
            // reconcile_interrupted 帮不上忙，放着就永远显示「进行中」。
            if let Err(write_err) =
                store.abandon_backup_record(&record_id, "客户端已断开，控制机中止了这次备份")
            {
                eprintln!("收敛控制机备份记录失败: {write_err}");
            }
            // 对端不听了，源服务器上那份 pg_dump 可能还在跑：像 GUI 的取消那样收拾干净。
            // 返回时 runner 随之析构，那条 SSH 会话和 sender 一起放掉。
            cleanup_backup(store, &server_id, &record_id).await;
            Err(err)
        }
    }
}

/// 跑一条容器备份配置。调用方需已持有 `container` 任务锁。
async fn run_container(store: &Arc<Store>, key: &str, sink: Sink) -> Result<ContainerRecord> {
    let config = store.load_config()?;
    let saved = Store::find_container_config(&config, key)?.clone();
    let engine = ContainerEngine::new(store.clone());
    let (record, job) = engine.prepare(&saved.request())?;
    run_job(store, engine, record, job, sink).await
}

/// 容器任务的公共执行部分：事件转发 + 对端断开则清理它碰过的每一台机器。
async fn run_job(
    store: &Arc<Store>,
    engine: ContainerEngine,
    record: ContainerRecord,
    job: ContainerJob,
    sink: Sink,
) -> Result<ContainerRecord> {
    let servers: Vec<String> = [record.server_id.clone(), record.target_server_id.clone()]
        .into_iter()
        .filter(|id| !id.is_empty())
        .collect();
    let record_id = record.id.clone();
    let (sender, receiver) = mpsc::unbounded_channel::<ContainerEvent>();
    let (gone_tx, mut gone_rx) = mpsc::channel::<()>(1);
    let runner = engine.run(record, job, Some(sender));
    tokio::pin!(runner);
    let pump = tokio::spawn(pump(receiver, sink, gone_tx));
    let outcome = loop {
        // 与 run_backup 同一套：pump 要在场，引擎优先。
        tokio::select! {
            biased;
            done = &mut runner => {
                let _ = pump.await;
                break Ok(done);
            }
            Some(()) = gone_rx.recv(), if sink == Sink::Stream => {
                break Err(CoreError::ssh("客户端已断开，控制机中止这次容器备份"));
            }
        }
    };
    match outcome {
        Ok(done) => Ok(done),
        Err(err) => {
            if let Err(write_err) =
                store.abandon_container_record(&record_id, "客户端已断开，控制机中止了这次容器备份")
            {
                eprintln!("收敛控制机容器记录失败: {write_err}");
            }
            for server_id in &servers {
                cleanup_container(store, &engine, server_id, &record_id).await;
            }
            Err(err)
        }
    }
}

async fn cleanup_backup(store: &Arc<Store>, server_id: &str, record_id: &str) {
    let cleanup = async {
        let Ok(config) = store.load_config() else {
            return;
        };
        let Ok(server) = Store::find_server(&config, server_id) else {
            return;
        };
        let _ = BackupEngine::new(store.clone())
            .cleanup_remote_script(server, record_id)
            .await;
    };
    let _ = tokio::time::timeout(CLEANUP_BUDGET, cleanup).await;
}

async fn cleanup_container(
    store: &Arc<Store>,
    engine: &ContainerEngine,
    server_id: &str,
    record_id: &str,
) {
    let cleanup = async {
        let Ok(config) = store.load_config() else {
            return;
        };
        let Ok(server) = Store::find_server(&config, server_id) else {
            return;
        };
        let _ = engine.cleanup_remote(server, record_id).await;
    };
    let _ = tokio::time::timeout(CLEANUP_BUDGET, cleanup).await;
}

/// 逐条取出事件并按 [`Sink`] 输出。Stream 模式下写不进去就通知上层中止。
async fn pump<E>(
    mut receiver: mpsc::UnboundedReceiver<E>,
    sink: Sink,
    gone: mpsc::Sender<()>,
) where
    E: serde::Serialize + EventLine + Send,
{
    while let Some(event) = receiver.recv().await {
        match sink {
            Sink::Journal => {
                if let Some(line) = event.line() {
                    // 不用 println!：它写失败会 panic，而 pump 在独立任务里跑，panic 会被上层的
                    // JoinHandle 吞掉 —— 定时任务日志静默少一片。journald 那条管道断了就该丢一行继续跑。
                    let mut stdout = std::io::stdout().lock();
                    let _ = writeln!(stdout, "{line}");
                }
            }
            Sink::Stream => {
                let Ok(json) = serde_json::to_string(&event) else {
                    continue;
                };
                // 锁必须在这个块里就释放：跨 await 持有 StdoutLock 会让整个 future 不再 Send，
                // 两条调度循环就没法 tokio::spawn 到多线程 runtime 上。
                let written = {
                    let mut stdout = std::io::stdout().lock();
                    writeln!(stdout, "{json}").and_then(|()| stdout.flush()).is_ok()
                };
                if !written {
                    // 写失败 = 客户端已经关了这条 SSH 通道。知会一声，让上层中止并清理。
                    let _ = gone.send(()).await;
                    return;
                }
            }
        }
    }
}

/// 两类任务事件的共同形状：给人读的那一行。
pub trait EventLine {
    /// `None` 表示这条在 journald 里不值得占一行（进度、Started）。
    fn line(&self) -> Option<String>;
}

impl EventLine for BackupEvent {
    fn line(&self) -> Option<String> {
        match self {
            BackupEvent::Log { level, message } => Some(event_line(*level, message)),
            BackupEvent::Finished { record } => Some(event_line(
                if record.status == DeployStatus::Failed {
                    LogLevel::Error
                } else {
                    LogLevel::Success
                },
                &format!(
                    "记录 {} 结束，状态 {:?}，耗时 {}ms，导出包 {}",
                    short(&record.id),
                    record.status,
                    record.duration_ms,
                    if record.bundle_path.is_empty() {
                        "未留存"
                    } else {
                        record.bundle_path.as_str()
                    }
                ),
            )),
            _ => None,
        }
    }
}

impl EventLine for ContainerEvent {
    fn line(&self) -> Option<String> {
        match self {
            ContainerEvent::Log { level, message } => Some(event_line(*level, message)),
            ContainerEvent::Finished { record } => Some(event_line(
                if record.status == DeployStatus::Failed {
                    LogLevel::Error
                } else {
                    LogLevel::Success
                },
                &format!(
                    "记录 {} 结束，状态 {:?}，备份包 {} 字节",
                    short(&record.id),
                    record.status,
                    record.bundle_size
                ),
            )),
            _ => None,
        }
    }
}

fn event_line(level: LogLevel, message: &str) -> String {
    let mark = match level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN",
        _ => "INFO",
    };
    format!(
        "{} {mark} {message}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )
}

/// 日志里 id 用前 8 位就够定位，完整值在记录文件里。
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn kind_requires_and_parses_its_flag() {
        assert_eq!(Kind::from_args(&args("--kind backup")).unwrap(), Kind::Backup);
        assert_eq!(
            Kind::from_args(&args("--kind CONTAINER")).unwrap(),
            Kind::Container
        );
        assert!(Kind::from_args(&args("--config x")).is_err());
        let err = Kind::from_args(&args("--kind pages")).unwrap_err().to_string();
        assert!(err.contains("backup"), "{err}");
    }

    #[test]
    fn lock_names_match_the_ones_gui_and_cli_use() {
        // 名字对不上就等于定时与手动各跑各的，同机上会同时打两份包。
        assert_eq!(Kind::Backup.lock_name(), "backup");
        assert_eq!(Kind::Container.lock_name(), "container");
    }

    #[test]
    fn finished_record_without_error_still_reads_a_reason() {
        let failed = BackupRecord {
            status: DeployStatus::Failed,
            error: None,
            ..backup_record()
        };
        match TaskOutcome::from(&failed) {
            TaskOutcome::Failed(message) => assert_eq!(message, "远端没有给出原因"),
            _ => panic!("失败记录不该被当成成功"),
        }
        let ok = BackupRecord {
            status: DeployStatus::Success,
            error: Some("残留信息".to_string()),
            ..backup_record()
        };
        assert!(matches!(TaskOutcome::from(&ok), TaskOutcome::Succeeded));
    }

    #[test]
    fn journal_skips_progress_but_keeps_logs_and_the_tail() {
        assert!(BackupEvent::Progress {
            percent: 30,
            message: "下载中".to_string()
        }
        .line()
        .is_none());
        assert!(BackupEvent::Started {
            record_id: "abc".to_string()
        }
        .line()
        .is_none());
        let log = BackupEvent::Log {
            level: LogLevel::Warn,
            message: "磁盘紧张".to_string(),
        }
        .line()
        .unwrap();
        assert!(log.contains("WARN 磁盘紧张"), "{log}");
        let tail = ContainerEvent::Finished {
            record: ContainerRecord {
                id: "abcdefghijklmnop".to_string(),
                status: DeployStatus::Success,
                bundle_size: 12,
                ..container_record()
            },
        }
        .line()
        .unwrap();
        assert!(tail.contains("记录 abcdefgh"), "{tail}");
        assert!(tail.contains("12 字节"), "{tail}");
    }

    fn backup_record() -> BackupRecord {
        BackupRecord {
            id: "id".to_string(),
            server_id: "s1".to_string(),
            server_name: "源机".to_string(),
            database: "app".to_string(),
            schema: "public".to_string(),
            target_name: String::new(),
            target: String::new(),
            status: DeployStatus::Success,
            error: None,
            log: String::new(),
            dump_size: 0,
            bundle_path: String::new(),
            started_at: String::new(),
            finished_at: None,
            duration_ms: 0,
        }
    }

    fn container_record() -> ContainerRecord {
        ContainerRecord {
            id: String::new(),
            kind: deploy_core::models::ContainerRecordKind::Backup,
            project: "blog".to_string(),
            server_id: String::new(),
            server_name: String::new(),
            target_server_id: String::new(),
            target_server_name: String::new(),
            target_dir: String::new(),
            bundle_path: String::new(),
            bundle_size: 0,
            services: Vec::new(),
            volumes: Vec::new(),
            images: Vec::new(),
            include_volumes: true,
            include_images: true,
            status: DeployStatus::Success,
            error: None,
            log: String::new(),
            started_at: String::new(),
            finished_at: None,
            duration_ms: 0,
        }
    }
}
