//! 控制机（备份 agent）相关命令：安装 / 下发 / 回读 / 即时发起。
//!
//! 这一层只做参数校验、事件转发与名额登记，业务实现全在 `deploy_core::agent::AgentControl`
//! （GUI 与 CLI 共用）。三个刻意的形状：
//!
//! 1. **发起一次远端任务的命令会等到底**（不像本机那样 spawn 后立即返回）：本机只是转发
//!    通道，任务真身在控制机上。等到底换来的是「根本没能开始」能作为命令的 Err 回到界面，
//!    而不是只留一行日志。进度与日志仍是实时事件，界面照旧走 `backup://event` /
//!    `container://event`，`applyTaskEvent` 那边不用区分本机还是远端。
//! 2. **远端任务占的是本机的同一个名额**（`ClaimKind::Backup` / `Container`）：一次只让
//!    用户跑一条同类任务，界面上也只有一个 `live*` 状态，取消、进度、收尾全部复用。
//! 3. **取消 = 关掉这条 SSH 通道**：abort 掉转发任务会让 `SshClient` 析构，控制机写不出
//!    stdout 就明白对端走了，于是自己停手并清理它在源机 / 目标机上留下的目录与包；
//!    本机不需要替它打扫（也没有那条记录可改，记录在控制机上）。

use std::future::Future;
use std::sync::Arc;

use deploy_core::agent::{
    AgentBinaryInfo, AgentControl, AgentStaleness, AgentStatus, AgentSyncReport,
};
use deploy_core::models::{BackupEvent, BackupRecord, ContainerEvent, ContainerRecord};
use deploy_core::{CoreError, Result, Store};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::UnboundedSender;

use crate::state::{ActiveContainer, AppState, ClaimGuard, ClaimKind};

/// 空 `server_id` 表示「用设置里那台控制机」，与 core 侧约定一致。
fn explicit_server(server_id: &str) -> Option<String> {
    let value = server_id.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn control_of(store: Arc<Store>) -> AgentControl {
    AgentControl::new(store)
}

/// 本机这份 agent 可执行文件的情况：只读盘上有什么，不连服务器、不占任务名额。
#[tauri::command(async)]
pub fn agent_binary_info(state: State<AppState>) -> AgentBinaryInfo {
    deploy_core::agent::binary_info(&state.store)
}

#[tauri::command]
pub async fn install_agent(state: State<'_, AppState>, server_id: String) -> Result<AgentStatus> {
    let key = server_id.trim().to_string();
    if key.is_empty() {
        return Err(CoreError::config("请选择一台服务器作为控制机"));
    }
    // promote = 装新机 + 记成新控制机 + 收回原来那台。
    // 装成功才把它记成控制机：失败还留下一个 id，后面的「下发 / 回读」都会对着没装成的机器报错。
    control_of(state.store.clone()).promote(&key).await
}

/// 换控制机时没能收回的旧机器：只读本机状态，不连服务器、不占任务名额。
#[tauri::command(async)]
pub fn agent_orphans(state: State<AppState>) -> Vec<String> {
    state.store.orphan_agent_servers()
}

/// 控制机上那份与本机设置是否已经不一致：只读本机的 agent-sync.json，不连服务器。
///
/// 界面那个「条数对得上就不提示」的口径看不见定时改动，也看不见某条配置已经改回本机，
/// 而这两件事都会在夜里做出与界面上相反的动作，所以必须有一个能问的地方。
#[tauri::command(async)]
pub fn agent_staleness(state: State<AppState>) -> AgentStaleness {
    deploy_core::agent::staleness(&state.store)
}

#[tauri::command]
pub async fn uninstall_agent(state: State<'_, AppState>, server_id: String) -> Result<String> {
    let target = explicit_server(&server_id);
    control_of(state.store.clone())
        .uninstall(target.as_deref())
        .await
}

#[tauri::command]
pub async fn agent_status(state: State<'_, AppState>, server_id: String) -> Result<AgentStatus> {
    let target = explicit_server(&server_id);
    control_of(state.store.clone()).status(target.as_deref()).await
}

/// 下发配置：把其余服务器的凭据与「执行位 = 控制机」的备份配置注过去。
#[tauri::command]
pub async fn agent_sync(state: State<'_, AppState>, server_id: String) -> Result<AgentSyncReport> {
    let target = explicit_server(&server_id);
    control_of(state.store.clone()).sync(target.as_deref()).await
}

#[tauri::command]
pub async fn agent_logs(state: State<'_, AppState>, lines: usize) -> Result<Vec<String>> {
    control_of(state.store.clone()).logs(lines, None).await
}

/// 回读控制机上的数据库备份记录。
///
/// 只读、不并进本机记录：本机的删除 / 清空管不到控制机那份，混在一起会出现
/// 「删了又回来」。界面用「本机 / 控制机」两个来源分开列。
#[tauri::command]
pub async fn agent_backup_records(
    state: State<'_, AppState>,
    limit: usize,
) -> Result<Vec<BackupRecord>> {
    control_of(state.store.clone()).backup_records(limit, None).await
}

#[tauri::command]
pub async fn agent_container_records(
    state: State<'_, AppState>,
    limit: usize,
) -> Result<Vec<ContainerRecord>> {
    control_of(state.store.clone()).container_records(limit, None).await
}

/// 让控制机立刻跑一条数据库备份配置，事件实时转给前端。
#[tauri::command]
pub async fn start_agent_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    config_id: String,
) -> Result<String> {
    let claim = ClaimGuard::acquire(&app, ClaimKind::Backup)?
        .ok_or_else(|| CoreError::busy("已有备份正在进行，请等待完成后再试"))?;
    let id = config_id.trim().to_string();
    let _claim = claim;
    run_stream(&app, "backup://event", state.store.clone(), move |control, sender| {
        async move {
            control
                .run_backup_config(&id, &mut |event: BackupEvent| {
                    let _ = sender.send(event);
                }, None)
                .await
        }
    })
    .await
}

