//! 原子发布：部署到 `{部署目录}/releases/<版本>`，成功后切换 `current` 软链。
//!
//! 失败时 `current` 仍指向旧版本，线上不受影响；历史版本可一键回滚。

use crate::error::{CoreError, Result};
use crate::models::{RemoteRelease, ServerConfig};
use crate::process::shell_quote;
use crate::ssh::SshClient;

/// 版本目录所在的子目录名。
pub const RELEASES_DIR: &str = "releases";
/// 指向当前生效版本的软链名。
pub const CURRENT_LINK: &str = "current";
/// 脚本成功执行后写入的完成标记：没有标记的版本目录视为未完成，不可切换 / 回滚。
pub const READY_MARKER: &str = ".deploy_code_ready";

/// 生成唯一的版本目录名：时间戳 + 提交短号 + 记录短号。
pub fn release_name(commit_short: &str, record_id: &str) -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let suffix: String = commit_short
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(12)
        .collect();
    let suffix = if suffix.is_empty() {
        "worktree".to_string()
    } else {
        suffix
    };
    let tail: String = record_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect();
    format!("{stamp}-{suffix}-{tail}")
}

/// 校验版本目录名：只允许字母 / 数字 / `-_.`，避免路径穿越与命令注入。
pub fn validate_release_name(name: &str) -> Result<String> {
    let name = name.trim();
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains("..")
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if invalid {
        return Err(CoreError::deploy(format!("版本目录名不合法: {name}")));
    }
    Ok(name.to_string())
}

/// 校验部署目录必须是服务器上的绝对路径。
pub fn normalize_target(target_dir: &str) -> Result<String> {
    let target = target_dir.trim().trim_end_matches('/');
    if target.is_empty() || !target.starts_with('/') {
        return Err(CoreError::deploy("部署目录必须是服务器上的绝对路径"));
    }
    Ok(target.to_string())
}

/// 切换 `current` 软链的远端命令。
///
/// 优先用临时软链 + `mv -Tf` 原子替换；`mv` 不支持 `-T`（如部分精简系统）时退回 `ln -sfn`。
pub fn switch_command(target_dir: &str, release: &str) -> String {
    let target = shell_quote(target_dir);
    let rel = shell_quote(&format!("{RELEASES_DIR}/{release}"));
    let current = shell_quote(CURRENT_LINK);
    format!(
        "cd {target} && ln -sfn {rel} .current.new && \
         ( mv -Tf .current.new {current} 2>/dev/null || {{ rm -f .current.new; ln -sfn {rel} {current}; }} )"
    )
}

/// 登录服务器列出版本目录（按名称倒序，最新在前）。
pub async fn list_releases(
    server: &ServerConfig,
    target_dir: &str,
    timeout_secs: u64,
) -> Result<Vec<RemoteRelease>> {
    let target = normalize_target(target_dir)?;
    let client = SshClient::connect(server, timeout_secs).await?;
    let target_q = shell_quote(&target);
    let dir_q = shell_quote(&format!("{target}/{RELEASES_DIR}"));
    let cmd = format!(
        "cd {dir_q} 2>/dev/null || exit 0; \
         cur=$(basename \"$(readlink {target_q}/{CURRENT_LINK} 2>/dev/null)\"); \
         for n in *; do [ -d \"$n\" ] || continue; \
           [ -f \"$n/{READY_MARKER}\" ] || continue; \
           m=$(stat -c %y \"$n\" 2>/dev/null | cut -d. -f1); \
           [ -n \"$m\" ] || m=$(stat -f %Sm -t '%Y-%m-%d %H:%M:%S' \"$n\" 2>/dev/null); \
           if [ \"$n\" = \"$cur\" ]; then c=1; else c=0; fi; \
           printf '%s\\t%s\\t%s\\n' \"$n\" \"$m\" \"$c\"; done"
    );
    let result = client.exec_capture(&cmd, timeout_secs.max(30)).await;
    client.disconnect().await;
    let (code, output) = result?;
    if code != 0 {
        return Err(CoreError::deploy(format!(
            "读取版本目录失败: {}",
            output.trim()
        )));
    }
    let mut releases: Vec<RemoteRelease> = output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next()?.trim();
            if name.is_empty() || name.starts_with("[stderr]") {
                return None;
            }
            let modified = parts.next().unwrap_or_default().trim().to_string();
            let current = parts.next().map(|value| value.trim() == "1").unwrap_or(false);
            Some(RemoteRelease {
                name: name.to_string(),
                current,
                modified,
            })
        })
        .collect();
    releases.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(releases)
}

