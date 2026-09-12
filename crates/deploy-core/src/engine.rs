use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{
    now_string, DeployEvent, DeployRecord, DeployRequest, DeployStatus, LogLevel, RepoConfig,
    RepoInfo, ServerConfig, Settings,
};
use crate::process::shell_quote;
use crate::security::SecurityReport;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;

/// 单条部署记录最多保留的日志行数（超出后丢弃最早的日志）。
const MAX_LOG_LINES: usize = 8000;

/// 部署结束后自动删除本地临时归档（成功或失败都会触发）。
struct TempArchiveGuard(PathBuf);

impl Drop for TempArchiveGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 部署事件发送端（GUI 转发为 Tauri 事件，CLI 直接打印）。
pub type EventSender = UnboundedSender<DeployEvent>;

/// 部署引擎：组织“打包 -> 上传 -> 解压 -> 执行脚本”的完整流程。
pub struct DeployEngine {
    store: Arc<Store>,
}

impl DeployEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// 校验参数、解析版本并生成一条“运行中”的部署记录（写入历史）。
    pub fn prepare(&self, req: &DeployRequest) -> Result<DeployRecord> {
        let config = self.store.load_config()?;
        let repo = Store::find_repo(&config, &req.repo_id)?.clone();
        let server = Store::find_server(&config, &req.server_id)?.clone();

        let target_dir = req.target_dir.trim().to_string();
        if target_dir.is_empty() {
            return Err(CoreError::deploy("请选择部署目录"));
        }

        let git = Git::open(&repo.path)?;
        let resolved = git.resolve(&req.rev)?;

        let record = DeployRecord {
            id: uuid::Uuid::new_v4().to_string(),
            repo_id: repo.id.clone(),
            repo_name: repo.name.clone(),
            rev: req.rev.trim().to_string(),
            branch: req.rev.trim().to_string(),
            commit: resolved.hash.clone(),
            commit_short: resolved.short.clone(),
            commit_subject: resolved.subject.clone(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            target_dir,
            script_dir: normalize_script_dir(&req.script_dir, &config.settings.script_dir),
            script: req
                .script
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            run_scripts: req.run_scripts,
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };

        self.store
            .upsert_history(&record, config.settings.history_limit)?;
        Ok(record)
    }

    /// 执行部署流程。无论成功失败都会返回带有最终状态的记录。
    pub async fn run(
        &self,
        mut record: DeployRecord,
        req: DeployRequest,
        events: Option<EventSender>,
    ) -> DeployRecord {
        let started = Instant::now();
        let mut logger = Logger::new(events.clone());

        let _ = logger.send(DeployEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始部署 {} @ {} -> {}",
            record.repo_name, record.commit_short, record.server_name
        ));

        let result = self.execute(&record, &req, &mut logger).await;

        match result {
            Ok(()) => {
                record.status = DeployStatus::Success;
                logger.success(format!(
                    "部署成功（耗时 {}）",
                    format_duration(started.elapsed().as_millis() as u64)
                ));
            }
            Err(err) => {
                record.status = DeployStatus::Failed;
                record.error = Some(err.to_string());
                logger.error(format!("部署失败: {err}"));
            }
        }

        record.log = join_log_lines(&logger);
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        let limit = self
            .store
            .load_config()
            .map(|config| config.settings.history_limit)
            .unwrap_or(500);
        if let Err(first_err) = self.store.upsert_history(&record, limit) {
            // 写入失败不能让最终状态静默丢失：记录错误事件（CLI/GUI 均可见）后重试一次。
            logger.error(format!("保存部署记录失败: {first_err}"));
            record.log = join_log_lines(&logger);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if let Err(second_err) = self.store.upsert_history(&record, limit) {
                logger.error(format!("重试保存部署记录仍失败: {second_err}"));
                record.log = join_log_lines(&logger);
            }
        }

        if let Some(sender) = events {
            let _ = sender.send(DeployEvent::Finished {
                record: record.clone(),
            });
        }
        record
    }

    async fn execute(
        &self,
        record: &DeployRecord,
        req: &DeployRequest,
        logger: &mut Logger,
    ) -> Result<()> {
        let config = self.store.load_config()?;
        let settings = config.settings.clone();
        let repo = Store::find_repo(&config, &record.repo_id)?.clone();
        let server = Store::find_server(&config, &record.server_id)?.clone();

        let target = record.target_dir.trim().trim_end_matches('/').to_string();
        if target.is_empty() {
            return Err(CoreError::deploy("部署目录不能为空"));
        }

        logger.info(format!("仓库: {} ({})", repo.name, repo.path));
        logger.info(format!(
            "版本: {} [{}] {}",
            record.commit_short, record.rev, record.commit_subject
        ));
        logger.info(format!(
            "服务器: {} ({}@{})",
            server.name, server.username, server.host
        ));
        logger.info(format!("部署目录: {target}"));

        // 1. 本地打包
        logger.info("正在打包代码 ...");
        // 文件名带记录 ID，避免同一仓库/提交并发部署时互相覆盖临时归档。
        let stem = format!(
            "{}-{}-{}",
            sanitize_component(&repo.name),
            short_hash(&record.commit),
            record.id
        );
        let archive_name = format!("{stem}.tar.gz");
        let tar_path = self.store.prepare_temp_file(&format!("{stem}.tar"))?;
        let _tar_guard = TempArchiveGuard(tar_path.clone());
        let gz_path = self.store.prepare_temp_file(&archive_name)?;
        // 部署结束（含失败）后删除本地临时压缩包，避免 temp 目录无限增长。
        let _archive_guard = TempArchiveGuard(gz_path.clone());
        let size = {
            let repo_path = repo.path.clone();
            let commit = record.commit.clone();
            let tar = tar_path.clone();
            let gz = gz_path.clone();
            tokio::task::spawn_blocking(move || -> Result<u64> {
                let git = Git::open(&repo_path)?;
                git.archive(&commit, &tar, &gz)
            })
            .await
            .map_err(|err| CoreError::deploy(format!("打包任务异常: {err}")))??
        };
        logger.success(format!(
            "打包完成: {} ({})",
            archive_name,
            human_size(size)
        ));

        // 2. 建立 SSH 连接
        logger.info(format!("正在连接服务器 {} ...", server.name));
        let client = SshClient::connect(&server, settings.connect_timeout_secs).await?;
        logger.success(format!("SSH 连接成功 ({})", client.label()));

        // 3. 准备目录并上传
        let remote_dir = format!("{target}/.deploy_code");
        client.mkdir_p(&target).await?;
        client.mkdir_p(&remote_dir).await?;
        let remote_archive = format!("{remote_dir}/{archive_name}");

        logger.info("正在上传代码包 ...");
        let mut last_percent = u8::MAX;
        client
            .upload_file(&gz_path, &remote_archive, &mut |sent, total| {
                let percent = if total > 0 {
                    ((sent * 100 / total).min(100)) as u8
                } else {
                    0
                };
                if percent != last_percent {
                    last_percent = percent;
                    logger.progress(percent, &format!("上传中 {percent}%"));
                }
            })
            .await?;
        logger.success(format!("上传完成 ({})", human_size(size)));

        // 4. 解压到目标目录
        logger.info("正在解压到目标目录 ...");
        let extract_cmd = format!(
            "tar -xzf {} -C {}",
            shell_quote(&remote_archive),
            shell_quote(&target)
        );
        let (code, output) = client
            .exec_capture(&extract_cmd, settings.script_timeout_secs)
            .await?;
        if code != 0 {
            return Err(CoreError::deploy(format!(
                "解压失败（退出码 {code}）: {}",
                output.trim()
            )));
        }
        logger.success("解压完成");

        if !settings.keep_remote_archive {
            let _ = client
                .exec_capture(&format!("rm -f {}", shell_quote(&remote_archive)), 60)
                .await;
        }

        // 5. 执行项目脚本
        if record.run_scripts {
            self.run_scripts(record, req, &client, logger, &settings)
                .await?;
        } else {
            logger.info("已按部署选项跳过脚本执行");
        }

        client.disconnect().await;
        Ok(())
    }

    async fn run_scripts(
        &self,
        record: &DeployRecord,
        req: &DeployRequest,
        client: &SshClient,
        logger: &mut Logger,
        settings: &Settings,
    ) -> Result<()> {
        let target = req.target_dir.trim().trim_end_matches('/');
        let script_dir = normalize_script_dir(&req.script_dir, &settings.script_dir);

        let scripts: Vec<String> = match req
            .script
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(name) => {
                let path = if name.contains('/') {
                    name.trim_start_matches("./").to_string()
                } else {
                    format!("{script_dir}/{name}")
                };
                let cmd = format!(
                    "cd {} && test -f {} && echo {}",
                    shell_quote(target),
                    shell_quote(&path),
                    shell_quote(&path)
                );
                let (code, output) = client.exec_capture(&cmd, 60).await?;
                if code != 0 {
                    return Err(CoreError::deploy(format!("未找到脚本: {path}")));
                }
                vec![output.trim().to_string()]
            }
            None => {
                let cmd = format!(
                    "cd {} && find {} -maxdepth 1 -type f -name '*.sh' 2>/dev/null | sort",
                    shell_quote(target),
                    shell_quote(&script_dir)
                );
                let (_, output) = client.exec_capture(&cmd, 60).await?;
                output
                    .lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty() && !line.starts_with("[stderr]"))
                    .collect()
            }
        };

        if scripts.is_empty() {
            logger.warn(format!("未在 {script_dir}/ 目录找到 .sh 脚本，跳过执行"));
            return Ok(());
        }

        logger.info(format!("共找到 {} 个脚本，开始执行", scripts.len()));
        for script in scripts {
            logger.command(format!("$ bash {script}"));
            let pidfile = remote_pidfile(target, &record.id);
            // 记录脚本 PID：退出/超时时据此找到进程组并整组终止（含脚本拉起的子进程）。
            let inner = format!(
                "echo $$ > {pid} && exec bash {script}",
                pid = shell_quote(&pidfile),
                script = shell_quote(&script)
            );
            let cmd = format!(
                "cd {} && rm -f {} && DEPLOY_BRANCH={} DEPLOY_REV={} DEPLOY_COMMIT={} DEPLOY_TARGET={} bash -c {}",
                shell_quote(target),
                shell_quote(&pidfile),
                shell_quote(&record.branch),
                shell_quote(&record.rev),
                shell_quote(&record.commit),
                shell_quote(target),
                shell_quote(&inner)
            );
            let result = client
                .exec_stream(&cmd, settings.script_timeout_secs, &mut |kind, line| {
                    match kind {
                        OutputKind::Stdout => logger.info(line),
                        OutputKind::Stderr => logger.warn(line),
                    }
                })
                .await;
            match result {
                Ok(0) => {
                    let _ = remove_pidfile(client, &pidfile).await;
                    logger.success(format!("脚本执行完成: {script}"));
                }
                Ok(code) => {
                    let _ = remove_pidfile(client, &pidfile).await;
                    return Err(CoreError::deploy(format!(
                        "脚本执行失败（退出码 {code}）: {script}"
                    )));
                }
                Err(err) => {
                    // 超时或连接异常：脚本可能仍在运行，先终止再返回。
                    let _ = kill_remote_script(client, &pidfile).await;
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// 应用退出时清理仍在运行的远端部署脚本（重新连接并终止其进程组）。
    pub async fn cleanup_remote_script(
        &self,
        server: &ServerConfig,
        target_dir: &str,
        record_id: &str,
        timeout_secs: u64,
    ) -> Result<()> {
        let target = target_dir.trim().trim_end_matches('/');
        if target.is_empty() {
            return Ok(());
        }
        let pidfile = remote_pidfile(target, record_id);
        let client = SshClient::connect(server, timeout_secs).await?;
        let result = client.exec_capture(&kill_script(&pidfile), timeout_secs).await;
        client.disconnect().await;
        result.map(|_| ())
    }

    /// 测试服务器连通性，返回远端系统信息。
    pub async fn test_server(&self, server: &ServerConfig) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        let client = SshClient::connect(server, settings.connect_timeout_secs).await?;
        let (code, output) = client
            .exec_capture("uname -srm 2>/dev/null || echo unknown", 30)
            .await?;
        client.disconnect().await;
        if code != 0 {
            return Err(CoreError::ssh("连接成功，但执行远端命令失败"));
        }
        Ok(format!("连接成功 · {}", output.trim()))
    }

    /// 采集服务器安全检查报告（登录日志 / 防火墙 / sshd 配置 / 自动防护状态）。
    pub async fn server_security(&self, server: &ServerConfig) -> Result<SecurityReport> {
        let settings = self.store.load_config()?.settings;
        crate::security::collect(server, settings.connect_timeout_secs).await
    }

    /// 启用服务器端自动防护（失败 N 次自动拉黑，服务器上长期生效）。
    pub async fn enable_server_guard(
        &self,
        server: &ServerConfig,
        threshold: u32,
        window_mins: u64,
    ) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::enable_guard(server, settings.connect_timeout_secs, threshold, window_mins)
            .await
    }

    /// 停用服务器端自动防护。
    pub async fn disable_server_guard(&self, server: &ServerConfig) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::disable_guard(server, settings.connect_timeout_secs).await
    }

    /// 拉黑某来源 IP。
    pub async fn block_server_ip(&self, server: &ServerConfig, ip: &str) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::block_ip(server, settings.connect_timeout_secs, ip).await
    }

    /// 解除某来源 IP 的拉黑。
    pub async fn unblock_server_ip(&self, server: &ServerConfig, ip: &str) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::unblock_ip(server, settings.connect_timeout_secs, ip).await
    }

    /// 强制踢出服务器上的在线会话。
    pub async fn kick_server_session(&self, server: &ServerConfig, tty: &str) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::kick_session(server, settings.connect_timeout_secs, tty).await
    }
}

