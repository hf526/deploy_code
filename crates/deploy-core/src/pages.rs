//! Cloudflare Pages 一键部署：本地构建（可选）后调用 wrangler CLI 上传产物。
//!
//! 依赖本机的 Node.js / npx（wrangler 通过 `npx --yes wrangler` 调用），
//! API Token 与 Account ID 通过环境变量传递，不写入命令行参数。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{
    now_string, DeployStatus, LogLevel, PagesConfig, PagesDeployRecord, PagesEvent, PagesRequest,
};
use crate::process::{run_shell_stream, run_stream};
use crate::ssh::OutputKind;
use crate::store::Store;

/// 单条记录最多保留的日志行数。
const MAX_LOG_LINES: usize = 8000;

/// Pages 部署事件发送端（GUI 转发为 Tauri 事件，CLI 直接打印）。
pub type PagesEventSender = UnboundedSender<PagesEvent>;

/// 已校验并解析好的 Pages 部署任务。
pub struct PreparedPagesDeploy {
    pub record: PagesDeployRecord,
    pub config: PagesConfig,
    pub repo_path: PathBuf,
    pub token: String,
    pub account_id: String,
}

/// Cloudflare Pages 部署引擎。
pub struct PagesEngine {
    store: Arc<Store>,
}

impl PagesEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    /// 校验参数、解析配置，并生成一条「运行中」的部署记录（写入历史）。
    pub fn prepare(&self, req: &PagesRequest) -> Result<PreparedPagesDeploy> {
        let app_config = self.store.load_config()?;
        let repo = Store::find_repo(&app_config, &req.repo_id)?.clone();

        let mut config = repo.pages.clone().unwrap_or_default();
        if let Some(value) = non_empty(&req.project_name) {
            config.project_name = value.to_string();
        }
        if let Some(value) = non_empty(&req.build_command) {
            config.build_command = value.to_string();
        }
        if let Some(value) = non_empty(&req.output_dir) {
            config.output_dir = value.to_string();
        }
        if let Some(value) = non_empty(&req.branch) {
            config.branch = value.to_string();
        }
        validate_config(&config)?;

        let token = app_config.settings.cloudflare_api_token.trim().to_string();
        if token.is_empty() {
            return Err(CoreError::config("请先在设置页填写 Cloudflare API Token"));
        }
        let account_id = app_config
            .settings
            .cloudflare_account_id
            .trim()
            .to_string();
        if account_id.is_empty() {
            return Err(CoreError::config("请先在设置页填写 Cloudflare Account ID"));
        }

        let git = Git::open(&repo.path).ok();
        let (commit, commit_short) = match git.and_then(|git| git.resolve("HEAD").ok()) {
            Some(rev) => (rev.hash, rev.short),
            None => (String::new(), String::new()),
        };

        let record = PagesDeployRecord {
            id: uuid::Uuid::new_v4().to_string(),
            repo_id: repo.id.clone(),
            repo_name: repo.name.clone(),
            project_name: config.project_name.clone(),
            branch: config.branch.clone(),
            commit,
            commit_short,
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            url: None,
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };
        self.store
            .upsert_pages_record(&record, app_config.settings.pages_history_limit)?;

        Ok(PreparedPagesDeploy {
            record,
            config,
            repo_path: PathBuf::from(repo.path),
            token,
            account_id,
        })
    }

    /// 执行部署（阻塞，调用方应放入阻塞线程池）。无论成败都返回最终记录。
    pub fn run(
        &self,
        mut record: PagesDeployRecord,
        config: PagesConfig,
        repo_path: PathBuf,
        token: String,
        account_id: String,
        skip_build: bool,
        events: Option<PagesEventSender>,
    ) -> PagesDeployRecord {
        let started = Instant::now();
        let mut logger = PagesLogger::new(events);

        let _ = logger.send(PagesEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始部署 {} -> Cloudflare Pages / {}",
            record.repo_name, record.project_name
        ));

        let result = self.execute(&config, &repo_path, &token, &account_id, skip_build, &mut logger);

        match result {
            Ok(url) => {
                record.status = DeployStatus::Success;
                record.url = url;
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
            .map(|config| config.settings.pages_history_limit)
            .unwrap_or(200);
        if let Err(err) = self.store.upsert_pages_record(&record, limit) {
            logger.error(format!("保存 Pages 部署记录失败: {err}"));
            record.log = join_log_lines(&logger);
        }

        if let Some(sender) = logger.events.take() {
            let _ = sender.send(PagesEvent::Finished {
                record: record.clone(),
            });
        }
        record
    }

    fn execute(
        &self,
        config: &PagesConfig,
        repo_path: &Path,
        token: &str,
        account_id: &str,
        skip_build: bool,
        logger: &mut PagesLogger,
    ) -> Result<Option<String>> {
        let app_config = self.store.load_config()?;
        let timeout = Duration::from_secs(app_config.settings.script_timeout_secs.max(120));
        let deploy_timeout = Duration::from_secs(app_config.settings.script_timeout_secs.max(600));

        // 1. 构建
        if skip_build {
            logger.info("已按选项跳过构建");
        } else if config.build_command.trim().is_empty() {
            logger.info("未配置构建命令，直接上传现有产物");
        } else {
            logger.command(format!("$ {}", config.build_command.trim()));
            let mut handler = |kind: OutputKind, line: String| match kind {
                OutputKind::Stdout => logger.info(line),
                OutputKind::Stderr => logger.warn(line),
            };
            let code = run_shell_stream(
                config.build_command.trim(),
                Some(repo_path),
                &[],
                timeout,
                &mut handler,
            )?;
            if code != 0 {
                return Err(CoreError::pages(format!("构建失败（退出码 {code}）")));
            }
            logger.success("构建完成");
        }

        // 2. 校验输出目录（必须在仓库内，防止把任意系统目录上传到公网）
        let output_path = resolve_output_dir(repo_path, &config.output_dir)?;

        // 3. 确保项目存在（已存在时 wrangler 返回错误，忽略并在日志中说明）
        logger.info(format!("检查 Pages 项目 {} ...", config.project_name));
        let create_args = vec![
            "--yes".to_string(),
            "wrangler".to_string(),
            "pages".to_string(),
            "project".to_string(),
            "create".to_string(),
            config.project_name.clone(),
            "--production-branch".to_string(),
            config.branch.clone(),
        ];
        let mut create_log = |kind: OutputKind, line: String| match kind {
            OutputKind::Stdout => logger.info(line),
            OutputKind::Stderr => logger.warn(line),
        };
        match run_stream(
            npx_program(),
            &create_args,
            Some(repo_path),
            &cf_envs(token, account_id),
            Duration::from_secs(120),
            &mut create_log,
        ) {
            Ok(0) => logger.success("Pages 项目已就绪"),
            Ok(_) => logger.info("项目已存在或创建被跳过，继续部署"),
            Err(err) => logger.warn(format!("项目检查失败（继续尝试部署）: {err}")),
        }

        // 4. 部署
        let output_arg = output_path.to_string_lossy().into_owned();
        logger.info(format!("正在上传 {output_arg} ..."));
        let deploy_args = vec![
            "--yes".to_string(),
            "wrangler".to_string(),
            "pages".to_string(),
            "deploy".to_string(),
            output_arg,
            "--project-name".to_string(),
            config.project_name.clone(),
            "--branch".to_string(),
            config.branch.clone(),
        ];
        let mut url: Option<String> = None;
        let mut deploy_log = |kind: OutputKind, line: String| {
            if let Some(found) = extract_pages_url(&line) {
                url = Some(found);
            }
            match kind {
                OutputKind::Stdout => logger.info(line),
                OutputKind::Stderr => logger.warn(line),
            }
        };
        let code = run_stream(
            npx_program(),
            &deploy_args,
            Some(repo_path),
            &cf_envs(token, account_id),
            deploy_timeout,
            &mut deploy_log,
        )?;
        if code != 0 {
            return Err(CoreError::pages(format!("wrangler 部署失败（退出码 {code}）")));
        }
        if let Some(url) = &url {
            logger.success(format!("部署地址: {url}"));
        } else {
            logger.info("未从输出中解析到部署地址，可到 Cloudflare 控制台查看");
        }
        Ok(url)
    }

    /// 检查部署环境：wrangler 可用性 + Token 是否有效，并返回账号信息。
    /// `config` 为 Some 时使用表单里未保存的配置（与保存逻辑同样的规范化）。
    pub fn test(&self, repo_id: &str, config: Option<PagesConfig>) -> Result<String> {
        let app_config = self.store.load_config()?;
        let repo = Store::find_repo(&app_config, repo_id)?.clone();
        let config = match config {
            Some(config) => normalize_config(config),
            None => repo.pages.clone().unwrap_or_default(),
        };
        validate_config(&config)?;

        let token = app_config.settings.cloudflare_api_token.trim().to_string();
        if token.is_empty() {
            return Err(CoreError::config("请先在设置页填写 Cloudflare API Token"));
        }
        let account_id = app_config
            .settings
            .cloudflare_account_id
            .trim()
            .to_string();
        if account_id.is_empty() {
            return Err(CoreError::config("请先在设置页填写 Cloudflare Account ID"));
        }

        let path = PathBuf::from(&repo.path);
        let mut output = String::new();
        let mut collect = |kind: OutputKind, line: String| {
            if kind == OutputKind::Stdout {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&line);
            }
        };
        let args = vec![
            "--yes".to_string(),
            "wrangler".to_string(),
            "whoami".to_string(),
        ];
        let code = run_stream(
            npx_program(),
            &args,
            Some(path.as_path()),
            &cf_envs(&token, &account_id),
            Duration::from_secs(120),
            &mut collect,
        )?;
        if code != 0 {
            return Err(CoreError::pages(format!(
                "wrangler 校验失败: {}",
                if output.trim().is_empty() {
                    "请检查 Node.js 与 API Token"
                } else {
                    output.trim()
                }
            )));
        }
        Ok(if output.trim().is_empty() {
            "wrangler 可用，Token 有效".to_string()
        } else {
            output.trim().to_string()
        })
    }
}

