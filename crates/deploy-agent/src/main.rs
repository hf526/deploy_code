//! 控制机常驻进程：把「定时备份」这件事从装客户端的那台机器搬到服务器上。
//!
//! 子命令就是客户端会经由 SSH 下达的全部内容：
//!
//! ```text
//! deploy-agent --version                 协议握手（客户端每次下发前都会跑）
//! deploy-agent run    [--data-dir D]     常驻：两条定时循环（数据库备份 / 容器备份）
//! deploy-agent status [--data-dir D]     回读状态（stdout 输出一个 JSON 文档）
//! deploy-agent records [--data-dir D] --kind backup|container [--limit N]
//! deploy-agent trigger --kind backup|container --config <id> [--data-dir D]
//! deploy-agent restore --record <id> --target <serverId> [--dir P] [--start 1]
//! ```
//!
//! 两个刻意的约定：
//!
//! - **不监听任何端口**。全部输入来自一次 `ssh exec`，因此控制机上没有需要加固的对外服务。
//! - **`trigger` / `restore` 只往 stdout 打 JSON 行**，人读的报错走 stderr + 非零退出码。
//!   客户端按行反序列化成事件喂给已有的进度通道，所以这里绝不能顺手 `println!` 一句说明文字。
//!   `run` / `status` / `records` 反之：它们分别走 journald 与 JSON 文档通道，互不冲突。

mod daemon;
mod report;
mod tasks;

use std::path::PathBuf;

use deploy_core::agent::{AgentVersion, DATA_DIR, PROTO};
use deploy_core::{CoreError, Store};

/// 任务失败（备份本身跑完但没成）：与 CLI 的退出码约定一致，界面据此标红。
pub(crate) const EXIT_FAILED: i32 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match dispatch(&args) {
        Ok(()) => 0,
        Err(err) => {
            // 报错只走 stderr：stdout 在 trigger 模式下是事件流，混进一句人话就会毁掉整场任务。
            eprintln!("{err}");
            match err {
                CoreError::Busy(_) => 3,
                _ => 1,
            }
        }
    };
    std::process::exit(code);
}

/// 同步入口：命令实现都是 tokio 任务，这里只为拿一个 runtime。
fn dispatch(args: &[String]) -> deploy_core::Result<()> {
    let first = args.first().map(String::as_str).unwrap_or("");
    if first.is_empty() {
        return Err(CoreError::config(usage()));
    }
    // 手写参数解析而不是用 clap：clap 的 --version 输出格式由它决定，
    // 而这里的版本行是协议的一部分（客户端要从中取出 proto= 做握手）。
    if matches!(first, "--version" | "-V") {
        println!("{}", version_text());
        return Ok(());
    }
    if matches!(first, "--help" | "-h" | "help") {
        println!("{}", usage());
        return Ok(());
    }
    let data_dir = flag(&args, "--data-dir").unwrap_or(DATA_DIR);
    let store = std::sync::Arc::new(Store::new(PathBuf::from(data_dir)));
    // 这个进程就是控制机：磁盘水位闸只在它身上拦任务（见 `Store::set_agent_mode`）。
    store.set_agent_mode(true);
    match first {
        "run" => runtime()?.block_on(daemon::run_all(store)),
        "status" => report::status(&store),
        "records" => report::records(&store, args),
        "trigger" => runtime()?.block_on(tasks::trigger(store, args)),
        "restore" => runtime()?.block_on(tasks::restore(store, args)),
        // 命令名先认下来再说：拼错的人该看到用法，而不是一个为必然失败的调用起起来的运行时。
        other => Err(CoreError::config(format!(
            "未知命令: {other}\n{}",
            usage()
        ))),
    }
}

/// 只有真正要跑任务的分支才建运行时。
fn runtime() -> deploy_core::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CoreError::config(format!("启动运行时失败: {e}")))
}

/// 握手用的版本行。客户端每次下发配置前都会跑一次 `deploy-agent --version` 并解析它，
/// 所以这行的格式属于协议，不能随手改。
pub(crate) fn version_text() -> String {
    AgentVersion {
        proto: PROTO,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
    .line()
}

fn usage() -> String {
    "用法: deploy-agent <命令> [参数]\n\
     \x20 run      [--data-dir D]                        常驻，按设置里的时间执行定时备份\n\
     \x20 status   [--data-dir D]                        输出一个 JSON 状态文档\n\
     \x20 records  [--data-dir D] --kind backup|container [--limit N]\n\
     \x20 trigger  --kind backup|container --config <配置 id 或名称>\n\
     \x20 restore  --record <容器记录 id> --target <服务器 id> [--dir 目录] [--start 1]\n\
     \x20 --version                                      打印协议与版本行"
        .to_string()
}

/// 取 `--name value` 形式参数的值。
pub(crate) fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let index = args.iter().position(|item| item == name)?;
    args.get(index + 1).map(String::as_str)
}

/// 取布尔开关：`--start 0` / `--start false` 为假，出现即真（末尾裸写也算真）。
pub(crate) fn bool_flag(args: &[String], name: &str) -> bool {
    let Some(index) = args.iter().position(|item| item == name) else {
        return false;
    };
    match args.get(index + 1) {
        None => true,
        Some(next) if next.starts_with("--") => true,
        Some(next) => !matches!(next.trim(), "0" | "false" | "no" | ""),
    }
}

pub(crate) fn usize_flag(args: &[String], name: &str, fallback: usize) -> usize {
    flag(args, name)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn flag_reads_the_value_right_after_the_name() {
        let list = args("--kind container --data-dir /srv/x --limit 7");
        assert_eq!(flag(&list, "--kind"), Some("container"));
        assert_eq!(flag(&list, "--data-dir"), Some("/srv/x"));
        assert_eq!(flag(&list, "--missing"), None);
        // 最后一个位置上的开关名没有值：按没给处理，而不是 panic。
        let tail = args("--kind");
        assert_eq!(flag(&tail, "--kind"), None);
    }

    #[test]
    fn bool_flag_treats_explicit_zero_as_false() {
        assert!(bool_flag(&args("--start 1"), "--start"));
        assert!(!bool_flag(&args("--start 0"), "--start"));
        assert!(!bool_flag(&args("--limit 5"), "--start"));
        assert!(bool_flag(&args("--start yes"), "--start"));
    }

    #[test]
    fn usize_flag_falls_back_on_garbage() {
        assert_eq!(usize_flag(&args("--limit 30"), "--limit", 50), 30);
        assert_eq!(usize_flag(&args("--limit x"), "--limit", 50), 50);
        assert_eq!(usize_flag(&args(""), "--limit", 50), 50);
    }

    #[test]
    fn version_text_round_trips_through_the_handshake() {
        // --version 不碰数据目录也不读配置：握手发生在安装那一刻，那时盘上还是空的。
        let text = version_text();
        let parsed = deploy_core::agent::parse_version(&text)
            .unwrap_or_else(|| panic!("版本行解析失败: {text:?}"));
        assert_eq!(parsed.proto, PROTO);
        assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        let err = dispatch(&args("nope")).unwrap_err().to_string();
        assert!(err.contains("未知命令"), "{err}");
        assert!(err.contains("deploy-agent"), "{err}");
        let err = dispatch(&args("")).unwrap_err().to_string();
        assert!(err.contains("用法"), "{err}");
    }
}
