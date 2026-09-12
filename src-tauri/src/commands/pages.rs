use deploy_core::models::{PagesConfig, PagesDeployRecord, PagesEvent, PagesRequest};
use deploy_core::{CoreError, PagesEngine, PreparedPagesDeploy, Result, Store};
use tauri::{AppHandle, Emitter, State};

use crate::state::{AppState, ClaimGuard, ClaimKind, PagesTaskGuard};

/// 在阻塞线程执行 Pages 部署，并把日志/结果通过事件推送给前端。
fn spawn_pages(
    app: AppHandle,
    engine: PagesEngine,
    prepared: PreparedPagesDeploy,
    skip_build: bool,
    claim: ClaimGuard,
) -> String {
    let record_id = prepared.record.id.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<PagesEvent>();

    let emit_app = app.clone();
    tauri::async_runtime::spawn(async move {
        // 守卫移入事件转发任务：等 engine.run 返回、事件全部转发完再释放抢占标记，
        // 避免紧接着启动的新任务收到上一轮残留的 finished 事件。
        let _guard = PagesTaskGuard::new(claim);
        while let Some(event) = receiver.recv().await {
            let _ = emit_app.emit("pages://event", &event);
        }
    });

    tauri::async_runtime::spawn_blocking(move || {
        engine.run(
            prepared.record,
            prepared.config,
            prepared.repo_path,
            prepared.token,
            prepared.account_id,
            skip_build,
            Some(sender),
        );
    });

    record_id
}

#[tauri::command(async)]
pub fn start_pages_deploy(
    app: AppHandle,
    state: State<AppState>,
    request: PagesRequest,
) -> Result<String> {
    // 先抢占名额（进程内原子标记 + 跨进程文件锁）再 prepare，避免并发命令同时通过检查。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Pages)? {
        Some(claim) => claim,
        None => return Err(CoreError::config("已有 Pages 部署正在进行，请等待完成后再试")),
    };
    let engine = PagesEngine::new(state.store.clone());
    let prepared = engine.prepare(&request)?;
    Ok(spawn_pages(app, engine, prepared, request.skip_build, claim))
}

#[tauri::command(async)]
pub fn list_pages_records(
    state: State<AppState>,
    repo_id: Option<String>,
) -> Result<Vec<PagesDeployRecord>> {
    let mut records = state.store.load_pages_records()?;
    records.reverse();
    if let Some(repo_id) = repo_id.filter(|value| !value.is_empty()) {
        records.retain(|record| record.repo_id == repo_id);
    }
    Ok(records)
}

#[tauri::command(async)]
pub fn get_pages_record(state: State<AppState>, record_id: String) -> Result<PagesDeployRecord> {
    state.store.find_pages_record(&record_id)
}

#[tauri::command(async)]
pub fn delete_pages_record(state: State<AppState>, record_id: String) -> Result<bool> {
    state.store.remove_pages_record(&record_id)
}

#[tauri::command(async)]
pub fn clear_pages_records(state: State<AppState>) -> Result<()> {
    state.store.clear_pages_records()
}

#[tauri::command(async)]
pub fn get_pages_config(state: State<AppState>, repo_id: String) -> Result<PagesConfig> {
    let config = state.store.load_config()?;
    let repo = Store::find_repo(&config, &repo_id)?;
    Ok(repo.pages.clone().unwrap_or_default())
}

#[tauri::command(async)]
pub fn save_pages_config(
    state: State<AppState>,
    repo_id: String,
    config: PagesConfig,
) -> Result<PagesConfig> {
    let mut normalized = config;
    normalized.project_name = normalized.project_name.trim().to_string();
    normalized.build_command = normalized.build_command.trim().to_string();
    normalized.output_dir = normalized.output_dir.trim().to_string();
    normalized.branch = normalized.branch.trim().to_string();
    if normalized.output_dir.is_empty() {
        normalized.output_dir = "dist".to_string();
    }
    if normalized.branch.is_empty() {
        normalized.branch = "main".to_string();
    }
    if normalized.project_name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CoreError::config("项目名不能包含空白字符"));
    }

    state.store.mutate_config(|app| {
        let repo = app
            .repos
            .iter_mut()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {repo_id}")))?;
        repo.pages = if normalized.project_name.is_empty() {
            None
        } else {
            Some(normalized.clone())
        };
        Ok(())
    })?;
    Ok(normalized)
}

/// 检查 wrangler / Token / Account 是否可用。
#[tauri::command]
pub async fn test_pages(
    state: State<'_, AppState>,
    repo_id: String,
    config: Option<PagesConfig>,
) -> Result<String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || PagesEngine::new(store).test(&repo_id, config))
        .await
        .map_err(|e| CoreError::Process(format!("检查任务异常: {e}")))?
}
