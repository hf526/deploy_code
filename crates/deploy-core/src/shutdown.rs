//! 定时关机：让运行 DeployCode 的本机在指定时刻关机（目前仅 Windows）。
//!
//! 这里只放「怎么向系统要一次关机」：参数构造与实际调用。
//! 「什么时候该关机」仍在 Tauri 侧的 `scheduler.rs` —— 那里有到点触发的轮询，
//! 且关机前必须走应用的退出清理流程，核心库不该依赖这些运行时状态。

use crate::error::{CoreError, Result};
use crate::process;

/// 一次性关机倒计时的最短时长（分钟）：再短就没有「反悔」的意义了。
pub const MIN_DELAY_MINUTES: u32 = 1;
/// 一次性关机倒计时的最长时长（分钟）：更久请改用「每天定时关机」。
pub const MAX_DELAY_MINUTES: u32 = 24 * 60;
/// 关机前的可取消窗口（秒）：到点后排这么久的倒计时，期间用户能反悔。
pub const CANCEL_WINDOW_SECS: i64 = 60;
/// 交给系统关机命令的缓冲时长（秒）。
///
/// `shutdown /s /t N` 一旦下发就不归本程序管，所以 N 要覆盖应用的退出清理
/// （终止本地子进程 + 回收远端脚本，正常一两秒内完成），又不能大到让用户对着
/// 一台已经不用的机器干等。
pub const OS_GRACE_SECS: u32 = 20;

/// 校验倒计时分钟数，返回给用户看的错误信息说明范围与下一步。
pub fn checked_delay_minutes(minutes: u32) -> Result<u32> {
    if minutes < MIN_DELAY_MINUTES || minutes > MAX_DELAY_MINUTES {
        return Err(CoreError::config(format!(
            "关机倒计时需要在 {MIN_DELAY_MINUTES}-{MAX_DELAY_MINUTES} 分钟之间（当前 {minutes} 分钟）。"
        )));
    }
    Ok(minutes)
}

/// 构造 `shutdown.exe` 的参数。
///
/// 单独拆出来是为了让 argv 可被单测覆盖：每个参数都是独立元素，不经 shell，
/// 也就不存在需要转义的东西。
///
/// 故意不带 `/c "原因"`：这台机器的 `shutdown /?` 把 `/c` 归在 `/d` 的说明之下，
/// 传上去有可能整条请求被拒（错误码 87）。请求失败 = 到点不关机，
/// 比通知里少一行自定义文案严重得多。
pub fn shutdown_args(delay_secs: u32) -> Vec<String> {
    vec![
        "/s".to_string(),
        "/t".to_string(),
        delay_secs.to_string(),
        "/f".to_string(),
    ]
}

/// 下发系统关机请求：`delay_secs` 秒后关机。
///
/// 带 `/f`：无人值守的定时关机不能被某个还在弹「是否保存」的程序挡住。
/// 本程序自己的收尾在调用方通过正常退出流程完成，不依赖这里。
pub fn request_shutdown(delay_secs: u32) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let output = process::run("shutdown", &shutdown_args(delay_secs), None)?;
        if output.code != 0 {
            return Err(CoreError::Process(format!(
                "下发关机请求失败（退出码 {}）：{}",
                output.code,
                output.combined().trim()
            )));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = delay_secs;
        Err(CoreError::Process(
            "定时关机目前仅支持 Windows".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_must_be_within_range() {
        assert_eq!(checked_delay_minutes(1).unwrap(), 1);
        assert_eq!(checked_delay_minutes(MAX_DELAY_MINUTES).unwrap(), 1440);
        assert!(matches!(
            checked_delay_minutes(0),
            Err(CoreError::Config(_))
        ));
        assert!(matches!(
            checked_delay_minutes(MAX_DELAY_MINUTES + 1),
            Err(CoreError::Config(_))
        ));
    }

    #[test]
    fn shutdown_args_match_the_documented_form() {
        // 顺序与 `shutdown /?` 一致：动作、超时、最后才是强制关闭。
        assert_eq!(
            shutdown_args(20),
            vec![
                "/s".to_string(),
                "/t".to_string(),
                "20".to_string(),
                "/f".to_string()
            ]
        );
    }

    #[test]
    fn shutdown_args_needs_no_shell_quoting() {
        // 一旦参数里出现 shell 元字符，说明有人在拼接命令行而不是传 argv。
        for arg in shutdown_args(30) {
            assert!(!arg.contains('&'), "{arg}");
            assert!(!arg.contains('|'), "{arg}");
            assert!(!arg.contains(';'), "{arg}");
            assert!(!arg.contains('`'), "{arg}");
        }
    }
}