/// 与保存时一致地规范化表单配置：去空白并补默认值。
fn normalize_config(mut config: PagesConfig) -> PagesConfig {
    config.project_name = config.project_name.trim().to_string();
    config.build_command = config.build_command.trim().to_string();
    config.output_dir = config.output_dir.trim().to_string();
    config.branch = config.branch.trim().to_string();
    if config.output_dir.is_empty() {
        config.output_dir = "dist".to_string();
    }
    if config.branch.is_empty() {
        config.branch = "main".to_string();
    }
    config
}

fn validate_config(config: &PagesConfig) -> Result<()> {
    let project = config.project_name.trim();
    if project.is_empty() {
        return Err(CoreError::config("请先填写 Cloudflare Pages 项目名"));
    }
    if project.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CoreError::config("项目名不能包含空白字符"));
    }
    Ok(())
}

/// 解析并校验输出目录：必须是仓库内的相对路径，且规范化后仍位于仓库内。
fn resolve_output_dir(repo_path: &Path, output_dir: &str) -> Result<PathBuf> {
    let value = output_dir.trim().trim_end_matches(['/', '\\']);
    if value.is_empty() {
        return Err(CoreError::pages("输出目录不能为空"));
    }
    if value == "." || value == "./" {
        return Err(CoreError::pages(
            "输出目录不能是仓库根目录，请填写构建产物目录（如 dist）",
        ));
    }
    let rel = Path::new(value);
    if rel.is_absolute() || value.contains(':') {
        return Err(CoreError::pages("输出目录必须是仓库内的相对路径"));
    }
    if rel
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CoreError::pages("输出目录不能包含 .."));
    }

    let joined = repo_path.join(rel);
    if !joined.is_dir() {
        return Err(CoreError::pages(format!(
            "输出目录不存在: {}（请检查构建命令与输出目录配置）",
            joined.display()
        )));
    }
    // 解析符号链接后再确认仍在仓库内，避免链接逃逸。
    let canonical = std::fs::canonicalize(&joined)
        .map_err(|e| CoreError::pages(format!("输出目录不可用: {e}")))?;
    let repo_canonical = std::fs::canonicalize(repo_path)
        .map_err(|e| CoreError::pages(format!("仓库目录不可用: {e}")))?;
    if !canonical.starts_with(&repo_canonical) {
        return Err(CoreError::pages("输出目录必须在仓库内"));
    }
    Ok(canonical)
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn npx_program() -> &'static str {
    if cfg!(windows) {
        "npx.cmd"
    } else {
        "npx"
    }
}

