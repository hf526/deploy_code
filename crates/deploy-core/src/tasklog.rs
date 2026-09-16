//! 部署 / 备份 / Pages 三类任务共用的日志器。
//!
//! 三类任务的事件枚举不同，但日志收集、截断与推送方式完全一致；事件由调用方通过
//! 构造闭包生成，避免每个任务维护一份逐行复制的 Logger。

use std::collections::VecDeque;

use tokio::sync::mpsc::UnboundedSender;

use crate::models::LogLevel;

/// 单条任务记录最多保留的日志行数（超出后丢弃最早的日志）。
const MAX_LOG_LINES: usize = 8000;

/// 通用任务日志器：收集日志行供持久化，并实时推送事件。
pub struct TaskLogger<E> {
    lines: VecDeque<String>,
    events: Option<UnboundedSender<E>>,
    make_log: fn(LogLevel, String) -> E,
    make_progress: Option<fn(u8, String) -> E>,
}

impl<E> TaskLogger<E> {
    pub fn new(
        events: Option<UnboundedSender<E>>,
        make_log: fn(LogLevel, String) -> E,
        make_progress: Option<fn(u8, String) -> E>,
    ) -> Self {
        Self {
            lines: VecDeque::new(),
            events,
            make_log,
            make_progress,
        }
    }

    pub fn send(&self, event: E) {
        if let Some(sender) = &self.events {
            let _ = sender.send(event);
        }
    }

    /// 取回事件发送端。发送端被取出后日志器不再推送事件；任务结束时用它在
    /// 日志器仍存活时发送 `Finished`，接收端靠所有发送端 drop 判断任务收尾。
    pub fn take_events(&mut self) -> Option<UnboundedSender<E>> {
        self.events.take()
    }

    pub fn line(&mut self, level: LogLevel, message: impl Into<String>) {
        let text = format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            message.into()
        );
        if self.lines.len() >= MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(text.clone());
        self.send((self.make_log)(level, text));
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Info, message);
    }

    pub fn command(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Command, message);
    }

    pub fn success(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Success, message);
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Warn, message);
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Error, message);
    }

    /// 推送进度；未配置进度事件的调用方（如 Pages）自动忽略。
    pub fn progress(&self, percent: u8, message: &str) {
        if let Some(make) = self.make_progress {
            self.send(make(percent, message.to_string()));
        }
    }

    /// 汇总所有日志行，用于写入任务记录。
    pub fn joined(&self) -> String {
        self.lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum TestEvent {
        Log(LogLevel, String),
        Progress(u8, String),
    }

    fn logger(progress: bool) -> TaskLogger<TestEvent> {
        TaskLogger::new(
            None,
            TestEvent::Log,
            progress.then_some(TestEvent::Progress),
        )
    }

    #[test]
    fn keeps_timestamped_lines_in_order() {
        let mut logger = logger(false);
        logger.info("第一行");
        logger.warn("第二行");
        let joined = logger.joined();
        let lines: Vec<&str> = joined.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("] 第一行"));
        assert!(lines[1].ends_with("] 第二行"));
    }

    #[test]
    fn progress_is_ignored_without_progress_event() {
        let logger = logger(false);
        logger.progress(50, "上传中");
        assert!(logger.joined().is_empty());
    }
}
