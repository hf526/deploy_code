//! 交互式终端命令：在常驻会话上开一条 PTY 通道，输出走 `terminal://event`。
//!
//! 业务逻辑都在 `deploy_core::terminal`，这一层只做参数校验、事件转发与登记。
//! 终端**不占任务名额**：它只是已有会话上多出来的一条通道，跟部署/备份/容器任务不互斥，
//! 所以也不参与退出清理——会话一收（`hosts.stop_all()`）远端那些 shell 就随传输一起没了。

use deploy_core::{CoreError, Result, Store, TerminalInfo};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// 终端窗口的合理边界：0 列的 PTY 会让远端 shell 的行编辑直接坏掉，而几千列是界面算错了。
const MIN_COLS: u16 = 20;
const MIN_ROWS: u16 = 4;
const MAX_DIM: u16 = 1000;

fn check_dims(cols: u16, rows: u16) -> Result<()> {
    if !(MIN_COLS..=MAX_DIM).contains(&cols) || !(MIN_ROWS..=MAX_DIM).contains(&rows) {
        return Err(CoreError::config(format!(
            "终端尺寸不在范围内（宽 {MIN_COLS}-{MAX_DIM}、高 {MIN_ROWS}-{MAX_DIM}），请缩放窗口后重试"
        )));
    }
    Ok(())
}

/// 打开一个终端标签：复用这一台当前在线的那条常驻会话，不再认证一次。
///
/// 会话没在线就直接报错并指路（去服务器页重连），不偷偷自己建一条连接——那样每次开标签
/// 都会多一次认证，而服务器上的自动防护正是按认证次数判事的。
#[tauri::command(async)]
pub async fn open_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalInfo> {
    check_dims(cols, rows)?;
    let config = state.store.load_config()?;
    let server = Store::find_server(&config, &server_id)?.clone();
    let client = state.hosts.client(&server_id).ok_or_else(|| {
        CoreError::ssh(format!(
            "{} 的常驻连接还没在线，请先在服务器页重连",
            server.name
        ))
    })?;

    let terminals = state.terminals.clone();
    let emit_app = app.clone();
    terminals
        .open(client, &server.id, &server.name, cols, rows, move |event| {
            let _ = emit_app.emit(EVENT_TOPIC, &event);
        })
        .await
}

/// 把界面敲进来或粘贴进来的文本送到远端。
///
/// 入参用字符串而不是 base64：键盘输入本来就是文本，JSON 直接带 UTF-8 过去，回到 Rust 侧
/// `into_bytes()` 就是远端要的那几个字节。输出反过来必须 base64——见 `TerminalEvent::Data`。
#[tauri::command(async)]
pub async fn send_terminal_input(
    state: State<'_, AppState>,
    terminal_id: String,
    data: String,
) -> Result<()> {
    state.terminals.send_input(&terminal_id, data.into_bytes()).await
}

/// 窗口尺寸变了要报给远端：不报的话 vim/top 会按旧尺寸画，出现残影和错行。
#[tauri::command(async)]
pub async fn resize_terminal(
    state: State<'_, AppState>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    check_dims(cols, rows)?;
    state.terminals.resize(&terminal_id, cols, rows).await
}

/// 关闭一个标签。
#[tauri::command(async)]
pub async fn close_terminal(state: State<'_, AppState>, terminal_id: String) -> Result<()> {
    state.terminals.close(&terminal_id).await
}

/// 当前还活着的标签：界面重载后拿它对账，避免留下一个点了没反应的空白标签。
#[tauri::command(async)]
pub fn list_terminals(state: State<'_, AppState>) -> Vec<TerminalInfo> {
    state.terminals.list()
}

/// 事件名与前端订阅处对齐（`terminal://event`）。
pub const EVENT_TOPIC: &str = "terminal://event";

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::TerminalEvent;

    #[test]
    fn zero_sized_terminal_is_rejected_with_a_way_out() {
        let err = check_dims(0, 24).expect_err("0 列要拒绝");
        assert!(matches!(err, CoreError::Config(_)), "{err}");
        assert!(err.to_string().contains("缩放窗口"), "要说下一步怎么办：{err}");
        assert!(check_dims(80, 24).is_ok());
        assert!(check_dims(MIN_COLS, MIN_ROWS).is_ok());
        assert!(check_dims(MAX_DIM, MAX_DIM).is_ok());
        assert!(check_dims(MAX_DIM + 1, 24).is_err());
    }

    /// 前端 `useTauriEvent("terminal://event", ...)` 订阅的就是这一个字符串，写错了不会
    /// 编译报错，只会「终端打开了但一个字都不出」。
    #[test]
    fn event_topic_matches_the_frontend_subscription() {
        assert_eq!(EVENT_TOPIC, "terminal://event");
    }

    /// 事件枚举的 JSON 形状要和 `src/lib/types/terminal.ts` 对得上：字段名靠
    /// `rename_all_fields`，漏了就前端读到 undefined（同 hostlink 那个口径）。
    #[test]
    fn terminal_event_json_shape_matches_the_frontend_types() {
        let opened = serde_json::to_value(TerminalEvent::Opened {
            id: "t1".to_string(),
            server_id: "s1".to_string(),
            server_name: "深圳".to_string(),
        })
        .expect("序列化");
        assert_eq!(opened["type"], "opened");
        assert_eq!(opened["serverId"], "s1");

        let data = serde_json::to_value(TerminalEvent::Data {
            id: "t1".to_string(),
            bytes: "AA==".to_string(),
        })
        .expect("序列化");
        assert_eq!(data["type"], "data");
        assert_eq!(data["bytes"], "AA==");

        let closed = serde_json::to_value(TerminalEvent::Closed {
            id: "t1".to_string(),
            reason: None,
        })
        .expect("序列化");
        assert_eq!(closed["type"], "closed");
        assert!(closed.get("reason").is_some());
    }
}
