//! GitHub REST 客户端（无 HTTP 依赖，走本机 `gh` CLI 或 `curl`）与 Pages 自动配置。
//!
//! 查询 / 启用 / 切换发布分支都只是「尽力而为」：失败由调用方记警告继续部署，
//! 用户仍可到仓库设置里手动选择。

use std::time::Duration;

use crate::error::{CoreError, Result};
use crate::models::PagesEvent;
use crate::process::{run, run_stream};
use crate::ssh::OutputKind;
use crate::tasklog::TaskLogger;

use super::PagesConfig;

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
pub(super) fn ensure_github_pages(
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
