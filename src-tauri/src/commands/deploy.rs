use std::time::Duration;

use deploy_core::models::{
    elapsed_ms_since, now_string, DeployConfig, DeployEvent, DeployRecord, DeployRequest,
    DeployStatus, RemoteRelease,
};
use deploy_core::{release, CoreError, DeployEngine, Result, Store};
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
        None => return Err(CoreError::busy("已有部署正在进行，请等待完成后再试")),
    };
    let engine = state.engine();
    let record = engine.prepare(&request)?;
    Ok(spawn_deploy(app, engine, record, request, claim))
}

/// 列出保存的部署配置。
#[tauri::command(async)]
pub fn list_deploy_configs(state: State<AppState>) -> Result<Vec<DeployConfig>> {
    Ok(state.store.load_config()?.deploy_configs)
}

/// 新建或更新一条部署配置。
#[tauri::command(async)]
pub fn save_deploy_config(state: State<AppState>, config: DeployConfig) -> Result<DeployConfig> {
    Store::save_deploy_config(&state.store, config)
}

/// 删除一条部署配置（不影响已有部署记录）。
#[tauri::command(async)]
pub fn delete_deploy_config(state: State<AppState>, config_id: String) -> Result<bool> {
    Store::delete_deploy_config(&state.store, &config_id)
}

/// 按保存的部署配置发起部署：参数完全取自配置，避免前端传参与保存内容不一致。
#[tauri::command(async)]
pub fn start_deploy_config(
    app: AppHandle,
    state: State<AppState>,
    config_id: String,
) -> Result<String> {
    // 先抢占名额（进程内原子标记 + 跨进程文件锁）再 prepare，避免并发命令同时通过检查。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Deploy)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有部署正在进行，请等待完成后再试")),
    };
    let engine = state.engine();
    let config = {
        let app_config = state.store.load_config()?;
        Store::find_deploy_config(&app_config, &config_id)?.clone()
    };
    let request = DeployRequest {
        repo_id: config.repo_id,
        rev: config.rev,
        server_id: config.server_id,
        target_dir: config.target_dir,
        run_scripts: config.run_scripts,
        script_dir: config.script_dir,
        scripts: config.scripts,
        upload_env: config.upload_env,
    };
    let record = engine.prepare(&request)?;
    Ok(spawn_deploy(app, engine, record, request, claim))
}

/// 按历史记录重新部署（使用记录中的提交号，保证版本一致）。
#[tauri::command(async)]
pub fn redeploy(app: AppHandle, state: State<AppState>, record_id: String) -> Result<String> {
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Deploy)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有部署正在进行，请等待完成后再试")),
    };
    let engine = state.engine();
    let record = state.store.find_record(&record_id)?;
    let request = DeployRequest {
        repo_id: record.repo_id.clone(),
        // 工作区部署没有固定提交，重新部署时同样打包当前工作区。
        rev: if record.worktree {
            String::new()
        } else {
            record.commit.clone()
        },
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

/// 取消正在进行的部署：中止本地任务、清理远端脚本与临时包，并把记录收敛为「已取消」。
#[tauri::command(async)]
pub async fn cancel_deploy(
    app: AppHandle,
    state: State<'_, AppState>,
    record_id: String,
) -> Result<String> {
    let active = state
        .take_deploy(&record_id)
        .ok_or_else(|| CoreError::deploy("没有正在进行的部署（可能已完成）"))?;

    // 中止本地任务：未来在下一处 await 退出，本地临时文件由 TempArchiveGuard 清理。
    active.abort.abort();
    // 留一点时间给任务退出，避免它与下面的记录写入互相覆盖。
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 中止后任务守卫会摘除 active_deploys；单独登记到 pending_cleanups，
    // 保证取消清理期间退出应用时仍会终止远端脚本。
    state.add_pending_cleanup(&record_id, active.clone());

    // 重连服务器终止远端脚本进程组并清理残留压缩包；清理失败不影响取消结果。
    if let Ok(config) = state.store.load_config() {
        if let Ok(server) = Store::find_server(&config, &active.server_id) {
            let engine = state.engine();
            let _ = tokio::time::timeout(
                Duration::from_secs(15),
                engine.cleanup_remote_script(server, &active.target_dir, &record_id, 5),
            )
            .await;
        }
    }
    state.remove_pending_cleanup(&record_id);

    // 记录收敛为失败（已取消）；引擎任务被中止后不会再写这条记录。
    // 记录可能已被手动删除（或读取失败）：此时不阻断取消流程，界面由命令成功返回收敛。
    if let Ok(mut record) = state.store.find_record(&record_id) {
        if record.status == DeployStatus::Running {
            record.status = DeployStatus::Failed;
            record.error = Some("部署已取消".to_string());
            record.finished_at = Some(now_string());
            record.duration_ms = elapsed_ms_since(&record.started_at);
            record.log = format!("{}[已取消] 用户手动取消了本次部署\n", record.log);
            let limit = state
                .store
                .load_config()
                .map(|config| config.settings.history_limit)
                .unwrap_or(500);
            let _ = state.store.upsert_history(&record, limit);
        }
        // 通过 finished 事件让界面结束 running 状态并刷新历史。
        let _ = app.emit("deploy://event", DeployEvent::Finished { record });
    }
    Ok("已取消".to_string())
}

/// 列出某个部署目录下的历史发布版本（原子发布）。
#[tauri::command(async)]
pub async fn list_releases(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    target_dir: String,
) -> Result<Vec<RemoteRelease>> {
    // 与部署互斥：避免读到正在解压、尚未完成的版本目录。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Deploy)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有部署正在进行，请等待完成后再试")),
    };
    let config = state.store.load_config()?;
    let server = Store::find_server(&config, &server_id)?.clone();
    let releases =
        release::list_releases(&server, &target_dir, config.settings.connect_timeout_secs).await;
    drop(claim);
    releases
}

/// 把 current 软链切换到指定历史版本。
#[tauri::command(async)]
pub async fn rollback_release(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    target_dir: String,
    release_name: String,
) -> Result<String> {
    // 与部署互斥：避免切换版本与正在进行的部署相互踩踏。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Deploy)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有部署正在进行，请等待完成后再试")),
    };
    let config = state.store.load_config()?;
    let server = Store::find_server(&config, &server_id)?.clone();
    let message =
        release::switch_release(&server, &target_dir, &release_name, config.settings.connect_timeout_secs)
            .await;
    drop(claim);
    message
}