/// 切换 `current` 软链到指定版本。
pub async fn switch_release(
    server: &ServerConfig,
    target_dir: &str,
    release: &str,
    timeout_secs: u64,
) -> Result<String> {
    let target = normalize_target(target_dir)?;
    let release = validate_release_name(release)?;
    let client = SshClient::connect(server, timeout_secs).await?;
    let rel_path = format!("{target}/{RELEASES_DIR}/{release}");
    let (exists, _) = client
        .exec_capture(
            &format!("test -f {}", shell_quote(&format!("{rel_path}/{READY_MARKER}"))),
            15,
        )
        .await?;
    if exists != 0 {
        client.disconnect().await;
        return Err(CoreError::deploy(format!(
            "版本不存在或尚未完成部署: releases/{release}"
        )));
    }
    let result = client
        .exec_capture(&switch_command(&target, &release), 30)
        .await;
    client.disconnect().await;
    let (code, output) = result?;
    if code != 0 {
        return Err(CoreError::deploy(format!(
            "切换版本失败: {}",
            output.trim()
        )));
    }
    Ok(format!("已切换 current -> releases/{release}"))
}

/// 生成清理命令：删除未完成（无标记）的版本，并只保留最新 `keep` 个已完成版本。
pub fn prune_command(target_dir: &str, keep: usize) -> String {
    let keep = keep.clamp(1, 50);
    let cut = keep + 1;
    let target = target_dir.trim().trim_end_matches('/');
    let target_q = shell_quote(target);
    let dir_q = shell_quote(&format!("{target}/{RELEASES_DIR}"));
    format!(
        "cd {dir_q} 2>/dev/null || exit 0; \
         cur=$(basename \"$(readlink {target_q}/{CURRENT_LINK} 2>/dev/null)\"); \
         for n in *; do [ -d \"$n\" ] || continue; [ \"$n\" = \"$cur\" ] && continue; \
           [ -f \"$n/{READY_MARKER}\" ] || {{ rm -rf -- \"$n\" && printf '%s\\n' \"$n\"; }}; done; \
         ls -1dt -- */ 2>/dev/null | tail -n +{cut} | while read -r d; do \
           n=${{d%/}}; [ \"$n\" = \"$cur\" ] && continue; \
           rm -rf -- \"$n\" && printf '%s\\n' \"$n\"; done"
    )
}

/// 清理旧版本：删除未完成（无标记）的版本，并只保留最新的 `keep` 个已完成版本
/// （`current` 指向的版本永不删除）。返回被删除的目录名。
pub async fn prune_releases(
    client: &SshClient,
    target_dir: &str,
    keep: usize,
) -> Result<Vec<String>> {
    let cmd = prune_command(target_dir, keep);
    let (code, output) = client.exec_capture(&cmd, 60).await?;
    if code != 0 {
        return Err(CoreError::deploy(format!(
            "清理旧版本失败: {}",
            output.trim()
        )));
    }
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("[stderr]"))
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_name_is_safe_and_unique_suffix() {
        let name = release_name("3b2424f", "a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        assert!(name.starts_with("20"), "name = {name}");
        assert!(name.contains("3b2424f"), "name = {name}");
        assert!(name.ends_with("a1b2c3"), "name = {name}");
        assert!(validate_release_name(&name).is_ok());

        let empty = release_name("", "abcdef01-2345");
        assert!(empty.ends_with("worktree-abcdef"), "name = {empty}");
    }

    #[test]
    fn validate_release_name_rejects_traversal_and_injection() {
        assert!(validate_release_name("20260914-153001-abc123").is_ok());
        assert!(validate_release_name("v1.2.3").is_ok());
        for bad in ["", ".", "..", "a/../b", "a/b", "a;rm -rf", "$(x)", "a b", "a\nb"] {
            assert!(validate_release_name(bad).is_err(), "应拒绝 {bad}");
        }
    }

    #[test]
    fn normalize_target_requires_absolute() {
        assert_eq!(normalize_target("/opt/app/").unwrap(), "/opt/app");
        assert_eq!(normalize_target("  /srv/app  ").unwrap(), "/srv/app");
        for bad in ["", "opt/app", "../app", "  "] {
            assert!(normalize_target(bad).is_err(), "应拒绝 {bad}");
        }
    }

    #[test]
    fn switch_command_quotes_paths_and_falls_back() {
        let cmd = switch_command("/opt/my app", "20260914-153001-abc");
        assert!(cmd.contains("'/opt/my app'"), "cmd = {cmd}");
        assert!(cmd.contains("releases/20260914-153001-abc"), "cmd = {cmd}");
        assert!(cmd.contains("mv -Tf"), "cmd = {cmd}");
        assert!(cmd.contains("ln -sfn"), "cmd = {cmd}");
    }

    #[test]
    fn prune_command_skips_unfinished_and_keeps_current() {
        let cmd = prune_command("/opt/my app/", 5);
        // 未完成（无标记）的版本会被删除。
        assert!(cmd.contains(READY_MARKER), "cmd = {cmd}");
        assert!(cmd.contains("'/opt/my app'"), "cmd = {cmd}");
        // keep = 5 时从第 6 个开始清理。
        assert!(cmd.contains("tail -n +6"), "cmd = {cmd}");
        // current 指向的版本永不删除（两个循环里都有保护）。
        assert_eq!(cmd.matches("[ \"$n\" = \"$cur\" ] && continue").count(), 2, "cmd = {cmd}");
        // keep 会被限幅到 1..=50。
        assert!(prune_command("/opt/app", 0).contains("tail -n +2"));
        assert!(prune_command("/opt/app", 999).contains("tail -n +51"));
    }
}
