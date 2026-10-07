//! Cloudflare Pages 后端：通过 `npx --yes wrangler` 直传产物目录，
//! API Token / Account ID 走环境变量。

use std::path::Path;
use std::time::Duration;

use crate::error::{CoreError, Result};
use crate::models::{PagesConfig, PagesEvent};
use crate::process::run_stream;
use crate::ssh::OutputKind;
use crate::tasklog::TaskLogger;

use super::sensitive::assert_output_has_no_secrets;
use super::{npx_program, resolve_output_dir, run_build, PagesEngine};

impl PagesEngine {
    pub(super) fn execute(
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
        // 产物里混进 .env / 私钥就拒绝发布：这两条路径都不过提交拦截，发出去等于直接公开。
        assert_output_has_no_secrets(&output_path)?;

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
}

pub(super) fn cf_envs(token: &str, account_id: &str) -> Vec<(String, String)> {
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
}
