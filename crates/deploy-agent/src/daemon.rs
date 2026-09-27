//! 常驻调度：把 GUI 里那两条定时循环搬到服务器上跑。
//!
//! 到点判定（HH:MM、错过不补跑、重试窗口、同一调度日只跑一次）全部来自
//! `deploy_core::schedule`，与 `src-tauri/src/scheduler.rs` 是同一份实现 —— 这两条是用户能
//! 直接感知的行为，分叉了就会出现「本机晚上跑过、控制机又跑一遍」。
//!
//! 队列推进与忙则等待仍留在各自的循环里：容器一条任务动辄几十分钟，和数据库备份的
//! 「20 秒看一次」混在一个循环里只会互相缠绕（GUI 那边也是因此分成两条循环）。
//!
//! 触发日期记在 `schedule-state.json`（[`Store::save_schedule_state`]）而不是 settings：
//! 客户端每次下发配置都会整体覆盖 `config.json`，日期若存在 settings 里就会被顶掉，
//! systemd 一拉起重启就把当天已经跑过的备份再跑一次。

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate};
use deploy_core::schedule::{
    has_crossed, today_at, window_expired, ScheduleKind, ScheduleState,
};
use deploy_core::{CoreError, Result, Store};

use crate::tasks::{execute, Kind, Sink, TaskOutcome};

/// 定时检查间隔，与 GUI 的调度循环一致。
const CHECK_INTERVAL: Duration = Duration::from_secs(20);
/// 循环 panic 后的重启退避：基数 × 次数，封顶在 5 分钟。
const PANIC_BACKOFF: Duration = Duration::from_secs(5);
const PANIC_BACKOFF_MAX: Duration = Duration::from_secs(300);

/// 两条循环各跑一个任务：数据库备份通常几十秒，容器备份几十分钟，不能互相挡。
pub async fn run_all(store: Arc<Store>) -> Result<()> {
    // 上次被 kill / 崩溃时留下的 Running 记录先收敛成失败，否则列表里永远挂着一条「进行中」。
    let reconciled = store.reconcile_interrupted().unwrap_or(0);
    log(
        "INFO",
        &format!(
            "deploy-agent {} proto={} 启动，数据目录 {}，{} 条遗留的进行中记录已收敛为失败",
            env!("CARGO_PKG_VERSION"),
            deploy_core::agent::PROTO,
            store.base_dir().display(),
            reconciled
        ),
    );
    let backup = tokio::spawn(supervised(store.clone(), "数据库备份", backup_loop));
    let container = tokio::spawn(supervised(store.clone(), "容器备份", container_loop));
    let both = async {
        // 两条循环都不该正常返回：任何一条退出都要说清楚，否则 journald 里只剩一片安静。
        join_logged(backup, "数据库备份").await?;
        join_logged(container, "容器备份").await
    };

    // systemd 停服务发的是 SIGTERM。不接它就是「进程当场消失、journal 里一句为什么都没有」；
    // 接一下只为了留下退出原因。在跑的任务不去假装能跑完：驱动方一死，它的 SSH 通道就断，
    // 远端脚本靠自己的 trap 收尾，那条记录下次启动时由 reconcile_interrupted 收敛成失败。
    tokio::select! {
        result = both => result,
        _ = wait_for_stop() => {
            log("INFO", "收到退出信号，停止调度（未跑完的任务按失败处理）");
            Ok(())
        }
    }
}

/// 等「该退出了」的信号：unix 上是 SIGTERM（`systemctl stop` 发的那个）。
///
/// Windows 分支用 Ctrl-C 顶位 —— 不是因为它真能跑 agent（没有 systemd 也没有 docker），
/// 而是让上面那段 `select!` 在开发机上同样被类型检查：这个交叉编译环境装不起 linux 的 C 工具链，
/// `cargo check --target x86_64-unknown-linux-gnu` 到 openssl-sys 就断了， cfg 掉的代码没人检查。
async fn wait_for_stop() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        terminate.recv().await;
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
    }
    Ok(())
}

