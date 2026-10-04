//! 定时任务的到点判定。
//!
//! GUI 的调度循环（`src-tauri/src/scheduler.rs`）与控制机上的常驻 agent 共用这一份判断，
//! 免得两边对「错过要不要补跑」「重试窗口多长」的理解分叉 —— 这两条都是用户能直接感知的行为。
//! 队列推进、忙则等待这类状态机仍留在各自的循环里：它们依赖进程内的任务名额，抽出来只会更难懂。

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

/// 到点后若因已有任务占用而无法启动，最多重试的时长。
pub const RETRY_WINDOW_MINUTES: i64 = 10;

/// 一天的分钟数：平移墙上时钟时用来取模。
const MINUTES_PER_DAY: i64 = 24 * 60;

/// 解析 "HH:MM"（也接受 "H:MM"）。
pub fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour: u32 = hour.trim().parse().ok()?;
    let minute: u32 = minute.trim().parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// 设置里那个时间点在今天对应的本地时刻；写法不对时返回 None（静默跳过，不给用户报错）。
///
/// 夏令时回拨会产生两个相同的本地时间：取较早者触发，当天只执行一次靠 [`has_crossed`] 去重。
pub fn today_at(value: &str, now: &DateTime<Local>) -> Option<DateTime<Local>> {
    let time = parse_hhmm(value)?;
    Local
        .from_local_datetime(&now.date_naive().and_time(time))
        .earliest()
}

/// 本机的时区偏移（分钟，东为正）在 `value` 这个墙上时刻「下一次发生」时是多少。
///
/// 取下一次而不是取现在：带夏令时的本机在切换日前后，同一个 `HH:MM` 对应的偏移可能不同，
/// 要按真正会跑的那一次算。写法不对时返回 None（调用方按「不折算」处理）。
pub fn local_offset_at_next(value: &str, now: &DateTime<Local>) -> Option<i32> {
    let time = parse_hhmm(value)?;
    let today = now.date_naive().and_time(time);
    let candidate = Local.from_local_datetime(&today).earliest()?;
    let next = if candidate > *now {
        candidate
    } else {
        // 今天这个点已经过去，那下一次发生就是明天（跨夏令时的偏移差要在明天那次取）。
        Local
            .from_local_datetime(&(today + Duration::days(1)))
            .earliest()?
    };
    Some(next.offset().local_minus_utc() / 60)
}

/// 把「墙上时钟的 HH:MM」平移 `delta_minutes` 分钟，仍然返回 HH:MM（跨日取模）。
///
/// 用于下发配置给控制机：设置里的 `HH:MM` 是用户在**本机时区**认定的时刻，而 agent 用
/// 它自己的本地时区解释这个字符串（见 [`today_at`]）；机器在 UTC 时"03:00"是本地下午。
/// 所以下发前把两个时区的差平移掉，让控制机到点那一刻正好等于本机的定点。
///
/// 只平移固定分钟数、不查目标时区的夏令时规则：agent 上报的是一个固定偏移（`date +%z`），
/// 目标机若在带夏令时的时区，切换那一两天的执行时刻会差一小时（中国境内与 UTC 无此问题）。
pub fn shift_hhmm(value: &str, delta_minutes: i64) -> Option<String> {
    let time = parse_hhmm(value)?;
    let total = (time.hour() as i64 * 60 + time.minute() as i64 + delta_minutes).rem_euclid(MINUTES_PER_DAY);
    Some(format!("{:02}:{:02}", total / 60, total % 60))
}

/// 相邻两轮轮询之间允许的最大空档（秒）。
///
/// 调度循环正常是 20 秒一跳（排定关机后 1 秒一跳），留出十倍余量是把「一次卡顿」和
/// 「睡了一觉」分开：超过它就只能认为进程当时没在跑。
pub const CLOCK_JUMP_SECS: i64 = 90;

/// 相邻两轮之间是否空转了太久 —— 只可能是进程睡了或者时钟被往前拨了。
///
/// [`has_crossed`] 只挡住「进程停启之间错过的点」，挡不住**进程活着但中间睡过去**的那种：
/// 休眠时这一跳被拉长，唤醒后 `last_tick` 还停在睡前，设置的时间点就落在 `(last_tick, now]`
/// 里，看起来像刚跨过。破坏性动作（定时关机）必须先过这道闸。
pub fn clock_jumped(last_tick: DateTime<Local>, now: DateTime<Local>) -> bool {
    now - last_tick > Duration::seconds(CLOCK_JUMP_SECS)
}