/// 让控制机立刻跑一条容器备份配置。
#[tauri::command]
pub async fn start_agent_container(
    app: AppHandle,
    state: State<'_, AppState>,
    config_id: String,
) -> Result<String> {
    let agent_server = agent_server_id(&state.store)?;
    let id = config_id.trim().to_string();
    run_container_stream(&app, state.store.clone(), agent_server, move |control, sender| {
        async move {
            control
                .run_container_config(&id, &mut |event: ContainerEvent| {
                    let _ = sender.send(event);
                }, None)
                .await
        }
    })
    .await
}

/// 用控制机上已有的包恢复到另一台服务器（包在控制机上，所以搬运由它出面）。
#[tauri::command]
pub async fn restore_agent_container(
    app: AppHandle,
    state: State<'_, AppState>,
    record_id: String,
    target_server_id: String,
    target_dir: String,
    start_services: bool,
) -> Result<String> {
    let agent_server = agent_server_id(&state.store)?;
    let (record, target, dir) = (
        record_id.trim().to_string(),
        target_server_id.trim().to_string(),
        target_dir.trim().to_string(),
    );
    run_container_stream(&app, state.store.clone(), agent_server, move |control, sender| {
        async move {
            control
                .restore_container_bundle(
                    &record,
                    &target,
                    &dir,
                    start_services,
                    &mut |event: ContainerEvent| {
                        let _ = sender.send(event);
                    },
                    None,
                )
                .await
        }
    })
    .await
}

/// 起一个远端任务并把事件转成 Tauri 事件；返回控制机给出的记录 id。
///
/// worker 单独 spawn 是为了拿到它的 `AbortHandle`：容器任务要能被「停止」打断，
/// 而中止点只能落在持有 SSH 连接的那个任务上。
async fn run_stream<E, F, Fut>(
    app: &AppHandle,
    event_name: &'static str,
    store: Arc<Store>,
    work: F,
) -> Result<String>
where
    E: serde::Serialize + Send + 'static,
    F: FnOnce(AgentControl, UnboundedSender<E>) -> Fut,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<E>();
    let worker = tokio::spawn(work(control_of(store), sender));
    let mut record_id = String::new();
    while let Some(event) = receiver.recv().await {
        if let Some(id) = started_record_id(&event) {
            record_id = id;
        }
        let _ = app.emit(event_name, &event);
    }
    // 通道关（发送端随任务一起析构）之后再收任务结果：错误只有在这时候才说得准。
    outcome(worker.await).map(|_| record_id)
}

/// 容器版：额外把任务登记进 `AppState`，让既有的「停止任务」按钮找得到中止句柄。
async fn run_container_stream<F, Fut>(
    app: &AppHandle,
    store: Arc<Store>,
    agent_server: String,
    work: F,
) -> Result<String>
where
    F: FnOnce(AgentControl, UnboundedSender<ContainerEvent>) -> Fut,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let claim = ClaimGuard::acquire(app, ClaimKind::Container)?
        .ok_or_else(|| CoreError::busy("已有容器任务正在进行，请等待完成后再试"))?;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<ContainerEvent>();
    let worker = tokio::spawn(work(control_of(store), sender));
    let abort = worker.abort_handle();
    let mut tracking = RemoteTracking::new(app.clone(), claim);
    let mut record_id = String::new();
    while let Some(event) = receiver.recv().await {
        if let ContainerEvent::Started { record_id: id } = &event {
            // 记录 id 是控制机生成的，第一条 started 到手才能登记。
            record_id = id.clone();
            tracking.begin(&record_id, vec![agent_server.clone()], abort.clone());
        }
        let _ = app.emit("container://event", &event);
    }
    let result = outcome(worker.await).map(|_| record_id);
    drop(tracking);
    result
}

