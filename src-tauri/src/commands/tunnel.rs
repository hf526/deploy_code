//! SSH 隧道命令：规则落盘 + 本机端口转发的运行时控制。
//!
//! 业务逻辑都在 `deploy_core::tunnel`，这一层只做参数校验、登记与状态读取。

use deploy_core::models::{ServerConfig, TunnelRule};
use deploy_core::tunnel::{assert_local_ports_free, normalize_rules};
use deploy_core::{Result, ServerTunnelStatus, Store};
use tauri::State;

use crate::state::AppState;

/// 保存某台服务器的隧道规则：校验后写盘，并立刻按新规则重建监听。
///
/// 只收服务器 id 与规则列表，不收整份服务器配置——避免表单里的旧凭据被顺带覆盖回去。
#[tauri::command(async)]
pub fn save_server_tunnels(
    state: State<AppState>,
    server_id: String,
    tunnels: Vec<TunnelRule>,
) -> Result<ServerConfig> {
    let rules = normalize_rules(&tunnels)?;
    let (saved, servers) = state.store.mutate_config(|config| {
        let mut server = Store::find_server(config, &server_id)?.clone();
        // 本机端口是全局唯一的：别的服务器已经占了同一个端口就拒绝保存。
        assert_local_ports_free(&config.servers, &server_id, &rules)?;
        server.tunnels = rules.clone();
        Store::upsert_server(config, server.clone())?;
        Ok((server, config.servers.clone()))
    })?;
    state.tunnels.sync(&servers);
    Ok(saved)
}

/// 当前在跑的隧道状态（每台一条，含每条规则的监听情况）。
#[tauri::command(async)]
pub fn list_tunnel_status(state: State<AppState>) -> Vec<ServerTunnelStatus> {
    state.tunnels.status()
}

/// 手动重连这一台：本机端口重绑 + 重新建立 SSH 会话。
#[tauri::command(async)]
pub fn reconnect_tunnels(state: State<AppState>, server_id: String) -> Result<()> {
    let config = state.store.load_config()?;
    let server = Store::find_server(&config, &server_id)?.clone();
    state.tunnels.restart(&server)
}
