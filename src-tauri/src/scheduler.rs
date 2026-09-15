use std::time::Duration;

use chrono::{Local, NaiveTime, TimeZone};
use deploy_core::models::BackupRequest;
use deploy_core::CoreError;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::backup::start_backup;
use crate::state::AppState;

/// 定时检查间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(20);
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

/// 启动定时备份调度器：应用运行期间（含窗口隐藏到托盘）到点执行一次备份。
///
/// 只在软件运行时生效；应用启动前已错过的时间点不补跑。
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_tick = Local::now();
        // 已到点但因其它备份占用未能启动：记录「发现时刻」与所属调度日期，window 内继续重试。
        let mut pending: Option<(chrono::DateTime<Local>, chrono::NaiveDate)> = None;
        // 已成功触发的调度日期（不是完成日期）：防止时钟回拨后同一个调度日重复执行。
        let mut last_run: Option<chrono::NaiveDate> = None;

        loop {
            tokio::time::sleep(CHECK_INTERVAL).await;
            let now = Local::now();

            let Ok(config) = app.state::<AppState>().store.load_config() else {
                last_tick = now;
                continue;
            };
            let settings = config.settings.clone();

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
