//! Pages 一键部署：本地构建（可选）后发布静态产物。
//!
//! 支持两种平台：
//! - Cloudflare Pages：通过 `npx --yes wrangler` 上传，API Token / Account ID 走环境变量
//!   （`cloudflare` 模块）；
//! - GitHub Pages：通过 `npx --yes gh-pages` 把产物推送到发布分支（默认 gh-pages），
//!   复用仓库本机 Git 凭据，GitHub 会在推送后自动发布（`github` 模块）。
//!
//! 两条路径发布前都过 `sensitive` 模块的产物敏感文件扫描；GitHub 的发布分支自动配置
//! 在 `github_api`（本机 `gh` CLI / `curl` 版 REST 客户端）。
//!
//! 两者都依赖本机的 Node.js / npx。

mod cloudflare;
mod github;
mod github_api;
mod sensitive;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{
    now_string, DeployStatus, PagesConfig, PagesDeployRecord, PagesEvent, PagesRequest,
};
use crate::process::{run_shell_stream, run_stream};
use crate::ssh::OutputKind;
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::format_duration;

use cloudflare::cf_envs;
use github::{parse_github_remote, test_github};

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

/// Pages 部署引擎：`prepare` 校验并落记录，`run` 按平台分发执行（阻塞）。
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

#[cfg(test)]
mod tests {
    use super::*;

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
