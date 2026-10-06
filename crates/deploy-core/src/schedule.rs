//! 定时任务的到点判定。
//!
//! GUI 的调度循环（`src-tauri/src/scheduler.rs`）用这一份判断，队列推进、忙则等待这类
//! 状态机仍留在循环里：它们依赖进程内的任务名额，抽出来只会更难懂。

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone};

/// 到点后若因已有任务占用而无法启动，最多重试的时长。
pub const RETRY_WINDOW_MINUTES: i64 = 10;

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
}
