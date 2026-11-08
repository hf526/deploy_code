//! 文件传输命令：都走 `hostlink` 那条已在线的常驻会话，开一路 SFTP 而不是重新认证一次。
//!
//! 业务逻辑都在 `deploy_core::transfer`，这一层只做参数校验、会话查找与事件转发。
//! 与交互式终端同例：**不占任务名额**，也不参与退出清理——会话一收（`hosts.stop_all()`），
//! 在传的那条路自己就断了。代价要说清：那种情况下远端的 `.part` 临时名没人删，得人工收
//! （见 `deploy_core::transfer` 模块头第 2 条）。
//!
//! 这里刻意**不**接 `rz` / `sz`：ZMODEM 要终端在 PTY 上现场扮演协议对端，本项目只做「用户
//! 真正要的那件事」，走 SFTP。远端真跑了 `rz`，`terminal://event` 会发一条 `zmodemHint`。

use std::path::PathBuf;
use std::sync::Arc;

use deploy_core::ssh::SshClient;
use deploy_core::{CoreError, Result, Store, TransferDirection, TransferInfo};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// 事件名与前端订阅处对齐（`transfer://event`）。
pub const EVENT_TOPIC: &str = "transfer://event";

/// 这一台当前在线的那条常驻会话 + 它的展示名。
///
/// 没在线就直接报错并指路（去服务器页重连），不偷偷另建一条连接——那正是交互式终端那条理由：
/// 每次认证都堆在服务器的自动防护眼前。
fn live_client(state: &AppState, server_id: &str) -> Result<(Arc<SshClient>, String)> {
    let config = state.store.load_config()?;
    let server = Store::find_server(&config, server_id)?.clone();
    let client = state.hosts.client(server_id).ok_or_else(|| {
        CoreError::ssh(format!(
            "{} 的常驻连接还没在线，请先在服务器页重连",
            server.name
        ))
    })?;
    Ok((client, server.name))
}

/// 这一台登录后默认落在哪个目录，供界面把「远端路径」填成一个能改的起点。
#[tauri::command(async)]
pub async fn remote_home(state: State<'_, AppState>, server_id: String) -> Result<String> {
    let (client, _) = live_client(&state, &server_id)?;
    client.remote_home().await
}

/// 本机文件上传到远端。`remote_path` 可以是目录（以 / 结尾）也可以是完整文件名。
#[tauri::command(async)]
pub async fn start_upload(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    local_path: String,
    remote_path: String,
) -> Result<TransferInfo> {
    let (client, server_name) = live_client(&state, &server_id)?;
    let transfers = state.transfers.clone();
    let emit_app = app.clone();
    transfers
        .start(
            client,
            &server_id,
            &server_name,
            TransferDirection::Upload,
            PathBuf::from(local_path),
            remote_path,
            move |event| {
                let _ = emit_app.emit(EVENT_TOPIC, &event);
            },
        )
        .await
}

/// 远端文件下载到本机。`local_path` 是界面另存为选出来的完整本机路径。
#[tauri::command(async)]
pub async fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    remote_path: String,
    local_path: String,
) -> Result<TransferInfo> {
    let (client, server_name) = live_client(&state, &server_id)?;
    let transfers = state.transfers.clone();
    let emit_app = app.clone();
    transfers
        .start(
            client,
            &server_id,
            &server_name,
            TransferDirection::Download,
            PathBuf::from(local_path),
            remote_path,
            move |event| {
                let _ = emit_app.emit(EVENT_TOPIC, &event);
            },
        )
        .await
}

/// 取消一次传输。返回值里可能带着「已取消，但某个临时文件没能删掉」——那句话必须让用户看到。
#[tauri::command(async)]
pub async fn cancel_transfer(state: State<'_, AppState>, transfer_id: String) -> Result<()> {
    state.transfers.cancel(&transfer_id).await
}

/// 还在进行的传输：界面重载后拿它对账，否则进度条会凭空消失而远端还在收文件。
#[tauri::command(async)]
pub fn list_transfers(state: State<'_, AppState>) -> Vec<TransferInfo> {
    state.transfers.list()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前端 `useTauriEvent("transfer://event", ...)` 订阅的就是这一个字符串，写错了不会
    /// 编译报错，只会「传输在进行而界面一格都不动」。
    #[test]
    fn event_topic_matches_the_frontend_subscription() {
        assert_eq!(EVENT_TOPIC, "transfer://event");
    }

    /// 没登记过的服务器 id：要指名找不到，而不是「连接没在线」——后者会让人白去重连一次。
    #[test]
    fn an_unknown_server_is_reported_as_missing() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-transfer-cmd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.subsec_nanos())
                .unwrap_or_default()
        ));
        let state = AppState::new(Arc::new(Store::new(dir)));
        // 不用 expect_err：Ok 那一头带着 Arc<SshClient>，它没有 Debug。
        let err = match live_client(&state, "没有这台") {
            Ok(_) => panic!("没有登记过这台服务器，不该拿到会话"),
            Err(err) => err,
        };
        assert!(matches!(err, CoreError::NotFound(_)), "{err}");
    }
}
