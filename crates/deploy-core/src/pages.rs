//! Pages 一键部署：本地构建（可选）后发布静态产物。
//!
//! 支持两种平台：
//! - Cloudflare Pages：通过 `npx --yes wrangler` 上传，API Token / Account ID 走环境变量；
//! - GitHub Pages：通过 `npx --yes gh-pages` 把产物推送到发布分支（默认 gh-pages），
//!   复用仓库本机 Git 凭据，GitHub 会在推送后自动发布。
//!
//! 两者都依赖本机的 Node.js / npx。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{
    now_string, DeployStatus, PagesConfig, PagesDeployRecord, PagesEvent, PagesRequest,
};
use crate::process::{run, run_shell_stream, run_stream};
use crate::ssh::OutputKind;
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::format_duration;

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

        // 界面上的部署按钮在列表每一行：指定了 config_id 就用那一条，不能受「默认配置」指针
        // 影响（保存哪条就把哪条绑成默认，见 Store::save_pages_config），否则会发错项目。
        // 没指定时才回落到仓库默认；一条都没有才用平台默认值，再由请求参数覆盖。
        let requested = req
            .config_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut config = match requested {
            Some(id) => app_config
                .pages_configs
                .iter()
                .find(|entry| entry.id == id)
                .filter(|entry| entry.repo_id == repo.id)
                .ok_or_else(|| {
                    CoreError::not_found(format!("Pages 配置不存在，或不属于该仓库: {id}"))
                })?
                .config
                .clone(),
            None => match Store::get_repo_default_pages(&app_config, &repo.id) {
                Some(entry) => entry.config.clone(),
                None => PagesConfig::default(),
            },
        };
        
        // 允许请求参数覆盖
        if let Some(value) = non_empty(&req.provider) {
            config.provider = value.to_lowercase();
        }
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
        if let Some(value) = non_empty(&req.publish_branch) {
            config.publish_branch = value.to_string();
        }
        let config = config.normalize();

        let git = Git::open(&repo.path).ok();
        let remote = git
            .as_ref()
            .and_then(|git| git.any_remote_url().ok().flatten());
        validate_config(&config, remote.as_deref())?;

        let (project_name, token, account_id) = if config.is_github() {
            let (owner, name) = parse_github_remote(remote.as_deref().unwrap_or_default())
                .ok_or_else(|| CoreError::config("远端地址不是 GitHub 仓库（需 github.com）"))?;
            (format!("{owner}/{name}"), String::new(), String::new())
        } else {
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
            (config.project_name.clone(), token, account_id)
        };

        let (commit, commit_short) = match git.and_then(|git| git.resolve("HEAD").ok()) {
            Some(rev) => (rev.hash, rev.short),
            None => (String::new(), String::new()),
        };

        let record = PagesDeployRecord {
            id: uuid::Uuid::new_v4().to_string(),
            provider: config.provider.clone(),
            repo_id: repo.id.clone(),
            repo_name: repo.name.clone(),
            project_name,
            branch: config.display_branch().to_string(),
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
        let mut logger = TaskLogger::new(
            events,
            |level, message| PagesEvent::Log { level, message },
            None,
        );

        logger.send(PagesEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始部署 {} -> {}",
            record.repo_name,
            if config.is_github() {
                format!("GitHub Pages / {}", record.project_name)
            } else {
                format!("Cloudflare Pages / {}", record.project_name)
            }
        ));

        let result = if config.is_github() {
            self.execute_github(&config, &repo_path, skip_build, &mut logger)
        } else {
            self.execute(&config, &repo_path, &token, &account_id, skip_build, &mut logger)
        };

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

        record.log = logger.joined();
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        let limit = self
            .store
            .load_config()
            .map(|config| config.settings.pages_history_limit)
            .unwrap_or(200);
        if let Err(err) = self.store.upsert_pages_record(&record, limit) {
            logger.error(format!("保存 Pages 部署记录失败: {err}"));
            record.log = logger.joined();
        }

        if let Some(sender) = logger.take_events() {
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
        logger: &mut TaskLogger<PagesEvent>,
    ) -> Result<Option<String>> {
        let app_config = self.store.load_config()?;
        let timeout = Duration::from_secs(app_config.settings.script_timeout_secs.max(120));
        let deploy_timeout = Duration::from_secs(app_config.settings.script_timeout_secs.max(600));

        // 1. 构建
        run_build(config, repo_path, skip_build, timeout, logger)?;

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

    /// GitHub Pages：本地构建后把产物推送到发布分支，由 GitHub 自动发布。
    fn execute_github(
        &self,
        config: &PagesConfig,
        repo_path: &Path,
        skip_build: bool,
        logger: &mut TaskLogger<PagesEvent>,
    ) -> Result<Option<String>> {
        let app_config = self.store.load_config()?;
        let build_timeout = Duration::from_secs(app_config.settings.script_timeout_secs.max(120));
        let deploy_timeout = Duration::from_secs(app_config.settings.script_timeout_secs.max(600));

        run_build(config, repo_path, skip_build, build_timeout, logger)?;
        let output_path = resolve_output_dir(repo_path, &config.output_dir)?;

        let git = Git::open(repo_path)?;
        let remote = git
            .any_remote_url()?
            .ok_or_else(|| CoreError::config("请先为仓库绑定 GitHub 远端地址（origin）"))?;
        let (owner, name) = parse_github_remote(&remote)
            .ok_or_else(|| CoreError::config("远端地址不是 GitHub 仓库（需 github.com）"))?;
        let publish_branch = config.publish_branch.trim().to_string();
        if git.current_branch().ok().as_deref() == Some(publish_branch.as_str()) {
            return Err(CoreError::pages(format!(
                "发布分支 {publish_branch} 与当前所在分支相同，请改用独立分支（如 gh-pages）"
            )));
        }

        let output_arg = output_path.to_string_lossy().into_owned();
        logger.info(format!(
            "正在把 {output_arg} 推送到 GitHub 分支 {publish_branch} ..."
        ));
        let args = vec![
            "--yes".to_string(),
            "gh-pages".to_string(),
            "-d".to_string(),
            output_arg,
            "-b".to_string(),
            publish_branch.clone(),
            "--dotfiles".to_string(),
            "-m".to_string(),
            format!("DeployCode {}", now_string()),
        ];
        let mut deploy_log = |kind: OutputKind, line: String| match kind {
            OutputKind::Stdout => logger.info(line),
            OutputKind::Stderr => logger.warn(line),
        };
        // gh-pages 需要 git 身份才能创建提交；仓库与全局都没配置时给一个兜底身份。
        let mut envs: Vec<(String, String)> = Vec::new();
        if git_identity_missing(repo_path, "user.name") {
            envs.push(("GIT_AUTHOR_NAME".to_string(), "DeployCode".to_string()));
            envs.push(("GIT_COMMITTER_NAME".to_string(), "DeployCode".to_string()));
        }
        if git_identity_missing(repo_path, "user.email") {
            let email = "deploycode@localhost".to_string();
            envs.push(("GIT_AUTHOR_EMAIL".to_string(), email.clone()));
            envs.push(("GIT_COMMITTER_EMAIL".to_string(), email));
        }
        let code = run_stream(
            npx_program(),
            &args,
            Some(repo_path),
            &envs,
            deploy_timeout,
            &mut deploy_log,
        )?;
        if code != 0 {
            return Err(CoreError::pages(format!(
                "gh-pages 推送失败（退出码 {code}）"
            )));
        }

        // 推送成功、发布分支已创建后再启用 / 切换 GitHub Pages：
        // GitHub API 要求分支已存在，否则首次部署必然 422。
        ensure_github_pages(
            config,
            &owner,
            &name,
            app_config.settings.github_token.as_str(),
            logger,
        );

        let url = github_pages_url(&owner, &name);
        logger.success(format!("部署地址: {url}"));
        Ok(Some(url))
    }

    /// 检查部署环境：Cloudflare 校验 wrangler / Token，GitHub 校验远端可访问。
    /// `config` 为 Some 时使用表单里未保存的配置（与保存逻辑同样的规范化）。
    pub fn test(&self, repo_id: &str, config: Option<PagesConfig>) -> Result<String> {
        let app_config = self.store.load_config()?;
        let repo = Store::find_repo(&app_config, repo_id)?.clone();
        let config = match config {
            Some(config) => config.normalize(),
            None => {
                // 优先从 pages_configs 列表获取默认配置
                if let Some(entry) = Store::get_repo_default_pages(&app_config, repo_id) {
                    entry.config.clone()
                } else {
                    PagesConfig::default()
                }
            }
        }.normalize();
        let path = PathBuf::from(&repo.path);
        let remote = Git::open(path.as_path())
            .ok()
            .and_then(|git| git.any_remote_url().ok().flatten());
        validate_config(&config, remote.as_deref())?;

        if config.is_github() {
            return test_github(&config, remote.as_deref().unwrap_or_default(), &path);
        }

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

impl PagesConfig {
    /// 规范化表单配置：去空白并补默认值（保存与测试共用，GUI / CLI 都调用）。
    pub fn normalize(mut self) -> Self {
        self.provider = self.provider.trim().to_lowercase();
        if self.provider.is_empty() {
            self.provider = "cloudflare".to_string();
        }
        self.project_name = self.project_name.trim().to_string();
        self.build_command = self.build_command.trim().to_string();
        self.output_dir = self.output_dir.trim().to_string();
        self.branch = self.branch.trim().to_string();
        self.publish_branch = self.publish_branch.trim().to_string();
        if self.output_dir.is_empty() {
            self.output_dir = "dist".to_string();
        }
        if self.branch.is_empty() {
            self.branch = "main".to_string();
        }
        if self.publish_branch.is_empty() {
            self.publish_branch = "gh-pages".to_string();
        }
        self
    }

    fn is_github(&self) -> bool {
        self.provider == "github"
    }

    /// 记录里展示的分支：Cloudflare 用生产分支，GitHub 用发布分支。
    fn display_branch(&self) -> &str {
        if self.is_github() {
            &self.publish_branch
        } else {
            &self.branch
        }
    }
}

fn validate_config(config: &PagesConfig, remote: Option<&str>) -> Result<()> {
    if config.is_github() {
        validate_publish_branch(&config.publish_branch)?;
        let remote = remote
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoreError::config("GitHub Pages 需要先为仓库绑定 Git 地址"))?;
        if parse_github_remote(remote).is_none() {
            return Err(CoreError::config(
                "远端地址不是 GitHub 仓库（需 github.com），请先修改仓库的 Git 地址",
            ));
        }
        return Ok(());
    }
    if config.provider != "cloudflare" {
        return Err(CoreError::config(format!(
            "不支持的 Pages 平台: {}（可选 cloudflare / github）",
            config.provider
        )));
    }
    let project = config.project_name.trim();
    if project.is_empty() {
        return Err(CoreError::config("请先填写 Cloudflare Pages 项目名"));
    }
    if project.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CoreError::config("项目名不能包含空白字符"));
    }
    Ok(())
}

