use deploy_core::models::{DeployRecord, DeployRequest};
use deploy_core::{CoreError, DeployEngine, Result};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{ActiveDeploy, AppState, ClaimGuard, ClaimKind, DeployTaskGuard};

/// 在后台执行部署，并把日志/进度/结果通过事件推送给前端。
fn spawn_deploy(
    app: AppHandle,
    engine: DeployEngine,
    record: DeployRecord,
    request: DeployRequest,
    claim: ClaimGuard,
) -> String {
    let record_id = record.id.clone();
    let server_id = record.server_id.clone();
    let target_dir = record.target_dir.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    let emit_app = app.clone();
    let guard_app = app.clone();
    let run_record_id = record_id.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        // 守卫移入事件转发任务：等 engine.run 返回、事件全部转发完再释放抢占标记，
        // 避免紧接着启动的新任务收到上一轮残留的 finished 事件。
        let _guard = DeployTaskGuard::new(guard_app, run_record_id, claim);
        while let Some(event) = receiver.recv().await {
            let _ = emit_app.emit("deploy://event", &event);
        }
    });

    let handle = tokio::spawn(async move {
        // 等登记完成后再开始，避免部署瞬间结束时残留登记项、阻塞后续部署。
        let _ = start_rx.await;
        engine.run(record, request, Some(sender)).await;
    });

    app.state::<AppState>().track_deploy(
        &record_id,
        ActiveDeploy {
            server_id,
            target_dir,
            abort: handle.abort_handle(),
        },
    );
    let _ = start_tx.send(());

    record_id
}

#[tauri::command(async)]
pub fn start_deploy(
    app: AppHandle,
    state: State<AppState>,
    request: DeployRequest,
) -> Result<String> {
    // 先抢占名额（进程内原子标记 + 跨进程文件锁）再 prepare，避免并发命令同时通过检查。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Deploy)? {
        Some(claim) => claim,
        None => return Err(CoreError::deploy("已有部署正在进行，请等待完成后再试")),
    };
    let engine = state.engine();
    let record = engine.prepare(&request)?;
    Ok(spawn_deploy(app, engine, record, request, claim))
}

/// 按历史记录重新部署（使用记录中的提交号，保证版本一致）。
#[tauri::command(async)]
pub fn redeploy(app: AppHandle, state: State<AppState>, record_id: String) -> Result<String> {
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Deploy)? {
        Some(claim) => claim,
        None => return Err(CoreError::deploy("已有部署正在进行，请等待完成后再试")),
    };
    let engine = state.engine();
    let record = state.store.find_record(&record_id)?;
    let request = DeployRequest {
        repo_id: record.repo_id.clone(),
        rev: record.commit.clone(),
        server_id: record.server_id.clone(),
        target_dir: record.target_dir.clone(),
        run_scripts: record.run_scripts,
        script_dir: record.script_dir.clone(),
        scripts: record.scripts.clone(),
        upload_env: true,
    };
    let prepared = engine.prepare(&request)?;
    Ok(spawn_deploy(app, engine, prepared, request, claim))
}
