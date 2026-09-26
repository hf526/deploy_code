use std::time::Duration;

use chrono::{Local, NaiveTime, TimeZone};
use deploy_core::models::{BackupRequest, Settings};
use deploy_core::shutdown::{request_shutdown, CANCEL_WINDOW_SECS, OS_GRACE_SECS};
use deploy_core::CoreError;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::backup::start_backup;
use crate::commands::shutdown::emit_status;
use crate::state::{AppState, PendingShutdown, ShutdownSource};

/// 定时检查间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(20);
/// 排定关机后的轮询间隔：20 秒的粒度会让关机点最多晚 20 秒，倒计时也会跳格。
const COUNTDOWN_INTERVAL: Duration = Duration::from_secs(1);
/// 到点后若因已有任务占用而无法启动，最多重试的时长。
const RETRY_WINDOW_MINUTES: i64 = 10;

/// 定时任务通知（前端转成 toast）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerNotice {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// 启动定时调度器：应用运行期间（含窗口隐藏到托盘）按设置的时间点自动执行备份与关机。
///
/// 只在软件运行时生效；应用启动前已错过的时间点不补跑。
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_tick = Local::now();
        // 已到点但因其它备份占用未能启动：记录「发现时刻」与所属调度日期，window 内继续重试。
        let mut pending: Option<(chrono::DateTime<Local>, chrono::NaiveDate)> = None;
        // 已成功触发的调度日期（不是完成日期）：防止时钟回拨后同一个调度日重复执行。
        let mut last_run: Option<chrono::NaiveDate> = None;
        // 已排过关机的调度日期：同一天只排一次，用户取消后当天也不再排。
        let mut shutdown_last_run: Option<chrono::NaiveDate> = None;

        loop {
            tokio::time::sleep(tick_interval(&app)).await;
            let now = Local::now();

            // 关机不依赖配置读取：即使 config.json 这一 tick 恰好读不出来也要到点执行。
            fire_due_shutdown(&app, now);

            let Ok(config) = app.state::<AppState>().store.load_config() else {
                last_tick = now;
                continue;
            };
            let settings = config.settings.clone();

            arm_scheduled_shutdown(&app, &settings, now, last_tick, &mut shutdown_last_run);

            if !settings.scheduled_backup_enabled {
                pending = None;
                last_tick = now;
                continue;
            }
            let Some(time) = parse_hhmm(&settings.scheduled_backup_time) else {
                pending = None;
                last_tick = now;
                continue;
            };
            // 夏令时回拨会产生两个相同的本地时间：取较早者触发，last_run 保证当天只执行一次。
            let Some(scheduled) = Local
                .from_local_datetime(&now.date_naive().and_time(time))
                .earliest()
            else {
                last_tick = now;
                continue;
            };

            // 只在「本次运行期间」跨过时间点时触发一次；休眠/时钟前跳后跨过的点按唤醒时刻补跑。
            if pending.is_none()
                && scheduled > last_tick
                && scheduled <= now
                && last_run != Some(scheduled.date_naive())
            {
                pending = Some((now, scheduled.date_naive()));
            }
            last_tick = now;

            let Some((triggered_at, run_date)) = pending else { continue };
            if now - triggered_at > chrono::Duration::minutes(RETRY_WINDOW_MINUTES) {
                pending = None;
                emit_notice(
                    &app,
                    "failed",
                    Some("已有其它备份在运行，本次定时备份已跳过".to_string()),
                );
                continue;
            }

            let Some(config_id) = settings
                .scheduled_backup_config_id
                .clone()
                .filter(|value| !value.trim().is_empty())
            else {
                pending = None;
                emit_notice(&app, "noConfig", None);
                continue;
            };

            let request = BackupRequest {
                server_id: String::new(),
                backup_config_id: Some(config_id),
                source: None,
                target_id: None,
                supabase_url: None,
                database: None,
                schema: None,
            };
            match start_backup(app.clone(), app.state::<AppState>(), request) {
                Ok(_) => {
                    pending = None;
                    last_run = Some(run_date);
                    emit_notice(&app, "started", None);
                }
                Err(err) => {
                    // 已有备份（手动或 CLI）在跑：窗口内继续等待，不算失败。
                    // 用错误类型判断而非字符串匹配，文案变化不会让重试逻辑静默失效。
                    let busy = app.state::<AppState>().is_backup_active()
                        || matches!(&err, CoreError::Busy(_));
                    if !busy {
                        pending = None;
                        emit_notice(&app, "failed", Some(err.to_string()));
                    }
                }
            }
        }
    });
}