/// 把一条循环放进「panic 只重启它自己」的监护里跑。
///
/// 原来 `run_all` 直接把两个任务的 JoinError 往上抛，于是任何一条循环 panic 都会让整个进程退出，
/// 连带掐死另一条正在跑的几十分钟任务；systemd 的 `Restart=always` 再把它拉回来，
/// 看起来就是「一个小故障毁掉一整晚」。分开监护：崩的那条退避重启，另一条不受影响。
async fn supervised<F, Fut>(store: Arc<Store>, label: &'static str, mut run: F)
where
    F: FnMut(Arc<Store>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut restarts = 0u32;
    loop {
        match tokio::spawn(run(store.clone())).await {
            // 循环自己结束了（当前代码里意味着被取消）：不重开，对着空转没意义。
            Ok(()) => return,
            Err(err) if err.is_panic() => {
                restarts += 1;
                let backoff = std::cmp::min(PANIC_BACKOFF * restarts, PANIC_BACKOFF_MAX);
                log(
                    "ERROR",
                    &format!("{label}调度循环 panic（第 {restarts} 次），{backoff:?} 后重启: {err}"),
                );
                tokio::time::sleep(backoff).await;
            }
            Err(err) => {
                log("ERROR", &format!("{label}调度循环被取消，不再重启: {err}"));
                return;
            }
        }
    }
}

async fn join_logged(handle: tokio::task::JoinHandle<()>, label: &str) -> Result<()> {
    handle
        .await
        .map_err(|err| CoreError::config(format!("{label}调度循环异常退出: {err}")))
}

/// 数据库备份的定时循环。
async fn backup_loop(store: Arc<Store>) {
    let mut last_tick = Local::now();
    let mut state = store.load_schedule_state();
    // 已到点但控制机被占用：记下发现时刻，在重试窗口内继续试。
    let mut pending: Option<(DateTime<Local>, NaiveDate)> = None;
    // 配置读不出来时只说一次：20 秒一行会把真正的备份记录冲掉。
    let mut quiet = false;
    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        let now = Local::now();
        let config = match store.load_config() {
            Ok(config) => {
                reset_quiet(&mut quiet);
                config
            }
            Err(err) => {
                log_once(
                    &mut quiet,
                    "WARN",
                    &format!("读取配置失败，定时备份暂停: {err}"),
                );
                last_tick = now;
                continue;
            }
        };
        let settings = &config.settings;
        let scheduled = if settings.scheduled_backup_enabled {
            today_at(&settings.scheduled_backup_time, &now)
        } else {
            None
        };
        let Some(scheduled) = scheduled else {
            pending = None;
            last_tick = now;
            continue;
        };
        if pending.is_none() && has_crossed(scheduled, last_tick, now, state.last_run(ScheduleKind::Backup)) {
            pending = Some((now, scheduled.date_naive()));
        }
        last_tick = now;

        let Some((triggered_at, run_date)) = pending else {
            continue;
        };
        if window_expired(triggered_at, now) {
            pending = None;
            log("WARN", "重试窗口用完，本次定时备份跳过（控制机一直被同类任务占用）");
            continue;
        }
        let Some(key) = settings
            .scheduled_backup_config_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            pending = None;
            log("WARN", "定时备份没有选择配置，本次跳过");
            continue;
        };
        match execute(&store, Kind::Backup, &key, Sink::Journal).await {
            Ok(outcome) => {
                pending = None;
                report(&outcome, &key, "定时备份");
                // 跑完才记日期：一次失败不该把整晚的机会一起吞掉（GUI 记的是「成功发起」，
                // 这里因为循环是同步等的，跑完就是它的下一次机会）。
                remember(&store, &mut state, ScheduleKind::Backup, run_date);
            }
            Err(err) if matches!(err, CoreError::Busy(_)) => {
                // 占用（有人在别处发起）：等下一个 tick 再试，窗口用完才算跳过。
            }
            Err(err) => {
                pending = None;
                remember(&store, &mut state, ScheduleKind::Backup, run_date);
                log("ERROR", &format!("定时备份 {key} 没能开始: {err}"));
            }
        }
    }
}