/// 计算仓库的展示信息（当前分支 / 远端 / 变动数量）。
pub fn repo_info(repo: &RepoConfig) -> RepoInfo {
    match Git::open(&repo.path) {
        Ok(git) => {
            let current_branch = git.current_branch().unwrap_or_else(|_| "-".to_string());
            let remote = git.remote_url().ok().flatten();
            let change_count = git.status().map(|s| s.changes.len()).unwrap_or(0);
            RepoInfo {
                id: repo.id.clone(),
                name: repo.name.clone(),
                path: repo.path.clone(),
                is_repo: true,
                current_branch,
                remote,
                change_count,
                default_server_id: repo.default_server_id.clone(),
                default_target_dir: repo.default_target_dir.clone(),
            }
        }
        Err(_) => RepoInfo {
            id: repo.id.clone(),
            name: repo.name.clone(),
            path: repo.path.clone(),
            is_repo: false,
            current_branch: "-".to_string(),
            remote: None,
            change_count: 0,
            default_server_id: repo.default_server_id.clone(),
            default_target_dir: repo.default_target_dir.clone(),
        },
    }
}

/// 收集日志并按需推送给事件通道。
struct Logger {
    lines: VecDeque<String>,
    events: Option<EventSender>,
}

impl Logger {
    fn new(events: Option<EventSender>) -> Self {
        Self {
            lines: VecDeque::new(),
            events,
        }
    }

