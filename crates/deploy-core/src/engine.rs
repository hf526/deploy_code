use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{
    now_string, DeployEvent, DeployRecord, DeployRequest, DeployStatus, EnvFileConfig, LogLevel,
    RepoConfig, RepoInfo, ServerConfig, Settings,
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
        if !git.is_repo() {
            return Err(CoreError::git(format!(
                "{} 尚未初始化 Git 仓库，请先在仓库页绑定远端仓库地址",
                repo.name
            )));
        }
        let resolved = git.resolve(&req.rev)?;

        // 环境文件：解压后用本地文件覆盖服务器上的对应文件；提前校验，避免部署开始后才报错。
        let env_files = if req.upload_env {
            normalize_env_files(&repo.env_files)?
        } else {
            Vec::new()
        };
        for file in &env_files {
            if !std::path::Path::new(&file.local_path).is_file() {
                return Err(CoreError::deploy(format!(
                    "环境文件不存在: {}",
                    file.local_path
                )));
            }
        }

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
            scripts: normalize_scripts(&req.scripts),
            run_scripts: req.run_scripts,
            env_files,
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

        // 上传、解压、替换环境文件与执行脚本包成一体：无论哪一步失败，
        // 都在最后统一清理远端压缩包，避免失败/中止时在服务器上留下残包。
        let deployment: Result<()> = async {
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

            // 5. 上传并替换环境文件（在脚本执行前覆盖，确保脚本读到的是新内容）
            if !record.env_files.is_empty() {
                logger.info(format!("正在上传 {} 个环境文件 ...", record.env_files.len()));
                for file in &record.env_files {
                    let local = std::path::Path::new(&file.local_path);
                    let remote = format!("{}/{}", target.trim_end_matches('/'), file.remote_path);
                    if let Some((parent, _)) = file.remote_path.rsplit_once('/') {
                        client
                            .mkdir_p(&format!("{}/{}", target.trim_end_matches('/'), parent))
                            .await?;
                    }
                    // 先上传到同目录临时文件，再原子替换：上传中断（断网/磁盘满）时
                    // 不会截断服务器上原有的环境文件。
                    let temp = format!("{remote}.deploycode-tmp");
                    if let Err(err) = client.upload_file(local, &temp, &mut |_, _| {}).await {
                        let _ = client
                            .exec_capture(&format!("rm -f {}", shell_quote(&temp)), 30)
                            .await;
                        return Err(CoreError::deploy(format!(
                            "上传环境文件失败 {}: {err}",
                            file.remote_path
                        )));
                    }
                    // mv 会用临时文件的权限覆盖旧文件，先记录原权限再恢复。
                    let replace = format!(
                        "p=$(stat -c %a {remote} 2>/dev/null || stat -f %Lp {remote} 2>/dev/null); \
                         mv -f {temp} {remote}; \
                         if [ -n \"$p\" ]; then chmod \"$p\" {remote}; fi",
                        remote = shell_quote(&remote),
                        temp = shell_quote(&temp)
                    );
                    let (code, output) =
                        client.exec_capture(&replace, 60).await.map_err(|err| {
                            CoreError::deploy(format!(
                                "替换环境文件失败 {}: {err}",
                                file.remote_path
                            ))
                        })?;
                    if code != 0 {
                        let _ = client
                            .exec_capture(&format!("rm -f {}", shell_quote(&temp)), 30)
                            .await;
                        return Err(CoreError::deploy(format!(
                            "替换环境文件失败 {}: {}",
                            file.remote_path,
                            output.trim()
                        )));
                    }
                    logger.success(format!("已替换环境文件: {}", file.remote_path));
                }
            }

            // 6. 执行项目脚本
            if record.run_scripts {
                self.run_scripts(record, req, &client, logger, &settings)
                    .await?;
            } else {
                logger.info("已按部署选项跳过脚本执行");
            }

            Ok(())
        }
        .await;

        if !settings.keep_remote_archive {
            let _ = client
                .exec_capture(&format!("rm -f {}", shell_quote(&remote_archive)), 60)
                .await;
        }

        deployment?;

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

        let explicit = normalize_scripts(&req.scripts);
        let scripts: Vec<String> = if explicit.is_empty() {
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
        } else {
            // 先全部校验存在性，避免执行到一半才发现缺失脚本。
            let mut paths = Vec::with_capacity(explicit.len());
            for name in explicit {
                let path = if name.contains('/') {
                    name.trim_start_matches("./").to_string()
                } else {
                    format!("{script_dir}/{name}")
                };
                let cmd = format!(
                    "cd {} && test -f {}",
                    shell_quote(target),
                    shell_quote(&path)
                );
                let (code, _) = client.exec_capture(&cmd, 60).await?;
                if code != 0 {
                    return Err(CoreError::deploy(format!("未找到脚本: {path}")));
                }
                paths.push(path);
            }
            paths
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
        let keep_remote_archive = self
            .store
            .load_config()
            .map(|config| config.settings.keep_remote_archive)
            .unwrap_or(false);
        let pidfile = remote_pidfile(target, record_id);
        // 任务被中止时压缩包会以记录 UUID 命名残留在 .deploy_code 下，一并清理。
        // record_id 是 UUID，可安全拼进 glob；kill_script 已用子 shell 包裹，不会中断后续命令。
        let mut command = kill_script(&pidfile);
        if !keep_remote_archive {
            command.push_str(&format!(
                "; rm -f {}/.deploy_code/*-{record_id}.tar.gz",
                shell_quote(target)
            ));
        }
        let client = SshClient::connect(server, timeout_secs).await?;
        let result = client.exec_capture(&command, timeout_secs).await;
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
    let path_exists = std::path::Path::new(&repo.path).is_dir();
    match Git::open(&repo.path) {
        Ok(git) if git.is_repo() => {
            let current_branch = git.current_branch().unwrap_or_else(|_| "-".to_string());
            let remote = git.remote_url().ok().flatten();
            let change_count = git.status().map(|s| s.changes.len()).unwrap_or(0);
            RepoInfo {
                id: repo.id.clone(),
                name: repo.name.clone(),
                path: repo.path.clone(),
                path_exists,
                is_repo: true,
                current_branch,
                remote,
                change_count,
                default_server_id: repo.default_server_id.clone(),
                default_target_dir: repo.default_target_dir.clone(),
                env_files: repo.env_files.clone(),
            }
        }
        _ => RepoInfo {
            id: repo.id.clone(),
            name: repo.name.clone(),
            path: repo.path.clone(),
            path_exists,
            is_repo: false,
            current_branch: "-".to_string(),
            remote: None,
            change_count: 0,
            default_server_id: repo.default_server_id.clone(),
            default_target_dir: repo.default_target_dir.clone(),
            env_files: repo.env_files.clone(),
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

/// 校验并规范化环境文件配置：跳过空条目，统一远端路径分隔符。
pub fn normalize_env_files(files: &[EnvFileConfig]) -> Result<Vec<EnvFileConfig>> {
    let mut normalized = Vec::with_capacity(files.len());
    for file in files {
        let local = file.local_path.trim();
        let remote = file.remote_path.trim();
        if local.is_empty() && remote.is_empty() {
            continue;
        }
        if local.is_empty() {
            return Err(CoreError::deploy("环境文件的本地路径不能为空"));
        }
        if remote.is_empty() {
            return Err(CoreError::deploy("环境文件的远端路径不能为空"));
        }
        normalized.push(EnvFileConfig {
            local_path: local.to_string(),
            remote_path: normalize_env_remote(remote)?,
        });
    }
    Ok(normalized)
}

/// 远端路径必须是部署目录下的相对路径：去掉 `./` 前缀，拒绝绝对路径与 `..`。
fn normalize_env_remote(value: &str) -> Result<String> {
    let rel = value.trim().replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    let parts: Vec<&str> = rel.split('/').collect();
    let invalid = rel.is_empty()
        || rel.starts_with('/')
        || rel.chars().any(char::is_control)
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..");
    if invalid {
        return Err(CoreError::deploy(format!("环境文件远端路径不合法: {value}")));
    }
    Ok(parts.join("/"))
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

/// 脚本列表去空白、去重，并保持用户给定顺序。
fn normalize_scripts(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_string()))
        .map(|value| value.to_string())
        .collect()
}

/// 远端脚本 pidfile 路径（放在部署目录的 .deploy_code 下）。
fn remote_pidfile(target: &str, record_id: &str) -> String {
    format!(
        "{}/.deploy_code/deploy-{record_id}.pid",
        target.trim_end_matches('/')
    )
}

/// 终止 pidfile 中记录的脚本进程组，并删除 pidfile。
pub(crate) fn kill_script(pidfile: &str) -> String {
    let file = shell_quote(pidfile);
    // 先发信号再删 pidfile：中途被中断时，脚本仍可被下次清理定位到。
    // 进程组优先从 /proc 读取（Linux 通用），失败再退回 ps，兼容精简系统的 ps。
    // 整段包在子 shell 里：pidfile 缺失时的 exit 0 只退出子 shell，
    // 不会中断调用方拼在其后的 rm 等清理命令。
    format!(
        "( f={file}; p=$(cat \"$f\" 2>/dev/null); \
         case \"$p\" in ''|*[!0-9]*) rm -f \"$f\"; exit 0 ;; esac; \
         g=$(sed 's/.*) //' /proc/$p/stat 2>/dev/null | cut -d' ' -f3); \
         case \"$g\" in ''|*[!0-9]*) g=$(ps -o pgid= -p \"$p\" 2>/dev/null | tr -d ' ') ;; esac; \
         if [ -n \"$g\" ]; then \
           kill -TERM -\"$g\" 2>/dev/null || kill -TERM \"$p\" 2>/dev/null; \
           sleep 1; \
           kill -KILL -\"$g\" 2>/dev/null || kill -KILL \"$p\" 2>/dev/null; \
         else \
           kill -TERM \"$p\" 2>/dev/null; sleep 1; kill -KILL \"$p\" 2>/dev/null; \
         fi; rm -f \"$f\"; echo done )"
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
        // 无效 pid 分支的 exit 0 必须只退出子 shell，不能中断调用方的后续清理命令。
        assert!(script.starts_with("( f="), "script = {script}");
        assert!(script.ends_with(')'), "script = {script}");
    }

    #[test]
    fn normalize_env_files_skips_empty_and_rejects_unsafe_paths() {
        let files = normalize_env_files(&[
            EnvFileConfig {
                local_path: " .env ".to_string(),
                remote_path: " .env ".to_string(),
            },
            EnvFileConfig {
                local_path: "C:/tmp/app.env".to_string(),
                remote_path: "docker\\app.env".to_string(),
            },
            EnvFileConfig {
                local_path: "C:/tmp/root.env".to_string(),
                remote_path: "././.env".to_string(),
            },
            EnvFileConfig::default(),
        ])
        .unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].local_path, ".env");
        assert_eq!(files[0].remote_path, ".env");
        assert_eq!(files[1].remote_path, "docker/app.env");
        // `./` 前缀会被去掉，但 `.env` 这类隐藏文件名不能被误伤。
        assert_eq!(files[2].remote_path, ".env");

        for bad in ["/etc/passwd", "../secret", "a/../b", "a//b", "./", "..", "a/"] {
            let files = vec![EnvFileConfig {
                local_path: "x".to_string(),
                remote_path: bad.to_string(),
            }];
            assert!(normalize_env_files(&files).is_err(), "应拒绝远端路径 {bad}");
        }

        let files = vec![EnvFileConfig {
            local_path: String::new(),
            remote_path: ".env".to_string(),
        }];
        assert!(normalize_env_files(&files).is_err());
    }

    #[test]
    fn prepare_validates_and_applies_env_files() {
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !git_ok {
            return;
        }
        let base =
            std::env::temp_dir().join(format!("deploycode-env-prepare-{}", uuid::Uuid::new_v4()));
        let repo_dir = base.join("work");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo_arg = repo_dir.to_string_lossy().into_owned();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_arg)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} 失败");
        };
        git(&["init"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo_dir.join("a.txt"), "hi").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-m", "init"]);

        let env_path = base.join("deploy.env");
        std::fs::write(&env_path, "KEY=1\n").unwrap();

        let store = std::sync::Arc::new(Store::new(base.join("data")));
        let (repo_id, server_id) = store
            .mutate_config(|config| {
                let mut repo = RepoConfig::new("demo".to_string(), repo_arg.clone());
                repo.env_files = vec![EnvFileConfig {
                    local_path: env_path.to_string_lossy().into_owned(),
                    remote_path: "docker\\.env".to_string(),
                }];
                config.repos.push(repo.clone());
                let server = ServerConfig::new(
                    "prod".to_string(),
                    "127.0.0.1".to_string(),
                    "root".to_string(),
                    crate::models::SshAuth::Password {
                        password: "x".to_string(),
                    },
                );
                let server_id = server.id.clone();
                config.servers.push(server);
                Ok((repo.id, server_id))
            })
            .unwrap();

        let engine = DeployEngine::new(store.clone());
        let request = DeployRequest {
            repo_id,
            rev: "HEAD".to_string(),
            server_id,
            target_dir: "/opt/demo".to_string(),
            run_scripts: false,
            script_dir: "docker".to_string(),
            scripts: Vec::new(),
            upload_env: true,
        };

        let record = engine.prepare(&request).unwrap();
        assert_eq!(record.env_files.len(), 1);
        assert_eq!(record.env_files[0].remote_path, "docker/.env");

        // 本地文件缺失时在开始部署前报错。
        std::fs::remove_file(&env_path).unwrap();
        let err = engine.prepare(&request).unwrap_err().to_string();
        assert!(err.contains("环境文件不存在"), "err = {err}");

        // uploadEnv=false 时不校验也不记录。
        std::fs::write(&env_path, "KEY=2\n").unwrap();
        let mut skip = request.clone();
        skip.upload_env = false;
        let record = engine.prepare(&skip).unwrap();
        assert!(record.env_files.is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn repo_info_distinguishes_non_git_and_missing_dirs() {        let dir =
            std::env::temp_dir().join(format!("deploycode-repo-info-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let repo = RepoConfig::new("plain".to_string(), dir.to_string_lossy().into_owned());

        let info = repo_info(&repo);
        assert!(info.path_exists);
        assert!(!info.is_repo);
        assert_eq!(info.change_count, 0);

        let missing = RepoConfig::new(
            "gone".to_string(),
            dir.join("not-exist").to_string_lossy().into_owned(),
        );
        let info = repo_info(&missing);
        assert!(!info.path_exists);
        assert!(!info.is_repo);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
