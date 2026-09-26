use deploy_core::shutdown::{checked_delay_minutes, CANCEL_WINDOW_SECS};
use deploy_core::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{AppState, PendingShutdown, ShutdownSource};

/// 定时关机状态：设置页摘要与底部状态栏的倒计时都读它。
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownStatus {
    /// 待执行的关机计划；`null` 表示当前没有排定的关机。
    pub pending: Option<PendingShutdown>,
    /// 可取消窗口（秒）：剩余时间少于它时倒计时转为醒目样式。
    /// 阈值只在核心库里定义，这里下发给前端，避免两边各写一份。
    pub cancel_window_secs: i64,
}

fn status_of(state: &AppState) -> ShutdownStatus {
    ShutdownStatus {
        pending: state.pending_shutdown(),
        cancel_window_secs: CANCEL_WINDOW_SECS,
    }
}

/// 推送关机状态变化：排定、取消、真正下发都走这里。
pub(crate) fn emit_status(app: &AppHandle) {
    let status = status_of(&app.state::<AppState>());
    let _ = app.emit("shutdown://status", status);
}

/// 查询当前关机状态：前端重载/冷启动后用它对账，避免看不到已排定的关机。
#[tauri::command(async)]
pub fn get_shutdown_status(state: State<AppState>) -> ShutdownStatus {
    status_of(&state)
}

/// 排定 `minutes` 分钟后关机，覆盖上一个计划。
#[tauri::command(async)]
pub fn schedule_shutdown(
    app: AppHandle,
    state: State<AppState>,
    minutes: u32,
) -> Result<ShutdownStatus> {
    let minutes = checked_delay_minutes(minutes)?;
    let at_ms = chrono::Local::now().timestamp_millis() + i64::from(minutes) * 60_000;
    state.arm_shutdown(PendingShutdown {
        at_ms,
        source: ShutdownSource::Manual,
    });
    emit_status(&app);
    Ok(status_of(&state))
}

/// 取消尚未下发的关机计划。没有计划时也成功返回（可能是另一个窗口先取消了）。
#[tauri::command(async)]
pub fn cancel_shutdown(app: AppHandle, state: State<AppState>) -> Result<ShutdownStatus> {
    state.cancel_shutdown();
    emit_status(&app);
    Ok(status_of(&state))
}