    fn send(&self, event: DeployEvent) -> Option<()> {
        self.events.as_ref().map(|sender| {
            let _ = sender.send(event);
        })
    }

    fn line(&mut self, level: LogLevel, message: impl Into<String>) {
        let text = format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            message.into()
        );
        if self.lines.len() >= MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(text.clone());
        self.send(DeployEvent::Log {
            level,
            message: text,
        });
    }

    fn info(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Info, message);
    }

    fn command(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Command, message);
    }

    fn success(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Success, message);
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Warn, message);
    }

    fn error(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Error, message);
    }

    fn progress(&self, percent: u8, message: &str) {
        self.send(DeployEvent::Progress {
            percent,
            message: message.to_string(),
        });
    }
}

fn join_log_lines(logger: &Logger) -> String {
    logger.lines.iter().cloned().collect::<Vec<_>>().join("\n")
}

fn normalize_script_dir(value: &str, fallback: &str) -> String {
    let value = value.trim().trim_matches('/');
    let value = value.strip_prefix("./").unwrap_or(value);
    if !value.is_empty() {
        value.to_string()
    } else {
        let fallback = fallback.trim().trim_matches('/');
        let fallback = fallback.strip_prefix("./").unwrap_or(fallback);
        if fallback.is_empty() {
            "docker".to_string()
        } else {
            fallback.to_string()
        }
    }
}