/// 本次轮询是否「首次跨过」`scheduled` 这个时刻。
///
/// 三个条件缺一不可：只在本轮区间 `(last_tick, now]` 内跨过才算触发，因此进程停启之间
/// 错过的时间点不补跑；`last_run` 挡住时钟回拨让同一个调度日二次触发。
/// 注意它不判断进程**活着但空转了一大段**（休眠唤醒、时钟前跳）的那种跨过，
/// 需要「到点就得准时」的调用方要另外用 [`clock_jumped`] 排除。
pub fn has_crossed(
    scheduled: DateTime<Local>,
    last_tick: DateTime<Local>,
    now: DateTime<Local>,
    last_run: Option<NaiveDate>,
) -> bool {
    scheduled > last_tick && scheduled <= now && last_run != Some(scheduled.date_naive())
}

/// 重试窗口是否已用完：超过它就放弃这一次调度，不再干等到下一个时间点。
pub fn window_expired(triggered_at: DateTime<Local>, now: DateTime<Local>) -> bool {
    now - triggered_at > Duration::minutes(RETRY_WINDOW_MINUTES)
}

/// 把调度日期写成可落盘的形态（存进 settings，进程重启后还能认出来）。
pub fn format_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// 读回落盘的调度日期；空串或写坏时按「从未触发」处理，而不是让调度器起不来。
pub fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}

/// 哪一条调度循环记的日期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKind {
    Backup,
    Container,
}

/// 调度器已经触发过的日期。
///
/// 单独一份文件（而不是塞进 `settings`）是为了给控制机上的 agent 用：客户端每次下发配置
/// 都会整体覆盖 agent 的 `config.json`，日期若存在 settings 里就会被顶成客户端那份，
/// systemd 一重启就把当天已经跑过的备份再跑一次。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleState {
    #[serde(default)]
    pub backup_last_run: String,
    #[serde(default)]
    pub container_last_run: String,
}

impl ScheduleState {
    fn slot(&mut self, kind: ScheduleKind) -> &mut String {
        match kind {
            ScheduleKind::Backup => &mut self.backup_last_run,
            ScheduleKind::Container => &mut self.container_last_run,
        }
    }

    pub fn last_run(&self, kind: ScheduleKind) -> Option<NaiveDate> {
        let value = match kind {
            ScheduleKind::Backup => self.backup_last_run.as_str(),
            ScheduleKind::Container => self.container_last_run.as_str(),
        };
        parse_date(value)
    }

