use deploy_core::models::{ContainerEvent, ContainerRecord, ContainerRequest, ContainerRestoreRequest};
use deploy_core::{ContainerEngine, ContainerJob, ContainerPlan, CoreError, Result, Store};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{ActiveContainer, AppState, ClaimGuard, ClaimKind, ContainerTaskGuard};

fn resolve_server(state: &AppState, server_id: &str) -> Result<deploy_core::ServerConfig> {
    let config = state.store.load_config()?;
    Ok(Store::find_server(&config, server_id)?.clone())
}

/// 任务涉及的服务器（来源与目标可能都有），取消与退出清理要逐个把它们的外壳文件清掉。
fn plan_servers(plan: &ContainerPlan) -> Vec<String> {
    let mut ids = vec![plan.source.id.clone()];
    if let Some((target, _, _)) = &plan.target {
        if !ids.contains(&target.id) {
            ids.push(target.id.clone());
        }
    }
    ids
}

/// 在后台执行容器备份 / 迁移，并把日志/进度/结果推给前端。
fn spawn_container(
    app: AppHandle,
    engine: ContainerEngine,
    record: ContainerRecord,
    job: ContainerJob,
    claim: ClaimGuard,
) -> String {
    let record_id = record.id.clone();
    let server_ids = match &job {
        ContainerJob::Snapshot(plan) | ContainerJob::Restore(plan) => plan_servers(plan),
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<ContainerEvent>();

    let emit_app = app.clone();
    let guard_app = app.clone();
    let run_record_id = record_id.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        // 守卫移入事件转发任务：等 engine.run 返回、事件全部转发完再释放抢占标记，
        // 避免紧接着启动的新任务收到上一轮残留的 finished 事件。
        let _guard = ContainerTaskGuard::new(guard_app, run_record_id, claim);
        while let Some(event) = receiver.recv().await {
            let _ = emit_app.emit("container://event", &event);
        }
    });

    let handle = tokio::spawn(async move {
        // 等登记完成后再开始，避免任务瞬间结束时残留登记项、阻塞下一次。
        let _ = start_rx.await;
        engine.run(record, job, Some(sender)).await;
    });

    app.state::<AppState>().track_container(
        &record_id,
        ActiveContainer {
            server_ids,
            record_id: record_id.clone(),
            abort: handle.abort_handle(),
        },
    );
    let _ = start_tx.send(());

    record_id
}

/// 备份包目录：只读展示与「打开目录」用，命令本身不占任务名额。
#[tauri::command(async)]
pub fn get_container_backup_dir(state: State<AppState>) -> Result<String> {
    let engine = ContainerEngine::new(state.store.clone());
    let dir = engine.bundle_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io_path(&dir, e))?;
    Ok(dir.to_string_lossy().into_owned())
}

/// 扫描某台服务器上的 compose 项目。
///
/// 发现类命令刻意不占容器任务名额：迁移一跑就是几十分钟，不能因此连看都看不了；
/// 这些调用只读 `docker ps` / `compose ls`，与正在进行的任务不冲突。
#[tauri::command(async)]
pub async fn list_compose_stacks(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<deploy_core::ComposeStack>> {
    let server = resolve_server(&state, &server_id)?;
    ContainerEngine::new(state.store.clone())
        .list_stacks(&server)
        .await
}

/// 解析一个 compose 项目的服务 / 数据卷 / 镜像与预检提示。
#[tauri::command(async)]
pub async fn inspect_compose_stack(
    state: State<'_, AppState>,
    server_id: String,
    project: String,
) -> Result<deploy_core::ComposeStackDetail> {
    let server = resolve_server(&state, &server_id)?;
    ContainerEngine::new(state.store.clone())
        .inspect_stack(&server, &project)
        .await
}

/// 发起一次容器快照（`request.target` 有值时接着恢复到目标服务器）。
#[tauri::command(async)]
pub fn start_container_transfer(
    app: AppHandle,
    state: State<AppState>,
    request: ContainerRequest,
) -> Result<String> {
    // 先抢占名额（进程内原子标记 + 跨进程文件锁）再 prepare，避免并发命令同时通过检查。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Container)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有容器任务正在进行，请等待完成后再试")),
    };
    let engine = ContainerEngine::new(state.store.clone());
    let (record, job) = engine.prepare(&request)?;
    Ok(spawn_container(app, engine, record, job, claim))
}

/// 用本机已有的备份包恢复到一台服务器。
#[tauri::command(async)]
pub fn restore_container_bundle(
    app: AppHandle,
    state: State<AppState>,
    request: ContainerRestoreRequest,
) -> Result<String> {
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Container)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有容器任务正在进行，请等待完成后再试")),
    };
    let engine = ContainerEngine::new(state.store.clone());
    let (record, job) = engine.prepare_restore(&request)?;
    Ok(spawn_container(app, engine, record, job, claim))
}

/// 取消正在进行的容器任务：中止本地任务、清理两台服务器上的工作目录，记录收敛为已取消。
#[tauri::command(async)]
pub async fn cancel_container(
    app: AppHandle,
    state: State<'_, AppState>,
    record_id: String,
) -> Result<String> {
    let active = state
        .take_container(&record_id)
        .ok_or_else(|| CoreError::backup("没有正在进行的容器任务（可能已完成）"))?;
    // 登记项刚被 take_container 摘掉：另记一份，保证下面这段清理期间退出应用
    // 仍会把两台服务器上的备份包收掉（否则 tar 就留在那台机器上了）。
    state.add_pending_container_cleanup(&record_id, active.clone());

    active.abort.abort();
    // 留一点时间给任务退出，避免它与下面的记录写入互相覆盖。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    if let Ok(config) = state.store.load_config() {
        let engine = ContainerEngine::new(state.store.clone());
        for server_id in &active.server_ids {
            if let Ok(server) = Store::find_server(&config, server_id) {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    engine.cleanup_remote(server, &record_id),
                )
                .await;
            }
        }
    }

    state.remove_pending_container_cleanup(&record_id);

    if let Ok(mut record) = state.store.find_container_record(&record_id) {
        if record.status == deploy_core::DeployStatus::Running {
            record.status = deploy_core::DeployStatus::Failed;
            record.error = Some("任务已取消".to_string());
            record.finished_at = Some(deploy_core::models::now_string());
            record.duration_ms = deploy_core::models::elapsed_ms_since(&record.started_at);
            record.log = format!("{}[已取消] 用户停止了本次任务\n", record.log);
            let limit = state
                .store
                .load_config()
                .map(|config| config.settings.container_history_limit)
                .unwrap_or(200);
            let _ = state.store.upsert_container(&record, limit);
        }
        let _ = app.emit("container://event", ContainerEvent::Finished { record });
    }
    Ok("已取消".to_string())
}

/// 记录列表：新 → 旧，界面直接渲染。
#[tauri::command(async)]
pub fn list_container_records(state: State<AppState>) -> Result<Vec<ContainerRecord>> {
    let mut records = state.store.load_containers()?;
    records.reverse();
    Ok(records)
}

#[tauri::command(async)]
pub fn delete_container_record(state: State<AppState>, record_id: String) -> Result<bool> {
    state.store.remove_container_record(&record_id)
}

/// 清空记录只删历史，本机的备份包文件留在原处。
#[tauri::command(async)]
pub fn clear_container_records(state: State<AppState>) -> Result<()> {
    state.store.clear_container_records()
}