/// 轮询间隔：排定了关机就切到秒级，否则保持低频。
fn tick_interval(app: &AppHandle) -> Duration {
    if app.state::<AppState>().pending_shutdown().is_some() {
        COUNTDOWN_INTERVAL
    } else {
        CHECK_INTERVAL
    }
}

/// 倒计时到点的关机：先摘掉计划（与「取消」互斥），再下发系统关机请求并触发退出清理。
fn fire_due_shutdown(app: &AppHandle, now: chrono::DateTime<Local>) {
    if app
        .state::<AppState>()
        .take_due_shutdown(now.timestamp_millis())
        .is_none()
    {
        return;
    }
    match request_shutdown(OS_GRACE_SECS) {
        Ok(()) => {
            emit_status(app);
            emit_notice(app, "shutdownFired", None);
            // 系统还有 OS_GRACE_SECS 秒才真关机，这段时间够应用走正常退出流程
            // （见 lib.rs：终止本地子进程 + 回收远端脚本），不至于被系统强杀在半路。
            app.exit(0);
        }
        // 不重试：轮询是秒级的，一次配置错误反复弹错会把日志和提示全刷掉。
        // 计划已经摘掉，状态也要跟着重发：否则状态栏的倒计时会一直空转，
        // 点「取消」也只是对着已经不存在的计划操作。
        Err(err) => {
            emit_status(app);
            emit_notice(app, "shutdownFailed", Some(err.to_string()));
        }
    }
}

/// 每天定时关机：跨过设置的时间点就排一次可取消的倒计时。
///
/// 和定时备份一样，应用启动前已错过的时间点不补跑 —— 唤醒电脑就被关机是不能接受的行为。
fn arm_scheduled_shutdown(
    app: &AppHandle,
    settings: &Settings,
    now: chrono::DateTime<Local>,
    last_tick: chrono::DateTime<Local>,
    last_run: &mut Option<chrono::NaiveDate>,
) {
    if !settings.scheduled_shutdown_enabled {
        // 开关被关掉：连同已经排好的那一次一起取消。手动倒计时是用户刚按下的，不动它。
        if app.state::<AppState>().cancel_scheduled_shutdown().is_some() {
            emit_status(app);
        }
        return;
    }
    let Some(time) = parse_hhmm(&settings.scheduled_shutdown_time) else {
        return;
    };
    // 夏令时回拨同备份：取较早的那个本地时间。
    let Some(scheduled) = Local
        .from_local_datetime(&now.date_naive().and_time(time))
        .earliest()
    else {
        return;
    };

    let state = app.state::<AppState>();
    let crossed = scheduled > last_tick && scheduled <= now;
    if !crossed || state.pending_shutdown().is_some() || *last_run == Some(scheduled.date_naive())
    {
        return;
    }
    // 先记日期再排计划：用户取消后当天不该再被同一个时间点打扰。
    *last_run = Some(scheduled.date_naive());
    state.arm_shutdown(PendingShutdown {
        at_ms: now.timestamp_millis() + CANCEL_WINDOW_SECS * 1000,
        source: ShutdownSource::Scheduled,
    });
    emit_status(app);
}

fn emit_notice(app: &AppHandle, kind: &'static str, message: Option<String>) {
    let _ = app.emit("scheduler://notice", SchedulerNotice { kind, message });
}

/// 解析 "HH:MM"（也接受 "H:MM"）。
fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour: u32 = hour.trim().parse().ok()?;
    let minute: u32 = minute.trim().parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hhmm_accepts_common_forms() {
        assert_eq!(parse_hhmm("03:00"), NaiveTime::from_hms_opt(3, 0, 0));
        assert_eq!(parse_hhmm("23:59"), NaiveTime::from_hms_opt(23, 59, 0));
        assert_eq!(parse_hhmm(" 8:5 "), NaiveTime::from_hms_opt(8, 5, 0));
    }

    #[test]
    fn parse_hhmm_rejects_invalid() {
        assert!(parse_hhmm("").is_none());
        assert!(parse_hhmm("24:00").is_none());
        assert!(parse_hhmm("12:60").is_none());
        assert!(parse_hhmm("abc").is_none());
        assert!(parse_hhmm("12").is_none());
    }
}
