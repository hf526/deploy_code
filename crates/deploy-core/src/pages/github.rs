//! GitHub Pages 后端：本地构建后把产物推送到发布分支，由 GitHub 自动发布。
//!
//! 推送走 `npx --yes gh-pages`（带 `--dotfiles`），复用仓库本机 Git 凭据；
//! 发布分支的存在性由 GitHub REST 客户端（`github_api`）在推送成功后自动配置。

use std::path::Path;
use std::time::Duration;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{now_string, PagesConfig, PagesEvent};
use crate::process::run_stream;
use crate::ssh::OutputKind;
use crate::tasklog::TaskLogger;

use super::github_api::ensure_github_pages;
use super::sensitive::assert_output_has_no_secrets;
use super::{npx_program, resolve_output_dir, run_build, PagesEngine};

impl PagesEngine {
    /// GitHub Pages：本地构建后把产物推送到发布分支，由 GitHub 自动发布。
    pub(super) fn execute_github(
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
        // 产物里混进 .env / 私钥就拒绝发布：这两条路径都不过提交拦截，发出去等于直接公开。
        assert_output_has_no_secrets(&output_path)?;

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
}

/// 检查部署环境：GitHub 校验远端可访问 + 凭据可用。
pub(super) fn test_github(config: &PagesConfig, remote: &str, repo_path: &Path) -> Result<String> {
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

/// 从远端地址解析 GitHub 的 owner / repo，支持 https、git@ 与 ssh 形式。
pub(super) fn parse_github_remote(remote: &str) -> Option<(String, String)> {
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

/// git 身份（user.name / user.email）是否缺失，用于 gh-pages 提交兜底。
fn git_identity_missing(repo_path: &Path, key: &str) -> bool {
    let args = vec![
        "-C".to_string(),
        repo_path.to_string_lossy().into_owned(),
        "config".to_string(),
        "--get".to_string(),
        key.to_string(),
    ];
    crate::process::run("git", &args, None)
        .map(|out| !out.success() || out.stdout.trim().is_empty())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
