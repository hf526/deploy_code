use deploy_core::{
    NginxConfigContent, NginxConfigFile, NginxContainerInfo, NginxEngine, Result, ServerConfig,
    Store,
};
use tauri::{AppHandle, State};

use crate::state::{AppState, ClaimGuard, ClaimKind};

fn resolve_server(state: &AppState, server_id: &str) -> Result<ServerConfig> {
    let config = state.store.load_config()?;
    Ok(Store::find_server(&config, server_id)?.clone())
}

/// Nginx 操作需要独占锁，避免并发修改配置导致冲突。
macro_rules! nginx_lock_guard {
    ($app:expr) => {
        match ClaimGuard::acquire(&$app, ClaimKind::Nginx)? {
            Some(claim) => claim,
            None => {
                return Err(deploy_core::CoreError::busy(
                    "已有 Nginx 操作正在进行，请等待完成后再试",
                ));
            }
        }
    };
}

#[tauri::command(async)]
pub async fn list_nginx_containers(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<NginxContainerInfo>> {
    // 获取 Nginx 独占锁
    let _guard = nginx_lock_guard!(app);
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .list_containers(&server)
        .await
}

#[tauri::command(async)]
pub async fn list_nginx_configs(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    container: String,
    dir: String,
) -> Result<Vec<NginxConfigFile>> {
    // 获取 Nginx 独占锁
    let _guard = nginx_lock_guard!(app);
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .list_configs(&server, &container, &dir)
        .await
}

#[tauri::command(async)]
pub async fn read_nginx_config(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    container: String,
    dir: String,
    name: String,
) -> Result<NginxConfigContent> {
    // 获取 Nginx 独占锁
    let _guard = nginx_lock_guard!(app);
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .read_config(&server, &container, &dir, &name)
        .await
}

#[tauri::command(async)]
pub async fn save_nginx_config(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    container: String,
    dir: String,
    name: String,
    content: String,
) -> Result<String> {
    // 获取 Nginx 独占锁
    let _guard = nginx_lock_guard!(app);
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .save_config(&server, &container, &dir, &name, &content)
        .await
}

#[tauri::command(async)]
pub async fn delete_nginx_config(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    container: String,
    dir: String,
    name: String,
) -> Result<String> {
    // 获取 Nginx 独占锁
    let _guard = nginx_lock_guard!(app);
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .delete_config(&server, &container, &dir, &name)
        .await
}

#[tauri::command(async)]
pub async fn reload_nginx(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    container: String,
) -> Result<String> {
    // 获取 Nginx 独占锁
    let _guard = nginx_lock_guard!(app);
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .reload(&server, &container)
        .await
}
