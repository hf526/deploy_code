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

/// 改动配置的 Nginx 操作需要独占，避免两处同时 `nginx -t` + 回滚互相踩。
/// 只读查询（列容器 / 列文件 / 读文件）不占这个名额：一台连不上的机器会把
/// 独占握到 SSH 超时为止，那样整个面板都点不动，与「短操作不占任务锁」的约定相反。
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
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<NginxContainerInfo>> {
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .list_containers(&server)
        .await
}

#[tauri::command(async)]
pub async fn list_nginx_configs(
    state: State<'_, AppState>,
    server_id: String,
    container: String,
    dir: String,
) -> Result<Vec<NginxConfigFile>> {
    let server = resolve_server(&state, &server_id)?;
    NginxEngine::new(state.store.clone())
        .list_configs(&server, &container, &dir)
        .await
}

#[tauri::command(async)]
pub async fn read_nginx_config(
    state: State<'_, AppState>,
    server_id: String,
    container: String,
    dir: String,
    name: String,
) -> Result<NginxConfigContent> {
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
