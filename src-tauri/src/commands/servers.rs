use deploy_core::models::{now_string, new_id, ServerConfig};
use deploy_core::tunnel::{assert_local_ports_free, normalize_rules};
use deploy_core::{CoreError, Result, Store};
use tauri::State;

use crate::state::AppState;

#[tauri::command(async)]
pub fn list_servers(state: State<AppState>) -> Result<Vec<ServerConfig>> {
    Ok(state.store.load_config()?.servers)
}

#[tauri::command(async)]
pub fn save_server(state: State<AppState>, mut server: ServerConfig) -> Result<ServerConfig> {
    if server.name.trim().is_empty() {
        return Err(CoreError::config("服务器名称不能为空"));
    }
    if server.host.trim().is_empty() {
        return Err(CoreError::config("主机地址不能为空"));
    }
    if server.username.trim().is_empty() {
        return Err(CoreError::config("SSH 用户名不能为空"));
    }
    if server.port == 0 {
        return Err(CoreError::config("SSH 端口必须在 1-65535 之间"));
    }
    // 默认部署目录会直接填进部署表单，和部署时同样要求服务器上的绝对路径。
    if !server.default_target_dir.trim().is_empty() {
        server.default_target_dir = deploy_core::release::normalize_target(&server.default_target_dir)?;
    }
    if server.id.trim().is_empty() {
        server.id = new_id();
    }
    if server.created_at.trim().is_empty() {
        server.created_at = now_string();
    }
    // 隧道规则与独立的隧道入口共用同一份校验，表单带过来的旧规则也照样规范化。
    server.tunnels = normalize_rules(&server.tunnels)?;

    let (saved, servers) = state.store.mutate_config(|config| {
        // 名称是 CLI / 仓库默认服务器的查找键之一，重名会导致命中错误服务器。
        if config
            .servers
            .iter()
            .any(|item| item.id != server.id && item.name == server.name)
        {
            return Err(CoreError::config(format!("服务器名称已存在: {}", server.name)));
        }
        // 本机端口全局唯一：和别的服务器撞了就不能存。
        assert_local_ports_free(&config.servers, &server.id, &server.tunnels)?;
        Store::upsert_server(config, server.clone())?;
        Ok((server.clone(), config.servers.clone()))
    })?;
    // 配置已落盘：按新规则对齐隧道（规则变了的重建监听，删掉的让出端口）。
    state.tunnels.sync(&servers);
    Ok(saved)
}

#[tauri::command(async)]
pub fn delete_server(state: State<AppState>, server_id: String) -> Result<()> {
    let servers = state.store.mutate_config(|config| {
        config.servers.retain(|server| server.id != server_id);
        // 服务器已删除，其备份配置不再可用（备份记录保留作历史）。
        let removed: Vec<String> = config
            .backup_configs
            .iter()
            .filter(|saved| saved.server_id == server_id)
            .map(|saved| saved.id.clone())
            .collect();
        config.backup_configs.retain(|saved| saved.server_id != server_id);
        // 服务器已删除：从各部署配置的目标列表里摘掉这一台，没有剩余目标的配置整条删除。
        Store::detach_server_from_deploy_configs(config, &server_id);
        // 容器备份配置同样跟着收敛：来源被删的整条删除，只当过迁移目标的降级为纯备份。
        Store::detach_server_from_container_configs(config, &server_id);
        // 定时备份若引用被删配置，清空引用，避免每天到点报错。
        if config
            .settings
            .scheduled_backup_config_id
            .as_ref()
            .is_some_and(|config_id| removed.contains(config_id))
        {
            config.settings.scheduled_backup_config_id = None;
        }
        for repo in config.repos.iter_mut() {
            if repo.default_server_id.as_deref() == Some(server_id.as_str()) {
                repo.default_server_id = None;
            }
        }
        Ok(config.servers.clone())
    })?;
    // 这台机器的隧道监听必须马上收掉，否则本机端口会一直被已删除的服务器占着。
    state.tunnels.sync(&servers);
    Ok(())
}

/// 测试连接（可以直接测试未保存的表单配置）。
#[tauri::command]
pub async fn test_server(state: State<'_, AppState>, server: ServerConfig) -> Result<String> {
    state.engine().test_server(&server).await
}

/// 采集服务器安全检查报告（登录失败、成功登录、防火墙、sshd 配置）。
#[tauri::command]
pub async fn scan_server_security(
    state: State<'_, AppState>,
    server: ServerConfig,
) -> Result<deploy_core::SecurityReport> {
    state.engine().server_security(&server).await
}

/// 拉黑来源 IP。
#[tauri::command]
pub async fn block_server_ip(
    state: State<'_, AppState>,
    server: ServerConfig,
    ip: String,
) -> Result<String> {
    state.engine().block_server_ip(&server, &ip).await
}

/// 解除来源 IP 的拉黑。
#[tauri::command]
pub async fn unblock_server_ip(
    state: State<'_, AppState>,
    server: ServerConfig,
    ip: String,
) -> Result<String> {
    state.engine().unblock_server_ip(&server, &ip).await
}

/// 强制踢出某个在线会话。
#[tauri::command]
pub async fn kick_server_session(
    state: State<'_, AppState>,
    server: ServerConfig,
    tty: String,
) -> Result<String> {
    state.engine().kick_server_session(&server, &tty).await
}

/// 启用服务器端自动防护（失败 N 次后自动拉黑，长期生效）。
/// `whitelist` 是免封白名单（单个 IP），整份随本次下发覆盖服务器那份配置。
#[tauri::command]
pub async fn enable_server_guard(
    state: State<'_, AppState>,
    server: ServerConfig,
    threshold: u32,
    window_mins: u64,
    whitelist: Vec<String>,
) -> Result<String> {
    state
        .engine()
        .enable_server_guard(&server, threshold, window_mins, &whitelist)
        .await
}

/// 停用服务器端自动防护。
#[tauri::command]
pub async fn disable_server_guard(
    state: State<'_, AppState>,
    server: ServerConfig,
) -> Result<String> {
    state.engine().disable_server_guard(&server).await
}
