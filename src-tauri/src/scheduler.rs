use std::collections::VecDeque;
use std::time::Duration;

use chrono::{Local, TimeZone};
use deploy_core::models::{BackupRequest, Settings};
use deploy_core::schedule::{
    clock_jumped, format_date, has_crossed, parse_date, today_at, window_expired,
};
use deploy_core::shutdown::{request_shutdown, CANCEL_WINDOW_SECS, OS_GRACE_SECS};
use deploy_core::CoreError;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::backup::start_backup;
use crate::commands::container::start_container_config_backup;
use crate::commands::shutdown::emit_status;
use crate::state::{AppState, PendingShutdown, ShutdownSource};

/// 定时检查间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(20);
/// 排定关机后的轮询间隔：20 秒的粒度会让关机点最多晚 20 秒，倒计时也会跳格。
const COUNTDOWN_INTERVAL: Duration = Duration::from_secs(1);
/// fire 侧的睡醒闸宽限。排定后轮询是 1 秒一次，正常到点的偏差远小于它；超过宽限还没
/// 发出去，只可能是倒计时期间进程睡过去（合盖）或时钟前跳 —— 与 arm 侧 clock_jumped
/// 挡的是同一件事（「唤醒电脑就被关机」），那种情况放弃这次关机，宁可错过不补发。
const FIRE_GRACE_MS: i64 = 10_000;

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
        // 启动时先把落盘的调度日期认回来：否则每次重启软件都会让当天的定时备份再触发一次。
        let stored = app
            .state::<AppState>()
            .store
            .load_config()
            .map(|config| config.settings)
            .unwrap_or_default();
        let mut last_tick = Local::now();
        // 已到点但因其它备份占用未能启动：记录「发现时刻」与所属调度日期，window 内继续重试。
        let mut pending: Option<(chrono::DateTime<Local>, chrono::NaiveDate)> = None;
        // 已发起过的调度日期（不是完成日期）：防止时钟回拨或重启后同一个调度日重复执行。
        let mut last_run = parse_date(&stored.scheduled_backup_last_run);
        // 已排过关机的调度日期：同一天只排一次，用户取消后当天也不再排。
        let mut shutdown_last_run = parse_date(&stored.scheduled_shutdown_last_run);

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
            let Some(scheduled) = today_at(&settings.scheduled_backup_time, &now) else {
                pending = None;
                last_tick = now;
                continue;
            };

            // 只在「本次运行期间」跨过时间点时触发一次；休眠/时钟前跳后跨过的点按唤醒时刻补跑。
            if pending.is_none() && has_crossed(scheduled, last_tick, now, last_run) {
                pending = Some((now, scheduled.date_naive()));
            }
            last_tick = now;

            let Some((triggered_at, run_date)) = pending else { continue };
            if window_expired(triggered_at, now) {
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
                    remember_schedule_run(&app, run_date, ScheduleKind::Backup);
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

/// 启动容器定时备份调度器：到点把勾选的配置排成队列，一次跑一个。
///
/// 与数据库备份的循环分开跑：容器一次任务要几十分钟，两个状态机混在一起
/// 只会让「等待上一个结束」的逻辑互相缠绕。容器任务有独立的名额，
/// 所以同一时刻和数据库备份并行也互不干扰。
pub fn spawn_container(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_tick = Local::now();
        // 待执行的配置队列：一次只下发一条，等前一条结束再取下一条。
        let mut queue: VecDeque<String> = VecDeque::new();
        // 本轮触发时刻：队列排不空（名额一直被占）时按它判断是否放弃。
        let mut triggered_at: Option<chrono::DateTime<Local>> = None;
        // 已排队的调度日期：与数据库备份一样落盘，重启后不重复排队的也不漏排。
        let mut last_run = app
            .state::<AppState>()
            .store
            .load_config()
            .map(|config| parse_date(&config.settings.scheduled_container_last_run))
            .unwrap_or(None);

        loop {
            tokio::time::sleep(CHECK_INTERVAL).await;
            let now = Local::now();
            let Ok(config) = app.state::<AppState>().store.load_config() else {
                last_tick = now;
                continue;
            };
            let settings = &config.settings;

            if settings.scheduled_container_enabled {
                if let Some(scheduled) = today_at(&settings.scheduled_container_time, &now) {
                    if has_crossed(scheduled, last_tick, now, last_run) {
                        // 先记日期：即使这一轮一个都没跑成，也不该在同一个调度日重复排队。
                        last_run = Some(scheduled.date_naive());
                        remember_schedule_run(&app, scheduled.date_naive(), ScheduleKind::Container);
                        triggered_at = Some(now);
                        // 只保留仍然存在的配置：被删掉的条目静默剔除，其余按勾选顺序排队。
                        queue = settings
                            .scheduled_container_config_ids
                            .iter()
                            .filter(|id| config.container_configs.iter().any(|item| &item.id == *id))
                            .cloned()
                            .collect();
                        if queue.is_empty() {
                            emit_notice(&app, "containerNoConfig", None);
                        }
                    }
                }
            } else if !queue.is_empty() {
                // 开关被关掉：这一晚排下的队列就地作废。
                queue.clear();
                triggered_at = None;
            }
            last_tick = now;

            let Some(first) = queue.front().cloned() else {
                triggered_at = None;
                continue;
            };
            // 重试窗口只约束「一次都还没跑成」的等待：跑成第一条之后，剩下的就该等前一条
            // 自然结束。一条容器任务本身可能跑几十分钟，不能被这个窗口掐掉。
            if let Some(started) = triggered_at {
                if window_expired(started, now) {
                    let skipped = queue.len();
                    queue.clear();
                    triggered_at = None;
                    emit_notice(
                        &app,
                        "containerFailed",
                        Some(format!(
                            "已有其它容器任务在运行，本次定时备份已跳过（{} 项未执行）",
                            skipped
                        )),
                    );
                    continue;
                }
            }
            // 上一条（手动的或队列里前一个配置）还在跑：等它结束再取下一个人。
            if app.state::<AppState>().has_active_container() {
                continue;
            }
            match start_container_config_backup(app.clone(), app.state::<AppState>(), first) {
                Ok(_) => {
                    queue.pop_front();
                    triggered_at = None;
                    emit_notice(&app, "containerStarted", None);
                }
                Err(err) => {
                    let busy = app.state::<AppState>().has_active_container()
                        || matches!(err, CoreError::Busy(_));
                    if busy {
                        continue;
                    }
                    // 单条配置自身有问题（服务器被删、项目已经不在了）：跳过它，
                    // 后面的配置照跑，否则一个坏条目能把整晚的备份堵在队列头上。
                    queue.pop_front();
                    emit_notice(&app, "containerFailed", Some(err.to_string()));
                }
            }
        }
    });
}