    pub fn remember(&mut self, kind: ScheduleKind, date: NaiveDate) {
        *self.slot(kind) = format_date(date);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, hh: u32, mm: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, hh, mm, 0).unwrap()
    }

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
        assert!(parse_hhmm("0300").is_none());
    }

    #[test]
    fn today_at_lands_on_the_same_day_as_now() {
        let now = at(2026, 3, 9, 0, 0);
        let scheduled = today_at("03:00", &now).unwrap();
        assert_eq!(scheduled.date_naive(), now.date_naive());
        assert_eq!(scheduled, at(2026, 3, 9, 3, 0));
    }

    #[test]
    fn today_at_rejects_malformed_time() {
        assert!(today_at("", &Local::now()).is_none());
        assert!(today_at("3:00pm", &Local::now()).is_none());
    }

    #[test]
    fn crossed_only_within_the_last_poll_interval() {
        let scheduled = at(2026, 3, 9, 3, 0);
        // 还没到点。
        assert!(!has_crossed(scheduled, at(2026, 3, 9, 2, 0), at(2026, 3, 9, 2, 20), None));
        // 本轮跨过了。
        assert!(has_crossed(scheduled, at(2026, 3, 9, 2, 0), at(2026, 3, 9, 3, 20), None));
        // 进程没开着的时候就已经过去了：不补跑。
        assert!(!has_crossed(
            scheduled,
            at(2026, 3, 9, 4, 0),
            at(2026, 3, 9, 9, 0),
            None
        ));
    }

    /// 进程活着但中间睡过去了：`has_crossed` 会把它当成刚跨过，所以要靠 `clock_jumped` 认出来。
    #[test]
    fn clock_jump_is_read_from_the_gap_between_ticks() {
        let last_tick = at(2026, 3, 9, 2, 0);
        // 正常的一跳（20 秒）与阈值之内都不算跳变。
        assert!(!clock_jumped(last_tick, last_tick + Duration::seconds(20)));
        assert!(!clock_jumped(last_tick, last_tick + Duration::seconds(CLOCK_JUMP_SECS)));
        // 合上盖子过夜：唤醒后这一跳跨了几个小时。
        assert!(clock_jumped(last_tick, last_tick + Duration::hours(6)));
        // 时钟回拨不算「跨过」，也不该被当成跳变（挡住它的是 last_run）。
        assert!(!clock_jumped(last_tick, last_tick - Duration::hours(6)));
        // 对照：那种跨过确实被 has_crossed 认成「刚跨过」—— 这就是要额外挡的原因。
        let scheduled = at(2026, 3, 9, 3, 0);
        let woke_up = last_tick + Duration::hours(6);
        assert!(has_crossed(scheduled, last_tick, woke_up, None));
        assert!(clock_jumped(last_tick, woke_up));
    }

    #[test]
    fn crossed_only_once_per_day() {
        let scheduled = at(2026, 3, 9, 3, 0);
        let last_tick = at(2026, 3, 9, 2, 0);
        let now = at(2026, 3, 9, 3, 20);
        assert!(!has_crossed(
            scheduled,
            last_tick,
            now,
            Some(scheduled.date_naive())
        ));
        // 昨天的记录不该挡住今天。
        assert!(has_crossed(scheduled, last_tick, now, Some(at(2026, 3, 8, 3, 0).date_naive())));
    }

    #[test]
    fn window_expires_after_the_retry_window() {
        let triggered_at = at(2026, 3, 9, 3, 0);
        assert!(!window_expired(
            triggered_at,
            at(2026, 3, 9, 3, 0) + Duration::minutes(RETRY_WINDOW_MINUTES)
        ));
        assert!(window_expired(
            triggered_at,
            at(2026, 3, 9, 3, 0) + Duration::minutes(RETRY_WINDOW_MINUTES + 1)
        ));
    }

    #[test]
    fn persisted_date_round_trips_and_tolerates_garbage() {
        let date = at(2026, 3, 9, 3, 0).date_naive();
        let stored = format_date(date);
        assert_eq!(stored, "2026-03-09");
        assert_eq!(parse_date(&stored), Some(date));
        assert_eq!(parse_date("  2026-03-09  "), Some(date));
        // 从未触发（空串）与写坏了都按「没有记录」处理，不能让调度器起不来。
        assert_eq!(parse_date(""), None);
        assert_eq!(parse_date("昨天"), None);
    }

    #[test]
    fn schedule_state_keeps_the_two_loops_apart() {
        let mut state = ScheduleState::default();
        assert_eq!(state.last_run(ScheduleKind::Backup), None);
        state.remember(ScheduleKind::Backup, at(2026, 3, 9, 3, 0).date_naive());
        // 容器那条循环不该看见数据库那条的记录，否则两条循环互相把对方的日期当成「今天已跑」。
        assert_eq!(state.last_run(ScheduleKind::Container), None);
        assert_eq!(state.last_run(ScheduleKind::Backup).unwrap().to_string(), "2026-03-09");
        state.remember(ScheduleKind::Container, at(2026, 3, 10, 3, 30).date_naive());
        assert_eq!(state.backup_last_run, "2026-03-09");
        assert_eq!(state.container_last_run, "2026-03-10");
    }

    #[test]
    fn shift_hhmm_moves_the_wall_clock_and_wraps() {
        // 本机 UTC+8、控制机 UTC：本机 03:00 = 控制机 19:00（前一天），差 -480 分钟。
        assert_eq!(shift_hhmm("03:00", -480).as_deref(), Some("19:00"));
        // 本机 UTC+8、控制机 UTC+0：本机 20:00 = 控制机 12:00。
        assert_eq!(shift_hhmm("20:00", -480).as_deref(), Some("12:00"));
        // 同区：原样。
        assert_eq!(shift_hhmm("03:00", 0).as_deref(), Some("03:00"));
        // 半小时时区（差 -330 分钟）也要能落在合法的时刻上：03:00 往前 5 小时 30 分。
        assert_eq!(shift_hhmm("03:00", -330).as_deref(), Some("21:30"));
        // 正负都取模到 00:00-23:59，不会写出 24:00 或 -1:00。
        assert_eq!(shift_hhmm("23:59", 1).as_deref(), Some("00:00"));
        assert_eq!(shift_hhmm("00:00", -1).as_deref(), Some("23:59"));
        assert_eq!(shift_hhmm("03:00", 24 * 60).as_deref(), Some("03:00"));
        assert_eq!(shift_hhmm("坏的", 60), None);
    }

    #[test]
    fn local_offset_is_a_plausible_utc_offset() {
        // 只验形状：本机时区由环境决定，断言具体值会让测试跟着机器跑。
        let offset = local_offset_at_next("03:00", &Local::now()).unwrap();
        assert!((-12 * 60..=14 * 60).contains(&offset), "{offset}");
        assert!(local_offset_at_next("", &Local::now()).is_none());
    }

    #[test]
    fn schedule_state_json_is_camel_case_and_tolerates_missing_fields() {
        let state: ScheduleState = serde_json::from_str("{}").unwrap();
        assert_eq!(state, ScheduleState::default());
        let text = serde_json::to_string(&ScheduleState {
            backup_last_run: "2026-03-09".to_string(),
            container_last_run: String::new(),
        })
        .unwrap();
        assert!(text.contains("backupLastRun"), "{text}");
    }
}