/// 远端脚本 pidfile 路径（放在部署目录的 .deploy_code 下）。
fn remote_pidfile(target: &str, record_id: &str) -> String {
    format!(
        "{}/.deploy_code/deploy-{record_id}.pid",
        target.trim_end_matches('/')
    )
}

/// 终止 pidfile 中记录的脚本进程组，并删除 pidfile。
fn kill_script(pidfile: &str) -> String {
    let file = shell_quote(pidfile);
    // 先发信号再删 pidfile：中途被中断时，脚本仍可被下次清理定位到。
    // 进程组优先从 /proc 读取（Linux 通用），失败再退回 ps，兼容精简系统的 ps。
    format!(
        "f={file}; p=$(cat \"$f\" 2>/dev/null); \
         case \"$p\" in ''|*[!0-9]*) rm -f \"$f\"; exit 0 ;; esac; \
         g=$(sed 's/.*) //' /proc/$p/stat 2>/dev/null | cut -d' ' -f3); \
         case \"$g\" in ''|*[!0-9]*) g=$(ps -o pgid= -p \"$p\" 2>/dev/null | tr -d ' ') ;; esac; \
         if [ -n \"$g\" ]; then \
           kill -TERM -\"$g\" 2>/dev/null || kill -TERM \"$p\" 2>/dev/null; \
           sleep 1; \
           kill -KILL -\"$g\" 2>/dev/null || kill -KILL \"$p\" 2>/dev/null; \
         else \
           kill -TERM \"$p\" 2>/dev/null; sleep 1; kill -KILL \"$p\" 2>/dev/null; \
         fi; rm -f \"$f\"; echo done"
    )
}