/// 倒计时到点的关机：先摘掉计划（与「取消」互斥），再下发系统关机请求并触发退出清理。
fn fire_due_shutdown(app: &AppHandle, now: chrono::DateTime<Local>) {
    let now_ms = now.timestamp_millis();
    let Some(plan) = app.state::<AppState>().take_due_shutdown(now_ms) else {
        return;
    };
    // 计划在倒计时期间睡过去（合盖）或时钟前跳，唤醒后的第一个 tick 会发现它早已过期：
    // 照发就是掀开盖子就开始关机。放弃时计划已摘走，arm 侧当天也记过日期不会再排，
    // 剩下的只是把状态收敛、把「为什么没关」讲清楚。
    if now_ms - plan.at_ms > FIRE_GRACE_MS {
        // earliest 而不是 single：DST 回拨的歧义时刻 single 会落空，提示里就缺了时刻。
        let planned = Local
            .timestamp_millis_opt(plan.at_ms)
            .earliest()
            .map(|at| at.format("%H:%M").to_string())
            .unwrap_or_default();
        emit_status(app);
        emit_notice(app, "shutdownMissed", Some(planned));
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
/// 但这一条还得自己多挡一道：`has_crossed` 只认「进程停启之间错过」，认不出
/// 「进程活着、中间睡过去」—— 那种情况下 `last_tick` 停在合盖之前，设置的时间点正好落在
/// `(last_tick, now]` 里，掀开盖子就见底。定时备份照旧补跑，晚一点备份无害。
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
    let Some(scheduled) = today_at(&settings.scheduled_shutdown_time, &now) else {
        return;
    };
    // 这一跳空转了太久：把这个点当成「已经错过」放掉，不写 last_run（明天到点照常排）。
    // 放掉之后循环会把 last_tick 推到 now，同一个点不会再被判定为刚跨过。
    if clock_jumped(last_tick, now) {
        return;
    }

    let state = app.state::<AppState>();
    if !has_crossed(scheduled, last_tick, now, *last_run) || state.pending_shutdown().is_some() {
        return;
    }
    // 先记日期再排计划：用户取消后当天不该再被同一个时间点打扰。
    *last_run = Some(scheduled.date_naive());
    remember_schedule_run(app, scheduled.date_naive(), ScheduleKind::Shutdown);
    state.arm_shutdown(PendingShutdown {
        at_ms: now.timestamp_millis() + CANCEL_WINDOW_SECS * 1000,
        source: ShutdownSource::Scheduled,
    });
    emit_status(app);
}

fn emit_notice(app: &AppHandle, kind: &'static str, message: Option<String>) {
    let _ = app.emit("scheduler://notice", SchedulerNotice { kind, message });
}

/// 三条调度循环各自记的触发日期，互不覆盖。
enum ScheduleKind {
    Backup,
    Container,
    Shutdown,
}

/// 把「这个调度日已经触发过」写回配置，好让重启后的进程认得它。
///
/// 写失败时静默：盘写不进的情况下备份本身也跑不动，多弹一条提示只会添个说不清的错。
fn remember_schedule_run(app: &AppHandle, date: chrono::NaiveDate, kind: ScheduleKind) {
    let store = app.state::<AppState>().store.clone();
    let value = format_date(date);
    let _ = store.mutate_config(|config| {
        let slot = match kind {
            ScheduleKind::Backup => &mut config.settings.scheduled_backup_last_run,
            ScheduleKind::Container => &mut config.settings.scheduled_container_last_run,
            ScheduleKind::Shutdown => &mut config.settings.scheduled_shutdown_last_run,
        };
        *slot = value;
        Ok(())
    });
}
