use deploy_core::models::{
    PagesConfig, PagesConfigEntry, PagesDeployRecord, PagesEvent, PagesRequest,
};
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
        None => return Err(CoreError::busy("已有 Pages 部署正在进行，请等待完成后再试")),
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

/// 列出所有已保存的 Pages 配置（按仓库一条，供部署页统一列表展示）。
#[tauri::command(async)]
pub fn list_pages_configs(state: State<AppState>) -> Result<Vec<PagesConfigEntry>> {
    let config = state.store.load_config()?;
    Ok(config
        .repos
        .iter()
        .filter_map(|repo| {
            repo.pages.clone().map(|pages| PagesConfigEntry {
                repo_id: repo.id.clone(),
                repo_name: repo.name.clone(),
                config: pages,
            })
        })
        .collect())
}

/// 删除某个仓库的 Pages 配置（已有的 Pages 部署记录保留）。
#[tauri::command(async)]
pub fn delete_pages_config(state: State<AppState>, repo_id: String) -> Result<bool> {
    state.store.mutate_config(|app| {
        let repo = app
            .repos
            .iter_mut()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {repo_id}")))?;
        Ok(repo.pages.take().is_some())
    })
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
    let normalized = config.normalize();
    if normalized.provider != "cloudflare" && normalized.provider != "github" {
        return Err(CoreError::config(
            "不支持的 Pages 平台（可选 cloudflare / github）",
        ));
    }
    if normalized.provider == "github" {
        if normalized.publish_branch.starts_with('-')
            || normalized
                .publish_branch
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(CoreError::config("发布分支不能包含空白字符或以 - 开头"));
        }
    } else if normalized
        .project_name
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(CoreError::config("项目名不能包含空白字符"));
    }

    state.store.mutate_config(|app| {
        let repo = app
            .repos
            .iter_mut()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {repo_id}")))?;
        // Cloudflare 以项目名为准判断是否启用；GitHub 无项目名，保存配置即视为启用。
        repo.pages = if normalized.provider == "github" || !normalized.project_name.is_empty() {
            Some(normalized.clone())
        } else {
            None
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
