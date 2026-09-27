use std::collections::VecDeque;
use std::time::Duration;

use chrono::Local;
use deploy_core::models::{AppConfig, BackupRequest, Settings};
use deploy_core::schedule::{format_date, has_crossed, parse_date, today_at, window_expired};
use deploy_core::shutdown::{request_shutdown, CANCEL_WINDOW_SECS, OS_GRACE_SECS};
use deploy_core::{CoreError, Store};
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

            // 执行位是「控制机」的配置由 agent 到点执行：本机再跑一遍就是两次导出、两份包，
            // 而且控制机上那份的节奏会和本机这份混在一起。
            // 但只有「上次成功下发确实把这条带过去了」才算交给它 —— 光看执行位就撒手，
            // 用户改了名字没重新下发（或压根没装 agent）的那一晚就两头都不跑。
            match backup_owner(&app.state::<AppState>().store, &config, &config_id) {
                BackupOwner::Agent(name) => {
                    pending = None;
                    emit_notice(&app, "remoteSkipped", Some(name));
                    continue;
                }
                BackupOwner::Unsynced(name) => {
                    // 本机不代跑：包会落在本机，而用户以为在控制机上，恢复和迁移都找不到东西。
                    // 报失败，让他去点「下发配置」。
                    pending = None;
                    emit_notice(
                        &app,
                        "failed",
                        Some(format!(
                            "「{name}」的执行位是控制机，但控制机上没有这条配置（未安装或未下发），请到控制机页面点「下发配置」"
                        )),
                    );
                    continue;
                }
                BackupOwner::Local => {}
            }

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
                        // 只保留仍然存在、且今晚确实由本机负责的配置；真正交给控制机的才静默剔除。
                        let (local, handed, unsynced) = split_container_queue(
                            &app.state::<AppState>().store,
                            &config,
                            &settings.scheduled_container_config_ids,
                        );
                        queue = local;
                        if handed > 0 {
                            // 带单位：这条 message 会被拼进「已交给控制机执行：…」，
                            // 只发一个数字过去，界面读起来像坏掉的输出。
                            emit_notice(
                                &app,
                                "remoteSkipped",
                                Some(format!("{handed} 条容器备份")),
                            );
                        }
                        if !unsynced.is_empty() {
                            emit_notice(
                                &app,
                                "containerFailed",
                                Some(format!(
                                    "{} 条容器备份的执行位是控制机，但控制机上没有它们（未安装或未下发），本次不执行",
                                    unsynced.len()
                                )),
                            );
                        }
                        if queue.is_empty() && handed == 0 && unsynced.is_empty() {
                            // 只在真的没人可跑时说「未选择配置」。上面两条分支已经解释过原因时
                            // 再补一句「没配置」是错的，用户会去翻勾选框而不去点下发。
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
    let Some(scheduled) = today_at(&settings.scheduled_shutdown_time, &now) else {
        return;
    };

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

/// 一条定时备份配置今晚归谁跑。
enum BackupOwner {
    /// 本机负责（执行位本来就是本机，或配置已经不在了）。
    Local,
    /// 控制机负责，附带配置名（提示里要说清楚是哪条）。
    Agent(String),
    /// 执行位写着控制机，但那台机器上并没有这一份：没装、没下发、或下发之后才改名/新建。
    Unsynced(String),
}

/// 判定一条定时备份配置的归属。只在执行位是远端时才去读同步指纹，本机路径不多一次文件读。
fn backup_owner(store: &Store, config: &AppConfig, config_id: &str) -> BackupOwner {
    let Some(item) = config.backup_configs.iter().find(|entry| entry.id == config_id) else {
        return BackupOwner::Local;
    };
    if !item.run_location.is_remote() {
        return BackupOwner::Local;
    }
    let agent = config.settings.agent_server_id.trim();
    if store.load_agent_sync().holds_backup(agent, &item.id) {
        BackupOwner::Agent(item.name.clone())
    } else {
        BackupOwner::Unsynced(item.name.clone())
    }
}

/// 容器定时队列按执行位分成三份：本机要跑的、已交给控制机的条数、写着控制机但它没持有的名字。
///
/// 最后一类**不进本机队列**：本机代跑会把包落在本机，而用户以为在控制机上，恢复和迁移
/// 都找不到那个包 —— 宁可这一条不执行并报错，让他去补一次下发。
fn split_container_queue(
    store: &Store,
    config: &AppConfig,
    ids: &[String],
) -> (VecDeque<String>, usize, Vec<String>) {
    let mut local = VecDeque::new();
    let mut handed = 0usize;
    let mut unsynced = Vec::new();
    let agent = config.settings.agent_server_id.trim();
    let sync = store.load_agent_sync();
    for id in ids {
        match config.container_configs.iter().find(|item| &item.id == id) {
            Some(item) if !item.run_location.is_remote() => local.push_back(item.id.clone()),
            Some(item) if sync.holds_container(agent, &item.id) => handed += 1,
            Some(item) => unsynced.push(item.name.clone()),
            // 配置已经不在了：静默剔除，与改动前的行为一致。
            None => {}
        }
    }
    (local, handed, unsynced)
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

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::agent::AgentSyncState;
    use deploy_core::models::{BackupConfig, ContainerConfig, DbBackupSource, RunLocation};

    fn store_with(sync: Option<AgentSyncState>) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-scheduler-{}",
            deploy_core::models::new_id()
        ));
        let store = Store::new(&dir);
        if let Some(state) = sync {
            store.save_agent_sync(&state).unwrap();
        }
        store
    }

    fn backup(id: &str, remote: bool) -> BackupConfig {
        let mut config =
            BackupConfig::new(id.to_string(), "s1".to_string(), DbBackupSource::default());
        config.id = id.to_string();
        config.run_location = if remote { RunLocation::Remote } else { RunLocation::Local };
        config
    }

    fn container(id: &str, remote: bool) -> ContainerConfig {
        ContainerConfig {
            id: id.to_string(),
            name: id.to_string(),
            server_id: "s1".to_string(),
            project: "app".to_string(),
            pause_source: false,
            include_volumes: true,
            include_images: false,
            target: None,
            created_at: String::new(),
            run_location: if remote { RunLocation::Remote } else { RunLocation::Local },
        }
    }

    fn config_with(agent: &str, backups: Vec<BackupConfig>) -> AppConfig {
        let mut config = AppConfig::default();
        config.settings.agent_server_id = agent.to_string();
        config.backup_configs = backups;
        config
    }

    #[test]
    fn remote_config_only_yields_after_a_real_sync() {
        let held = AgentSyncState {
            server_id: "s1".to_string(),
            backup_config_ids: vec!["b1".to_string()],
            ..Default::default()
        };

        // 执行位是远端、且控制机确实持有这条 → 让位。
        let store = store_with(Some(held.clone()));
        let config = config_with("s1", vec![backup("b1", true)]);
        assert!(matches!(
            backup_owner(&store, &config, "b1"),
            BackupOwner::Agent(name) if name == "b1"
        ));

        // 执行位是远端但从没下发过 → 判为未就绪：循环据此报失败，本机不代跑。
        let empty = store_with(None);
        assert!(matches!(
            backup_owner(&empty, &config, "b1"),
            BackupOwner::Unsynced(name) if name == "b1"
        ));

        // 换了一台控制机：旧指纹不算数，得重新下发。
        assert!(matches!(
            backup_owner(&store, &config_with("s2", vec![backup("b1", true)]), "b1"),
            BackupOwner::Unsynced(_)
        ));

        // 本机执行位 / 配置已删除：与改动前一样走本机。
        let local = config_with("s1", vec![backup("b2", false)]);
        assert!(matches!(backup_owner(&store, &local, "b2"), BackupOwner::Local));
        assert!(matches!(backup_owner(&store, &local, "gone"), BackupOwner::Local));
    }

    #[test]
    fn container_queue_does_not_cover_for_unsynced_remote_configs() {
        let store = store_with(Some(AgentSyncState {
            server_id: "s1".to_string(),
            container_config_ids: vec!["c1".to_string()],
            ..Default::default()
        }));
        let mut config = AppConfig::default();
        config.settings.agent_server_id = "s1".to_string();
        config.container_configs = vec![
            container("c1", true),
            container("c2", true),
            container("c3", false),
        ];
        let ids: Vec<String> = ["c1", "c2", "c3", "gone"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        let (local, handed, unsynced) = split_container_queue(&store, &config, &ids);
        // c1 真的在控制机上（本机剔除）；c2 写着远端但没下发过 —— 本机也不代跑，
        // 只把它报成失败让人去补下发；c3 本来就是本机的。
        assert_eq!(handed, 1);
        assert_eq!(unsynced, vec!["c2".to_string()]);
        assert_eq!(local, VecDeque::from(["c3".to_string()]));
    }
}