/// 校验 GitHub Pages 发布分支名，避免以 `-` 开头被 git/gh-pages 当作选项。
fn validate_publish_branch(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoreError::config("GitHub Pages 发布分支不能为空"));
    }
    if value.starts_with('-') || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CoreError::config(format!("非法的发布分支: {value}")));
    }
    Ok(())
}

/// 从远端地址解析 GitHub 的 owner / repo，支持 https、git@ 与 ssh 形式。
fn parse_github_remote(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim().trim_end_matches('/');
    // 必须解析主机名本身，不能用子串匹配：路径里出现 github.com 不代表是 GitHub 仓库。
    let path = if let Some((_, rest)) = remote.split_once("://") {
        let slash = rest.find('/')?;
        let authority = &rest[..slash];
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let host = host_port.split(':').next().unwrap_or(host_port);
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
            return None;
        }
        &rest[slash + 1..]
    } else {
        // scp 形式：[user@]github.com:owner/repo(.git)
        let (host_part, path) = remote.split_once(':')?;
        let host = host_part.rsplit('@').next().unwrap_or(host_part);
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.trim_end_matches(".git").to_string();
    // GitHub 的 owner / repo 只允许字母数字与 - _ .，同时排除 . 与 .. 防止路径穿越。
    let valid = |value: &str| {
        !value.is_empty()
            && value != "."
            && value != ".."
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !valid(&owner) || !valid(&repo) {
        return None;
    }
    Some((owner, repo))
}

/// 推导 GitHub Pages 访问地址（用户站点仓库 `<owner>.github.io` 不带子路径）。
fn github_pages_url(owner: &str, repo: &str) -> String {
    if repo.eq_ignore_ascii_case(&format!("{owner}.github.io")) {
        format!("https://{owner}.github.io/")
    } else {
        format!("https://{owner}.github.io/{repo}/")
    }
}

/// GitHub 环境检查：远端可访问 + 凭据可用。
fn test_github(config: &PagesConfig, remote: &str, repo_path: &Path) -> Result<String> {
    let (owner, name) = parse_github_remote(remote)
        .ok_or_else(|| CoreError::config("远端地址不是 GitHub 仓库（需 github.com）"))?;
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
        "-C".to_string(),
        repo_path.to_string_lossy().into_owned(),
        "ls-remote".to_string(),
        "--exit-code".to_string(),
        "origin".to_string(),
        "HEAD".to_string(),
    ];
    let code = run_stream(
        "git",
        &args,
        None,
        &[],
        Duration::from_secs(60),
        &mut collect,
    )?;
    if code != 0 {
        return Err(CoreError::pages(format!(
            "无法访问 GitHub 远端: {}",
            if output.trim().is_empty() {
                "请检查网络与仓库访问权限"
            } else {
                output.trim()
            }
        )));
    }
    Ok(format!(
        "GitHub 远端可访问：{owner}/{name}，发布分支 {}，预计地址 {}",
        config.publish_branch,
        github_pages_url(&owner, &name)
    ))
}