fn cf_envs(token: &str, account_id: &str) -> Vec<(String, String)> {
    vec![
        ("CLOUDFLARE_API_TOKEN".to_string(), token.to_string()),
        ("CLOUDFLARE_ACCOUNT_ID".to_string(), account_id.to_string()),
    ]
}

/// 从 wrangler 输出中解析部署地址（...pages.dev），自动剥离 ANSI 颜色码。
fn extract_pages_url(line: &str) -> Option<String> {
    let cleaned = strip_ansi(line);
    let index = cleaned.find("https://")?;
    let rest = &cleaned[index..];
    // 只接受 URL 合法字符，遇到引号/括号/空白/控制字符即截断。
    let end = rest
        .find(|c: char| {
            !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '/' | '_'))
        })
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ')']);
    let host = url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("");
    if host.ends_with(".pages.dev") {
        Some(url.to_string())
    } else {
        None
    }
}

/// 去除 CSI ANSI 转义序列（如 `\x1b[36m`）。
fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            output.push(ch);
        }
    }
    output
}

// ---------------------------------------------------------------------------
// 日志
// ---------------------------------------------------------------------------

struct PagesLogger {
    lines: VecDeque<String>,
    events: Option<PagesEventSender>,
}

impl PagesLogger {
    fn new(events: Option<PagesEventSender>) -> Self {
        Self {
            lines: VecDeque::new(),
            events,
        }
    }