async fn remove_pidfile(client: &SshClient, pidfile: &str) -> Result<()> {
    client
        .exec_capture(&format!("rm -f {}", shell_quote(pidfile)), 15)
        .await
        .map(|_| ())
}

async fn kill_remote_script(client: &SshClient, pidfile: &str) -> Result<()> {
    client
        .exec_capture(&kill_script(pidfile), 15)
        .await
        .map(|_| ())
}

fn sanitize_component(value: &str) -> String {
    let mut result: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    result.truncate(40);
    if result.is_empty() {
        "repo".to_string()
    } else {
        result
    }
}

fn short_hash(hash: &str) -> String {
    hash.chars().take(8).collect()
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.2} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:.0}s", ms / 60_000, (ms % 60_000) as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_pidfile_normalizes_trailing_slash() {
        assert_eq!(
            remote_pidfile("/srv/app/", "abc"),
            "/srv/app/.deploy_code/deploy-abc.pid"
        );
    }

    #[test]
    fn kill_script_quotes_pidfile_and_kills_group() {
        let script = kill_script("/srv/my app/.deploy_code/deploy-a.pid");
        assert!(script.contains("'/srv/my app/.deploy_code/deploy-a.pid'"));
        assert!(script.contains("/proc/$p/stat"));
        assert!(script.contains("kill -TERM -\"$g\""));
        assert!(script.contains("kill -KILL -\"$g\""));
    }

    #[test]
    fn kill_script_ignores_invalid_pid_content() {
        let script = kill_script("/tmp/x.pid");
        assert!(script.contains("*[!0-9]*"));
    }
}