/// 容器备份的定时循环：到点把勾选的配置排成队列，一次跑一个。
async fn container_loop(store: Arc<Store>) {
    let mut last_tick = Local::now();
    let mut state = store.load_schedule_state();
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut triggered_at: Option<DateTime<Local>> = None;
    let mut quiet = false;
    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        let now = Local::now();
        let config = match store.load_config() {
            Ok(config) => {
                reset_quiet(&mut quiet);
                config
            }
            Err(err) => {
                log_once(
                    &mut quiet,
                    "WARN",
                    &format!("读取配置失败，容器定时备份暂停: {err}"),
                );
                last_tick = now;
                continue;
            }
        };
        let settings = &config.settings;

        let scheduled = if settings.scheduled_container_enabled {
            today_at(&settings.scheduled_container_time, &now)
        } else {
            None
        };
        if let Some(scheduled) = scheduled {
            if has_crossed(
                scheduled,
                last_tick,
                now,
                state.last_run(ScheduleKind::Container),
            ) {
                // 先记日期：即使这一晚一个都没跑成，也不该在同一个调度日重复排队。
                remember(
                    &store,
                    &mut state,
                    ScheduleKind::Container,
                    scheduled.date_naive(),
                );
                triggered_at = Some(now);
                // 只保留盘上还存在的配置：被删掉的条目静默剔除，别让一个坏条目带崩整晚。
                queue = settings
                    .scheduled_container_config_ids
                    .iter()
                    .filter(|id| {
                        config
                            .container_configs
                            .iter()
                            .any(|item| &item.id == *id)
                    })
                    .cloned()
                    .collect();
                if queue.is_empty() {
                    log("WARN", "容器定时备份没有可执行的配置");
                } else {
                    log("INFO", &format!("容器定时备份排队 {} 项", queue.len()));
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
        // 重试窗口只约束「一次都还没跑成」的等待：跑成第一条之后窗口就解除，
        // 否则一条 40 分钟的任务会把后面的整队全掐掉（与 GUI 同一条规则）。
        if let Some(started) = triggered_at {
            if window_expired(started, now) {
                let skipped = queue.len();
                queue.clear();
                triggered_at = None;
                log(
                    "WARN",
                    &format!("重试窗口用完，本次容器定时备份跳过（{skipped} 项未执行）"),
                );
                continue;
            }
        }
        match execute(&store, Kind::Container, &first, Sink::Journal).await {
            Ok(outcome) => {
                queue.pop_front();
                triggered_at = None;
                report(&outcome, &first, "容器定时备份");
            }
            Err(err) if matches!(err, CoreError::Busy(_)) => {
                // 上一条还在跑（或有人在别处占着名额）：等下一个 tick。
            }
            Err(err) => {
                // 单条配置自身有问题（服务器被删、项目已经不在了）：跳过它，后面的照跑。
                queue.pop_front();
                triggered_at = None;
                log("ERROR", &format!("容器定时备份 {first} 没能开始: {err}"));
            }
        }
    }
}

fn report(outcome: &TaskOutcome, key: &str, label: &str) {
    match outcome {
        TaskOutcome::Succeeded => log("INFO", &format!("{label} {key} 完成")),
        TaskOutcome::Failed(message) => {
            log("ERROR", &format!("{label} {key} 失败: {message}"))
        }
    }
}

fn remember(store: &Store, state: &mut ScheduleState, kind: ScheduleKind, date: NaiveDate) {
    state.remember(kind, date);
    if let Err(err) = store.save_schedule_state(state) {
        log("WARN", &format!("调度日期写盘失败: {err}"));
    }
}

/// 一行运行日志。journald 自带时间戳，这里补上秒级时间与级别，方便与记录对账。
pub(crate) fn log(level: &str, message: &str) {
    println!(
        "{} {level} {message}",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    );
}

/// 同一种故障只说一次，直到它恢复过为止。
fn log_once(quiet: &mut bool, level: &str, message: &str) {
    if *quiet {
        return;
    }
    *quiet = true;
    log(level, message);
}

fn reset_quiet(quiet: &mut bool) {
    *quiet = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::schedule::parse_date;

    fn at(y: i32, mo: u32, d: u32, hh: u32, mm: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(y, mo, d, hh, mm, 0).unwrap()
    }

    #[test]
    fn fresh_state_has_no_triggered_day() {
        // 第一次启动（还没下发过配置）就是这个状态：不能因此拒绝调度。
        let state = ScheduleState::default();
        assert_eq!(state.last_run(ScheduleKind::Backup), None);
        assert_eq!(state.last_run(ScheduleKind::Container), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn quiet_only_speaks_once_until_it_resets() {
        let mut quiet = false;
        log_once(&mut quiet, "WARN", "第一次");
        assert!(quiet, "说过一次之后应该转成安静");
        reset_quiet(&mut quiet);
        assert!(!quiet);
    }

    #[test]
    fn report_names_the_config_and_the_outcome() {
        // 只验证不打断、不 panic：级别与文本在 journald 里读。
        report(&TaskOutcome::Succeeded, "b1", "定时备份");
        report(&TaskOutcome::Failed("连不上".to_string()), "b1", "定时备份");
    }

    #[test]
    fn remember_moves_both_loops_apart() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-agent-daemon-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::new(&dir);
        let mut state = store.load_schedule_state();
        remember(&store, &mut state, ScheduleKind::Backup, at(2026, 9, 27, 3, 0).date_naive());
        // 落盘之后重读：容器那条必须还是空的，否则两条循环会互相挡。
        let reloaded = store.load_schedule_state();
        assert_eq!(
            reloaded.last_run(ScheduleKind::Backup),
            Some(at(2026, 9, 27, 3, 0).date_naive())
        );
        assert_eq!(reloaded.last_run(ScheduleKind::Container), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