/// GitHub API 凭据来源。
struct GithubAuth {
    /// 本机 gh CLI 是否可用。
    gh: bool,
    /// 可用 Token：设置页优先，其次 `gh auth token`。
    token: Option<String>,
}

fn github_auth(settings_token: &str) -> GithubAuth {
    let gh = run("gh", &["--version".to_string()], None)
        .map(|out| out.success())
        .unwrap_or(false);
    let settings_token = settings_token.trim().to_string();
    let token = if !settings_token.is_empty() {
        Some(settings_token)
    } else if gh {
        run("gh", &["auth".to_string(), "token".to_string()], None)
            .ok()
            .filter(|out| out.success())
            .map(|out| out.stdout.trim().to_string())
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    GithubAuth { gh, token }
}

fn curl_available() -> bool {
    run("curl", &["--version".to_string()], None)
        .map(|out| out.success())
        .unwrap_or(false)
}

fn gh_token_env(auth: &GithubAuth) -> Vec<(String, String)> {
    match &auth.token {
        Some(token) => vec![("GH_TOKEN".to_string(), token.clone())],
        None => Vec::new(),
    }
}

/// 运行命令并分别捕获 stdout / stderr。
fn run_capture(
    program: &str,
    args: &[String],
    envs: &[(String, String)],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut collect = |kind: OutputKind, line: String| match kind {
        OutputKind::Stdout => {
            stdout.push_str(&line);
            stdout.push('\n');
        }
        OutputKind::Stderr => {
            stderr.push_str(&line);
            stderr.push('\n');
        }
    };
    let code = run_stream(program, args, None, envs, timeout, &mut collect)?;
    Ok((code, stdout, stderr))
}

enum GithubPagesState {
    Missing,
    Enabled { branch: String },
}

fn github_pages_state(
    owner: &str,
    repo_name: &str,
    auth: &GithubAuth,
) -> Result<GithubPagesState> {
    let path = format!("repos/{owner}/{repo_name}/pages");
    if auth.gh {
        let args = vec!["api".to_string(), path];
        let (code, stdout, stderr) = run_capture(
            "gh",
            &args,
            &gh_token_env(auth),
            Duration::from_secs(60),
        )?;
        if code == 0 {
            return Ok(GithubPagesState::Enabled {
                branch: parse_pages_source_branch(&stdout).unwrap_or_default(),
            });
        }
        let detail = format!("{stdout}\n{stderr}");
        if detail.contains("404") || detail.contains("Not Found") {
            return Ok(GithubPagesState::Missing);
        }
        return Err(CoreError::pages(format!(
            "gh api 查询失败: {}",
            detail.trim()
        )));
    }

    let token = auth.token.clone().unwrap_or_default();
    let args = vec![
        "-sS".to_string(),
        "-w".to_string(),
        "\n%{http_code}".to_string(),
        "-H".to_string(),
        format!("Authorization: Bearer {token}"),
        "-H".to_string(),
        "Accept: application/vnd.github+json".to_string(),
        "-H".to_string(),
        "User-Agent: DeployCode".to_string(),
        format!("https://api.github.com/{path}"),
    ];
    let (code, stdout, stderr) = run_capture("curl", &args, &[], Duration::from_secs(60))?;
    if code != 0 {
        return Err(CoreError::pages(format!("curl 调用失败: {}", stderr.trim())));
    }
    let (body, status) = split_curl_output(&stdout);
    match status.as_str() {
        "200" => Ok(GithubPagesState::Enabled {
            branch: parse_pages_source_branch(&body).unwrap_or_default(),
        }),
        "404" => Ok(GithubPagesState::Missing),
        other => Err(CoreError::pages(format!(
            "GitHub API 返回 {other}: {}",
            body.trim()
        ))),
    }
}

fn github_pages_set(
    owner: &str,
    repo_name: &str,
    branch: &str,
    auth: &GithubAuth,
    create: bool,
) -> Result<()> {
    let path = format!("repos/{owner}/{repo_name}/pages");
    let method = if create { "POST" } else { "PUT" };
    let body = serde_json::json!({ "source": { "branch": branch, "path": "/" } }).to_string();

    if auth.gh {
        let args = vec![
            "api".to_string(),
            "--method".to_string(),
            method.to_string(),
            path,
            "-f".to_string(),
            format!("source[branch]={branch}"),
            "-f".to_string(),
            "source[path]=/".to_string(),
        ];
        let (code, stdout, stderr) = run_capture(
            "gh",
            &args,
            &gh_token_env(auth),
            Duration::from_secs(60),
        )?;
        if code != 0 {
            return Err(CoreError::pages(format!(
                "gh api 写入失败: {}",
                format!("{stdout}\n{stderr}").trim()
            )));
        }
        return Ok(());
    }

    let token = auth.token.clone().unwrap_or_default();
    let args = vec![
        "-sS".to_string(),
        "-X".to_string(),
        method.to_string(),
        "-H".to_string(),
        format!("Authorization: Bearer {token}"),
        "-H".to_string(),
        "Accept: application/vnd.github+json".to_string(),
        "-H".to_string(),
        "User-Agent: DeployCode".to_string(),
        "-H".to_string(),
        "Content-Type: application/json".to_string(),
        "-d".to_string(),
        body,
        "-w".to_string(),
        "\n%{http_code}".to_string(),
        format!("https://api.github.com/{path}"),
    ];
    let (code, stdout, stderr) = run_capture("curl", &args, &[], Duration::from_secs(60))?;
    if code != 0 {
        return Err(CoreError::pages(format!("curl 调用失败: {}", stderr.trim())));
    }
    let (body, status) = split_curl_output(&stdout);
    if status == "200" || status == "201" || status == "204" {
        return Ok(());
    }
    Err(CoreError::pages(format!(
        "GitHub API 返回 {status}: {}",
        body.trim()
    )))
}

fn parse_pages_source_branch(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("source")
        .and_then(|source| source.get("branch"))
        .and_then(|branch| branch.as_str())
        .map(str::to_string)
}

fn split_curl_output(output: &str) -> (String, String) {
    let trimmed = output.trim_end();
    match trimmed.rfind('\n') {
        Some(index) => (
            trimmed[..index].to_string(),
            trimmed[index + 1..].trim().to_string(),
        ),
        None => (String::new(), trimmed.to_string()),
    }
}

/// 尽力自动配置 GitHub Pages：查询 / 启用 / 更新发布分支。
/// 失败只记录警告，不影响部署（用户仍可到仓库设置里手动选择）。
fn ensure_github_pages(
    config: &PagesConfig,
    owner: &str,
    repo_name: &str,
    settings_token: &str,
    logger: &mut TaskLogger<PagesEvent>,
) {
    let publish_branch = config.publish_branch.trim();
    let auth = github_auth(settings_token);
    if !auth.gh && (auth.token.is_none() || !curl_available()) {
        logger.warn(format!(
            "未检测到可用的 gh CLI / GitHub Token，跳过自动配置 GitHub Pages；\
             请到仓库 Settings → Pages 手动选择分支 {publish_branch}"
        ));
        return;
    }

    let state = match github_pages_state(owner, repo_name, &auth) {
        Ok(state) => state,
        Err(err) => {
            logger.warn(format!("读取 GitHub Pages 配置失败（跳过自动配置）: {err}"));
            return;
        }
    };

    match state {
        GithubPagesState::Enabled { branch } if branch == publish_branch => {
            logger.info(format!("GitHub Pages 已指向发布分支 {publish_branch}"));
        }
        GithubPagesState::Enabled { branch } => {
            logger.info(format!(
                "GitHub Pages 当前指向 {branch}，正在切换到 {publish_branch} ..."
            ));
            match github_pages_set(owner, repo_name, publish_branch, &auth, false) {
                Ok(()) => logger.success(format!("已把 GitHub Pages 切换到 {publish_branch}")),
                Err(err) => logger.warn(format!("切换 GitHub Pages 分支失败（可手动设置）: {err}")),
            }
        }
        GithubPagesState::Missing => {
            logger.info(format!(
                "GitHub Pages 尚未启用，正在启用并选择分支 {publish_branch} ..."
            ));
            match github_pages_set(owner, repo_name, publish_branch, &auth, true) {
                Ok(()) => logger.success(format!("已启用 GitHub Pages（分支 {publish_branch}）")),
                Err(err) => logger.warn(format!("自动启用 GitHub Pages 失败（可手动设置）: {err}")),
            }
        }
    }
}

/// 执行本地构建（两个平台共用）。
fn run_build(
    config: &PagesConfig,
    repo_path: &Path,
    skip_build: bool,
    timeout: Duration,
    logger: &mut TaskLogger<PagesEvent>,
) -> Result<()> {
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
    Ok(())
}

/// git 身份（user.name / user.email）是否缺失，用于 gh-pages 提交兜底。
fn git_identity_missing(repo_path: &Path, key: &str) -> bool {
    let args = vec![
        "-C".to_string(),
        repo_path.to_string_lossy().into_owned(),
        "config".to_string(),
        "--get".to_string(),
        key.to_string(),
    ];
    run("git", &args, None)
        .map(|out| !out.success() || out.stdout.trim().is_empty())
        .unwrap_or(true)
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
        assert!(validate_config(&PagesConfig::default(), None).is_err());
        let mut config = PagesConfig::default();
        config.project_name = "my site".to_string();
        assert!(validate_config(&config, None).is_err());
        config.project_name = "my-site".to_string();
        assert!(validate_config(&config, None).is_ok());
    }

    #[test]
    fn validate_config_github_requires_github_remote() {
        let github = PagesConfig {
            provider: "github".to_string(),
            project_name: String::new(),
            ..PagesConfig::default()
        };
        assert!(validate_config(&github, None).is_err());
        assert!(validate_config(&github, Some("git@gitee.com:user/repo.git")).is_err());
        assert!(validate_config(&github, Some("git@github.com:user/repo.git")).is_ok());
    }

    #[test]
    fn parse_github_remote_supports_common_forms() {
        assert_eq!(
            parse_github_remote("git@github.com:user/repo.git"),
            Some(("user".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/user/repo.git"),
            Some(("user".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/user/repo"),
            Some(("user".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com:22/user/repo.git"),
            Some(("user".to_string(), "repo".to_string()))
        );
        assert_eq!(parse_github_remote("git@gitee.com:user/repo.git"), None);
        assert_eq!(parse_github_remote("https://notgithub.com/user/repo"), None);
        // 主机名必须是 github.com，不能被子串或路径欺骗。
        assert_eq!(
            parse_github_remote("https://git.example.com/github.com/user/repo"),
            None
        );
        assert_eq!(parse_github_remote("https://mygithub.company/user/repo"), None);
        assert_eq!(
            parse_github_remote("https://www.github.com/user/repo.git"),
            Some(("user".to_string(), "repo".to_string()))
        );
        assert_eq!(parse_github_remote("https://github.com/user"), None);
        assert_eq!(parse_github_remote("https://github.com/../repo"), None);
        assert_eq!(parse_github_remote("https://github.com/user/re po"), None);
    }

    #[test]
    fn github_pages_url_handles_user_site() {
        assert_eq!(
            github_pages_url("user", "repo"),
            "https://user.github.io/repo/"
        );
        assert_eq!(
            github_pages_url("user", "user.github.io"),
            "https://user.github.io/"
        );
    }

    #[test]
    fn prepare_requires_token_and_repo() {
        let dir = std::env::temp_dir().join(format!("deploycode-pages-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::new(&dir));
        
        // 创建 PagesConfigEntry
        let pages_entry = crate::models::PagesConfigEntry {
            id: "p1".to_string(),
            name: "default".to_string(),
            repo_id: "r1".to_string(),
            repo_name: "app".to_string(),
            config: crate::models::PagesConfig {
                project_name: "app".to_string(),
                output_dir: "dist".to_string(),
                ..crate::models::PagesConfig::default()
            },
            created_at: crate::models::now_string(),
        };
        
        let config = crate::models::AppConfig {
            repos: vec![crate::models::RepoConfig {
                id: "r1".to_string(),
                name: "app".to_string(),
                path: dir.display().to_string(),
                default_pages_config_id: Some("p1".to_string()),
                ..crate::models::RepoConfig::new("app".to_string(), dir.display().to_string())
            }],
            pages_configs: vec![pages_entry],
            pages_configs_migrated: true, // 标记已迁移，避免重复迁移
            ..crate::models::AppConfig::default()
        };
        store.save_config(&config).unwrap();
        let engine = PagesEngine::new(store.clone());
        let request = PagesRequest {
            repo_id: "r1".to_string(),
            config_id: None,
            provider: None,
            project_name: None,
            build_command: None,
            output_dir: None,
            branch: None,
            publish_branch: None,
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

    #[test]
    fn prepare_uses_the_requested_config_not_the_default_pointer() {
        let dir = std::env::temp_dir().join(format!("deploycode-pages-row-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::new(&dir));
        let entry = |id: &str, project: &str| crate::models::PagesConfigEntry {
            id: id.to_string(),
            name: id.to_string(),
            repo_id: "r1".to_string(),
            repo_name: "app".to_string(),
            config: crate::models::PagesConfig {
                project_name: project.to_string(),
                output_dir: "dist".to_string(),
                ..crate::models::PagesConfig::default()
            },
            created_at: crate::models::now_string(),
        };
        store
            .mutate_config(|config| {
                config.repos.push(crate::models::RepoConfig {
                    id: "r1".to_string(),
                    // 默认指针停在 p1：点 p2 那行部署时不能被它带偏。
                    default_pages_config_id: Some("p1".to_string()),
                    ..crate::models::RepoConfig::new("app".to_string(), dir.display().to_string())
                });
                config.pages_configs = vec![entry("p1", "site-a"), entry("p2", "site-b")];
                config.settings.cloudflare_api_token = "t".to_string();
                config.settings.cloudflare_account_id = "a".to_string();
                Ok(())
            })
            .unwrap();
        let engine = PagesEngine::new(store.clone());
        let request = |config_id: Option<&str>| PagesRequest {
            repo_id: "r1".to_string(),
            config_id: config_id.map(str::to_string),
            provider: None,
            project_name: None,
            build_command: None,
            output_dir: None,
            branch: None,
            publish_branch: None,
            skip_build: false,
        };

        assert_eq!(
            engine.prepare(&request(Some("p2"))).unwrap().config.project_name,
            "site-b"
        );
        // 不带 config_id 时照旧按仓库默认解析（CLI 与旧数据走这条路）。
        assert_eq!(
            engine.prepare(&request(None)).unwrap().config.project_name,
            "site-a"
        );
        // 不存在的行要报错，不能悄悄退回默认配置发错项目。
        assert!(matches!(
            engine.prepare(&request(Some("p9"))),
            Err(CoreError::NotFound(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
