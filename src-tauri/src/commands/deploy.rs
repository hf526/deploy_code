use deploy_core::models::{DeployRecord, DeployRequest};
use deploy_core::{CoreError, DeployEngine, Result};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{ActiveDeploy, AppState};

/// 在后台执行部署，并把日志/进度/结果通过事件推送给前端。
fn spawn_deploy(
    app: AppHandle,
    engine: DeployEngine,
    record: DeployRecord,
    request: DeployRequest,
) -> String {
    let record_id = record.id.clone();
    let server_id = record.server_id.clone();
    let target_dir = record.target_dir.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    let run_app = app.clone();
    let track_app = app.clone();
    let run_record_id = record_id.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        while let Some(event) = receiver.recv().await {
            let _ = app.emit("deploy://event", &event);
        }
    });

    let handle = tokio::spawn(async move {
        // 等登记完成后再开始，避免部署瞬间结束时残留登记项、阻塞后续部署。
        let _ = start_rx.await;
        engine.run(record, request, Some(sender)).await;
        let state = run_app.state::<AppState>();
        state.untrack_deploy(&run_record_id);
        state.release_deploy_claim();
    });

    track_app.state::<AppState>().track_deploy(
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
    // 先原子占位再 prepare，避免两个并发命令同时通过检查。
    if !state.try_claim_deploy() {
        return Err(CoreError::deploy("已有部署正在进行，请等待完成后再试"));
    }
    let engine = state.engine();
    let record = match engine.prepare(&request) {
        Ok(record) => record,
        Err(err) => {
            state.release_deploy_claim();
            return Err(err);
        }
    };
    Ok(spawn_deploy(app, engine, record, request))
}

/// 按历史记录重新部署（使用记录中的提交号，保证版本一致）。
#[tauri::command(async)]
pub fn redeploy(app: AppHandle, state: State<AppState>, record_id: String) -> Result<String> {
    if !state.try_claim_deploy() {
        return Err(CoreError::deploy("已有部署正在进行，请等待完成后再试"));
    }
    let engine = state.engine();
    let record = match state.store.find_record(&record_id) {
        Ok(record) => record,
        Err(err) => {
            state.release_deploy_claim();
            return Err(err);
        }
    };
    let request = DeployRequest {
        repo_id: record.repo_id.clone(),
        rev: record.commit.clone(),
        server_id: record.server_id.clone(),
        target_dir: record.target_dir.clone(),
        run_scripts: record.run_scripts,
        script_dir: record.script_dir.clone(),
        script: record.script.clone(),
    };
    let prepared = match engine.prepare(&request) {
        Ok(prepared) => prepared,
        Err(err) => {
            state.release_deploy_claim();
            return Err(err);
        }
    };
    Ok(spawn_deploy(app, engine, prepared, request))
}
