use std::io::{IsTerminal, Write};

use deploy_core::models::LogLevel;

const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";

fn color_enabled() -> bool {
    std::io::stdout().is_terminal()
}

fn paint(color: &str, text: &str) -> String {
    if color_enabled() {
        format!("{color}{text}{RESET}")
    } else {
        text.to_string()
    }
}

pub fn info(message: impl AsRef<str>) {
    println!("{}", message.as_ref());
}

pub fn dim(message: impl AsRef<str>) {
    println!("{}", paint(DIM, message.as_ref()));
}

pub fn success(message: impl AsRef<str>) {
    println!("{}", paint(GREEN, &format!("✔ {}", message.as_ref())));
}

pub fn error(message: impl AsRef<str>) {
    eprintln!("{}", paint(RED, &format!("✘ {}", message.as_ref())));
}

/// 按日志级别为部署日志着色。
pub fn deploy_log(level: LogLevel, message: &str) {
    match level {
        LogLevel::Info => println!("{message}"),
        LogLevel::Command => println!("{}", paint(CYAN, message)),
        LogLevel::Success => println!("{}", paint(GREEN, message)),
        LogLevel::Warn => println!("{}", paint(YELLOW, message)),
        LogLevel::Error => eprintln!("{}", paint(RED, message)),
    }
}

/// 覆盖当前行显示进度。
pub fn progress(message: &str) {
    print!("\r{message}     ");
    let _ = std::io::stdout().flush();
}

pub fn progress_done() {
    println!();
}