/// worker 的下场：正常返回就透传它的结果；被 abort（用户点了停止）就报「已停止」。
fn outcome(join: std::result::Result<Result<()>, tokio::task::JoinError>) -> Result<()> {
    match join {
        Ok(inner) => inner,
        Err(err) if err.is_cancelled() => Err(CoreError::backup("任务已停止")),
        Err(err) => Err(CoreError::backup(format!("远端任务异常结束: {err}"))),
    }
}

/// 从事件里取 started 的记录 id（备份与容器两种事件都要认）。
fn started_record_id<E: serde::Serialize>(event: &E) -> Option<String> {
    let value = serde_json::to_value(event).ok()?;
    if value.get("type")?.as_str()? != "started" {
        return None;
    }
    value
        .get("recordId")
        .and_then(|item| item.as_str())
        .map(str::to_string)
}

/// 远端容器任务的本机登记：Drop 时摘掉登记项并释放名额。
///
/// 名额之所以跟着 `started` 才占用到「有登记项」的状态，是因为期间用户点停止会提示
/// 「没有正在进行的任务」—— 那一刻任务确实还没在控制机上跑起来，界面上也没有进度可看。
struct RemoteTracking {
    app: AppHandle,
    record_id: Option<String>,
    _claim: ClaimGuard,
}

impl RemoteTracking {
    fn new(app: AppHandle, claim: ClaimGuard) -> Self {
        Self {
            app,
            record_id: None,
            _claim: claim,
        }
    }

    fn begin(&mut self, record_id: &str, server_ids: Vec<String>, abort: tokio::task::AbortHandle) {
        let state = self.app.state::<AppState>();
        state.track_container(
            record_id,
            ActiveContainer {
                server_ids,
                record_id: record_id.to_string(),
                abort,
            },
        );
        self.record_id = Some(record_id.to_string());
    }
}

impl Drop for RemoteTracking {
    fn drop(&mut self) {
        if let Some(id) = self.record_id.take() {
            let state = self.app.state::<AppState>();
            state.take_container(&id);
        }
    }
}

/// 设置里指定的控制机；没指定时报一句能看懂的话。
fn agent_server_id(store: &Store) -> Result<String> {
    let id = store
        .load_config()?
        .settings
        .agent_server_id
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(CoreError::config("还没有指定控制机，请先在控制机页面安装 agent"));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::models::DeployStatus;

    fn store_with(agent_id: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-agent-cmd-{}",
            deploy_core::models::new_id()
        ));
        let store = Store::new(&dir);
        let mut config = deploy_core::models::AppConfig::default();
        config.settings.agent_server_id = agent_id.to_string();
        store.save_config(&config).unwrap();
        store
    }

    #[test]
    fn explicit_server_treats_blank_as_use_the_saved_one() {
        assert_eq!(explicit_server("").as_deref(), None);
        assert_eq!(explicit_server("   ").as_deref(), None);
        assert_eq!(explicit_server("s2").as_deref(), Some("s2"));
        // 带空白的 id 要能被 trimmed 成可用值（界面传过来的 id 不带空白，命令行可能带）。
        assert_eq!(explicit_server(" s2 ").as_deref(), Some("s2"));
    }

    #[test]
    fn agent_server_id_is_trimmed_and_required() {
        assert_eq!(agent_server_id(&store_with(" s1 ")).unwrap(), "s1");
        let err = agent_server_id(&store_with("")).unwrap_err().to_string();
        assert!(err.contains("控制机"), "{err}");
    }

    #[test]
    fn started_event_carries_the_record_id_for_both_kinds() {
        // 登记与返回 id 都靠这个字段：形状变了要同步改，否则停止按钮永远找不到任务。
        let backup = BackupEvent::Started {
            record_id: "b-1".to_string(),
        };
        assert_eq!(started_record_id(&backup).as_deref(), Some("b-1"));
        let container = ContainerEvent::Started {
            record_id: "c-1".to_string(),
        };
        assert_eq!(started_record_id(&container).as_deref(), Some("c-1"));
        let log = BackupEvent::Log {
            level: deploy_core::models::LogLevel::Info,
            message: "开始了".to_string(),
        };
        assert_eq!(started_record_id(&log), None);
        let finished = BackupEvent::Finished {
            record: BackupRecord {
                id: "b-2".to_string(),
                server_id: String::new(),
                server_name: String::new(),
                database: String::new(),
                schema: String::new(),
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
            },
        };
        assert_eq!(started_record_id(&finished), None);
    }

    #[tokio::test]
    async fn aborted_worker_reads_as_stopped() {
        // JoinError 没有公开构造函数，只能真的 abort 一次再取它的错误。
        let handle = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
        handle.abort();
        let joined = handle.await.unwrap_err();
        let err = outcome(Err(joined)).unwrap_err().to_string();
        assert!(err.contains("已停止"), "{err}");
    }

    #[test]
    fn worker_result_passes_straight_through() {
        assert!(outcome(Ok(Ok(()))).is_ok());
        let err = outcome(Ok(Err(CoreError::ssh("连接超时")))).unwrap_err().to_string();
        assert!(err.contains("连接超时"), "{err}");
    }
}