    fn send(&self, event: PagesEvent) -> Option<()> {
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
        self.send(PagesEvent::Log {
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
}

fn join_log_lines(logger: &PagesLogger) -> String {
    logger.lines.iter().cloned().collect::<Vec<_>>().join("\n")
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
    fn extract_pages_url_parses_wrangler_output() {
        assert_eq!(
            extract_pages_url("Deployment complete! Take a look at the deployment: https://abc123.my-site.pages.dev"),
            Some("https://abc123.my-site.pages.dev".to_string())
        );
        assert_eq!(
            extract_pages_url("https://my-site.pages.dev"),
            Some("https://my-site.pages.dev".to_string())
        );
        assert_eq!(extract_pages_url("no url here"), None);
        assert_eq!(extract_pages_url("https://example.com/foo"), None);
    }

    #[test]
    fn extract_pages_url_strips_ansi_and_rejects_lookalike_hosts() {
        assert_eq!(
            extract_pages_url("\u{1b}[36mhttps://abc.my-site.pages.dev\u{1b}[0m"),
            Some("https://abc.my-site.pages.dev".to_string())
        );
        assert_eq!(
            extract_pages_url("visit (https://abc.my-site.pages.dev) now"),
            Some("https://abc.my-site.pages.dev".to_string())
        );
        assert_eq!(
            extract_pages_url("https://evil.pages.dev.attacker.com/x"),
            None
        );
    }

    #[test]
    fn resolve_output_dir_rejects_escape_and_missing() {
        let base =
            std::env::temp_dir().join(format!("deploycode-pages-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("dist")).unwrap();

        assert!(resolve_output_dir(&base, "").is_err());
        assert!(resolve_output_dir(&base, ".").is_err());
        assert!(resolve_output_dir(&base, "../outside").is_err());
        assert!(resolve_output_dir(&base, "C:\\Windows").is_err());
        assert!(resolve_output_dir(&base, "missing").is_err());
        assert!(resolve_output_dir(&base, "dist").is_ok());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn validate_config_rejects_empty_project() {
        assert!(validate_config(&PagesConfig::default()).is_err());
        let mut config = PagesConfig::default();
        config.project_name = "my site".to_string();
        assert!(validate_config(&config).is_err());
        config.project_name = "my-site".to_string();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn prepare_requires_token_and_repo() {
        let dir = std::env::temp_dir().join(format!("deploycode-pages-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::new(&dir));
        let mut config = crate::models::AppConfig::default();
        let repo = crate::models::RepoConfig {
            id: "r1".to_string(),
            name: "app".to_string(),
            path: dir.display().to_string(),
            pages: Some(PagesConfig {
                project_name: "app".to_string(),
                output_dir: "dist".to_string(),
                ..PagesConfig::default()
            }),
            ..crate::models::RepoConfig::new("app".to_string(), dir.display().to_string())
        };
        config.repos.push(repo);
        store.save_config(&config).unwrap();
        let engine = PagesEngine::new(store.clone());
        let request = PagesRequest {
            repo_id: "r1".to_string(),
            project_name: None,
            build_command: None,
            output_dir: None,
            branch: None,
            skip_build: false,
        };
        // 未配置 Token 时要求先配置。
        assert!(matches!(engine.prepare(&request), Err(CoreError::Config(_))));

        store
            .mutate_config(|config| {
                config.settings.cloudflare_api_token = "t".to_string();
                config.settings.cloudflare_account_id = "a".to_string();
                Ok(())
            })
            .unwrap();
        assert!(engine.prepare(&request).is_ok());

        // 清理临时目录
        let _ = std::fs::remove_dir_all(&dir);
    }
}
