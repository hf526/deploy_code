//! 服务器安全检查：读取登录日志、ssh 配置与防火墙状态，并支持拉黑 / 解除 IP。

use std::collections::HashMap;

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::models::{now_string, ServerConfig};
use crate::process::shell_quote;
use crate::ssh::SshClient;

/// 某个账号 + 来源 IP 的失败登录统计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedLogin {
    pub user: String,
    pub ip: String,
    pub count: u32,
}

/// 一条成功登录记录。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginEvent {
    pub user: String,
    pub ip: String,
    pub detail: String,
}

/// 一个当前在线会话（来自 who）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineSession {
    pub user: String,
    pub tty: String,
    pub login_at: String,
    pub from: String,
}

/// 一项 sshd 配置。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySetting {
    pub key: String,
    pub value: String,
}

/// 服务器安全检查报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityReport {
    pub is_root: bool,
    pub has_sudo: bool,
    /// 检测到的防火墙：ufw / firewalld / iptables / none / unknown
    pub firewall: String,
    /// 防火墙当前的 DENY / DROP 来源 IP 列表。
    pub blocked: Vec<String>,
    /// 服务器端自动防护是否已启用。
    pub guard_enabled: bool,
    /// 自动防护的失败次数阈值。
    pub guard_threshold: u32,
    /// 自动防护的统计窗口（分钟）。
    pub guard_window_mins: u64,
    /// 失败登录（按账号 + IP 汇总，次数降序）。
    pub failed: Vec<FailedLogin>,
    /// 最近成功登录。
    pub success: Vec<LoginEvent>,
    /// 当前在线会话。
    pub sessions: Vec<OnlineSession>,
    /// 报告生成时间（本机时间）。
    pub scanned_at: String,
    /// sshd 关键配置。
    pub sshd: Vec<SecuritySetting>,
    /// 权限或数据缺失等提示。
    pub notes: Vec<String>,
}

/// 采集脚本：一次 SSH 执行，输出按 `###` 标记分段。
const SCAN_SCRIPT: &str = r#"SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
echo '###ROOT '$(id -u 2>/dev/null || echo '?')
if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then echo '###SUDO yes'; else echo '###SUDO no'; fi
if command -v ufw >/dev/null 2>&1 && $SUDO ufw status 2>/dev/null | grep -qi 'Status: active'; then
  echo '###FW ufw'
  $SUDO ufw status 2>/dev/null | grep -Ei 'DENY|REJECT' || true
elif command -v firewall-cmd >/dev/null 2>&1; then
  echo '###FW firewalld'
  $SUDO firewall-cmd --list-rich-rules 2>/dev/null | grep -i drop || true
elif command -v iptables >/dev/null 2>&1; then
  echo '###FW iptables'
  $SUDO iptables -S INPUT 2>/dev/null | grep -Ei 'DROP|REJECT' || true
else
  echo '###FW none'
fi
echo '###F2B'
if command -v fail2ban-client >/dev/null 2>&1; then
  $SUDO fail2ban-client status sshd 2>/dev/null | grep -i 'Banned IP' || true
fi
echo '###SSHD'
SSHD_BIN="$(command -v sshd 2>/dev/null || echo /usr/sbin/sshd)"
if [ -x "$SSHD_BIN" ]; then
  $SUDO "$SSHD_BIN" -T 2>/dev/null | grep -Ei '^(port|permitrootlogin|passwordauthentication|pubkeyauthentication|maxauthtries) ' || true
fi
echo '###LASTB'
if command -v lastb >/dev/null 2>&1; then $SUDO lastb -i -n 80 2>/dev/null | head -n 80 || true; fi
echo '###AUTHLOG'
grep -hEi 'Failed password|Invalid user' /var/log/auth.log /var/log/auth.log.1 /var/log/secure /var/log/secure-* 2>/dev/null | tail -n 150 || true
echo '###JOURNAL'
journalctl -u sshd -u ssh --no-pager -n 500 2>/dev/null | grep -Ei 'Failed password|Invalid user' | tail -n 150 || true
echo '###LAST'
last -i -n 25 2>/dev/null | head -n 25 || true
echo '###SESSIONS'
who 2>/dev/null | head -n 50 || true
echo '###GUARD'
grep -E '^(THRESHOLD|WINDOW)=' /etc/deploycode-guard.conf 2>/dev/null || true
if [ -f /etc/cron.d/deploycode-guard ] && grep -q deploycode-guard /etc/cron.d/deploycode-guard 2>/dev/null; then echo 'CRON yes'; else echo 'CRON no'; fi
echo '###DONE'"#;

/// 连接服务器并采集安全检查报告。
pub async fn collect(server: &ServerConfig, connect_timeout_secs: u64) -> Result<SecurityReport> {
    let client = SshClient::connect(server, connect_timeout_secs).await?;
    let result = client.exec_capture(SCAN_SCRIPT, 60).await;
    client.disconnect().await;
    let (_, output) = result?;
    Ok(parse_report(&output))
}

/// 拉黑一个 IP（自动选择 ufw / firewalld / iptables）。
pub async fn block_ip(server: &ServerConfig, connect_timeout_secs: u64, ip: &str) -> Result<String> {
    validate_ip(ip)?;
    run_rule_command(server, connect_timeout_secs, &rule_script(ip, true), "拉黑").await
}

/// 解除一个 IP 的拉黑。
pub async fn unblock_ip(
    server: &ServerConfig,
    connect_timeout_secs: u64,
    ip: &str,
) -> Result<String> {
    validate_ip(ip)?;
    run_rule_command(server, connect_timeout_secs, &rule_script(ip, false), "解除").await
}

async fn run_rule_command(
    server: &ServerConfig,
    connect_timeout_secs: u64,
    command: &str,
    action: &str,
) -> Result<String> {
    let client = SshClient::connect(server, connect_timeout_secs).await?;
    let result = client.exec_capture(command, 60).await;
    client.disconnect().await;
    let (code, output) = result?;
    let clean = clean_output(&output);
    if code != 0 {
        return Err(CoreError::ssh(format!(
            "{action}失败: {}",
            if clean.is_empty() { "权限不足或命令执行失败" } else { clean.as_str() }
        )));
    }
    Ok(clean)
}

/// 服务器端守护脚本：由 cron 每分钟执行，统计窗口内失败登录并按阈值拉黑。
const GUARD_SCRIPT: &str = r#"#!/bin/sh
# DeployCode server-side guard (installed by the DeployCode app).
[ -f /etc/deploycode-guard.conf ] && . /etc/deploycode-guard.conf
THRESHOLD=${THRESHOLD:-5}
WINDOW=${WINDOW:-10}
LOG=/var/log/deploycode-guard.log
[ "$(id -u)" != "0" ] && exit 0

failed() {
  if command -v journalctl >/dev/null 2>&1; then
    out=$(journalctl -u sshd -u ssh --since=-${WINDOW}min --no-pager 2>/dev/null | grep -Ei 'Failed password|Invalid user')
    if [ -n "$out" ]; then
      printf '%s\n' "$out"
      return 0
    fi
  fi
  # 回退：按时间窗过滤日志文件（兼容无 journalctl 或 unit 名不同的系统）。
  for f in /var/log/auth.log /var/log/secure; do
    [ -f "$f" ] || continue
    awk -v window="${WINDOW}" '
      BEGIN {
        split("Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec", names, " ")
        for (i = 1; i <= 12; i++) mon[names[i]] = i
        "date +%m" | getline month; close("date +%m")
        "date +%d" | getline day; close("date +%d")
        "date +%H" | getline hh; close("date +%H")
        "date +%M" | getline mm; close("date +%M")
        now = ((month + 0) * 31 + (day + 0)) * 1440 + (hh + 0) * 60 + (mm + 0)
      }
      {
        m = mon[$1]
        if (!m) next
        t = (m * 31 + ($2 + 0)) * 1440 + substr($3, 1, 2) * 60 + substr($3, 4, 2)
        diff = now - t
        if (diff >= 0 && diff <= window) print
      }
    ' "$f"
  done 2>/dev/null | grep -Ei 'Failed password|Invalid user' | tail -n 5000
}

blocked_ips() {
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi 'Status: active'; then
    ufw status 2>/dev/null | grep -i deny | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}'
  elif command -v firewall-cmd >/dev/null 2>&1; then
    firewall-cmd --list-rich-rules 2>/dev/null | grep -oE 'address="[^"]+"' | cut -d'"' -f2
  else
    iptables -S INPUT 2>/dev/null | grep -Ei 'DROP|REJECT' | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}'
  fi
}

block() {
  ip=$1
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi 'Status: active'; then
    ufw deny from $ip >/dev/null 2>&1
  elif command -v firewall-cmd >/dev/null 2>&1; then
    firewall-cmd --permanent --add-rich-rule="rule family=ipv4 source address=$ip drop" >/dev/null 2>&1
    firewall-cmd --reload >/dev/null 2>&1
  else
    iptables -I INPUT -s $ip -j DROP >/dev/null 2>&1
  fi
}

BLOCKED=$(blocked_ips)
failed | grep -oE 'from [0-9a-fA-F:.]+' | awk '{print $2}' | sort | uniq -c | while read count ip; do
  [ "${count}" -ge "${THRESHOLD}" ] 2>/dev/null || continue
  echo "$BLOCKED" | grep -qxF "$ip" && continue
  block "$ip"
  echo "$(date '+%F %T') blocked $ip after ${count} failed logins" >> "$LOG"
done
"#;

/// 启用服务器端自动防护：写入守护脚本 + 配置 + 每分钟 cron 任务。
pub async fn enable_guard(
    server: &ServerConfig,
    connect_timeout_secs: u64,
    threshold: u32,
    window_mins: u64,
) -> Result<String> {
    let threshold = threshold.clamp(1, 100);
    let window_mins = window_mins.clamp(1, 1440);
    let command = format!(
        r#"SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
$SUDO tee /usr/local/bin/deploycode-guard >/dev/null <<'GUARD_EOF' || exit 1
{GUARD_SCRIPT}
GUARD_EOF
$SUDO tee /etc/deploycode-guard.conf >/dev/null <<'CONF_EOF' || exit 1
THRESHOLD={threshold}
WINDOW={window_mins}
CONF_EOF
$SUDO tee /etc/cron.d/deploycode-guard >/dev/null <<'CRON_EOF' || exit 1
* * * * * root /usr/local/bin/deploycode-guard >/dev/null 2>&1
CRON_EOF
$SUDO chmod 755 /usr/local/bin/deploycode-guard || exit 1
$SUDO chmod 644 /etc/deploycode-guard.conf /etc/cron.d/deploycode-guard || exit 1
[ -x /usr/local/bin/deploycode-guard ] && [ -f /etc/cron.d/deploycode-guard ] || exit 1
$SUDO /usr/local/bin/deploycode-guard >/dev/null 2>&1 || true
echo '自动防护已启用'
"#
    );
    run_rule_command(server, connect_timeout_secs, &command, "启用自动防护").await
}

/// 停用服务器端自动防护（移除 cron 任务，保留脚本与配置便于再次启用）。
pub async fn disable_guard(server: &ServerConfig, connect_timeout_secs: u64) -> Result<String> {
    let command = r#"SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
$SUDO rm -f /etc/cron.d/deploycode-guard && echo '自动防护已停用'
"#;
    run_rule_command(server, connect_timeout_secs, command, "停用自动防护").await
}

fn rule_script(ip: &str, block: bool) -> String {
    let quoted = shell_quote(ip);
    let family = if ip.contains(':') { "ipv6" } else { "ipv4" };
    if block {
        format!(
            r#"SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
IP={quoted}
done=0
if command -v ufw >/dev/null 2>&1 && $SUDO ufw status 2>/dev/null | grep -qi 'Status: active'; then
  $SUDO ufw deny from "$IP" >/dev/null 2>&1 && done=1
fi
if [ "$done" = "0" ] && command -v firewall-cmd >/dev/null 2>&1; then
  $SUDO firewall-cmd --permanent --add-rich-rule='rule family="{family}" source address="'"$IP"'" drop' >/dev/null 2>&1 && $SUDO firewall-cmd --reload >/dev/null 2>&1 && done=1
fi
if [ "$done" = "0" ]; then
  BIN=iptables; [ "{family}" = "ipv6" ] && BIN=ip6tables
  command -v "$BIN" >/dev/null 2>&1 && $SUDO $BIN -I INPUT -s "$IP" -j DROP >/dev/null 2>&1 && done=1
fi
if [ "$done" = "1" ]; then echo "已拉黑 $IP"; exit 0; fi
echo "拉黑失败：没有可用的防火墙或权限不足" >&2
exit 1
"#
        )
    } else {
        format!(
            r#"SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
IP={quoted}
if command -v fail2ban-client >/dev/null 2>&1; then
  $SUDO fail2ban-client unban "$IP" >/dev/null 2>&1
fi
if command -v ufw >/dev/null 2>&1; then
  $SUDO ufw delete deny from "$IP" >/dev/null 2>&1
fi
if command -v firewall-cmd >/dev/null 2>&1; then
  $SUDO firewall-cmd --permanent --remove-rich-rule='rule family="{family}" source address="'"$IP"'" drop' >/dev/null 2>&1
  $SUDO firewall-cmd --reload >/dev/null 2>&1
fi
BIN=iptables; [ "{family}" = "ipv6" ] && BIN=ip6tables
if command -v "$BIN" >/dev/null 2>&1; then
  $SUDO $BIN -D INPUT -s "$IP" -j DROP >/dev/null 2>&1
fi
# 复核：任一来源仍存在则视为失败（fail2ban unban 对未封禁 IP 也会返回成功）。
still=0
if command -v fail2ban-client >/dev/null 2>&1; then
  $SUDO fail2ban-client banned 2>/dev/null | grep -qwF "$IP" && still=1
fi
if command -v ufw >/dev/null 2>&1; then
  $SUDO ufw status 2>/dev/null | grep -qwF "$IP" && still=1
fi
if command -v firewall-cmd >/dev/null 2>&1; then
  $SUDO firewall-cmd --list-rich-rules 2>/dev/null | grep -qwF "$IP" && still=1
fi
if command -v "$BIN" >/dev/null 2>&1; then
  $SUDO $BIN -S INPUT 2>/dev/null | grep -qwF "$IP" && still=1
fi
if [ "$still" = "0" ]; then echo "已解除 $IP"; exit 0; fi
echo "解除失败：IP 仍在拦截列表中（权限不足或规则来源未知）" >&2
exit 1
"#
        )
    }
}

fn validate_ip(ip: &str) -> Result<()> {
    let ip = ip.trim();
    if is_ip(ip) {
        Ok(())
    } else {
        Err(CoreError::config(format!("无效的 IP 地址: {ip}")))
    }
}

fn clean_output(output: &str) -> String {
    // 保留 stderr 内容（去掉标记前缀），否则脚本的真实失败原因会被吞掉。
    output
        .lines()
        .map(|line| {
            let line = line.trim();
            line.strip_prefix("[stderr] ").unwrap_or(line).trim()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_report(output: &str) -> SecurityReport {
    let mut section = "";
    let mut is_root = false;
    let mut has_sudo = false;
    let mut firewall = "unknown".to_string();
    let mut blocked: Vec<String> = Vec::new();
    let mut sshd: Vec<SecuritySetting> = Vec::new();
    let mut failed_map: HashMap<(String, String), u32> = HashMap::new();
    let mut success: Vec<LoginEvent> = Vec::new();
    let mut sessions: Vec<OnlineSession> = Vec::new();
    let mut guard_conf = false;
    let mut guard_cron = false;
    let mut guard_threshold: u32 = 5;
    let mut guard_window_mins: u64 = 10;

    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("[stderr]") {
            continue;
        }
        if let Some(marker) = line.strip_prefix("###") {
            let mut parts = marker.splitn(2, ' ');
            let name = parts.next().unwrap_or("").trim();
            let value = parts.next().unwrap_or("").trim();
            match name {
                "ROOT" => is_root = value == "0",
                "SUDO" => has_sudo = value == "yes",
                "FW" => firewall = value.to_string(),
                _ => {}
            }
            section = match name {
                "F2B" => "f2b",
                "SSHD" => "sshd",
                "LASTB" => "lastb",
                "AUTHLOG" => "authlog",
                "JOURNAL" => "journal",
                "LAST" => "last",
                "SESSIONS" => "sessions",
                "GUARD" => "guard",
                _ => "",
            };
            continue;
        }

        match section {
            "f2b" => {
                if let Some(idx) = line.to_ascii_lowercase().find("banned ip list") {
                    for token in line[idx..].split([':', ' ', ',', '\t']) {
                        let token = token.trim();
                        if is_ip(token) && !blocked.iter().any(|b| b == token) {
                            blocked.push(token.to_string());
                        }
                    }
                }
            }
            "sshd" => {
                let mut parts = line.splitn(2, char::is_whitespace);
                let key = parts.next().unwrap_or("").trim();
                let value = parts.next().unwrap_or("").trim();
                if !key.is_empty() && !value.is_empty() {
                    sshd.push(SecuritySetting {
                        key: key.to_string(),
                        value: value.to_string(),
                    });
                }
            }
            "lastb" => {
                if line.contains("begins") || line.starts_with("btmp") {
                    continue;
                }
                if let Some(ip) = find_ip(line) {
                    let user = line
                        .split_whitespace()
                        .next()
                        .unwrap_or("-")
                        .trim_end_matches('*')
                        .to_string();
                    *failed_map.entry((user, ip)).or_insert(0) += 1;
                }
            }
            "authlog" | "journal" => {
                if let Some((user, ip)) = parse_failed_login(line) {
                    *failed_map.entry((user, ip)).or_insert(0) += 1;
                }
            }
            "last" => {
                if line.contains("begins") || line.starts_with("wtmp") {
                    continue;
                }
                if let Some(ip) = find_ip(line) {
                    let user = line
                        .split_whitespace()
                        .next()
                        .unwrap_or("-")
                        .trim_end_matches('*')
                        .to_string();
                    let detail = line
                        .split_once(&ip)
                        .map(|(_, rest)| rest.trim().to_string())
                        .unwrap_or_default();
                    if success.len() < 20 {
                        success.push(LoginEvent { user, ip, detail });
                    }
                }
            }
            "sessions" => {
                if let Some(session) = parse_who_line(line) {
                    if sessions.len() < 50 {
                        sessions.push(session);
                    }
                }
            }
            "guard" => {
                if let Some(value) = line.strip_prefix("THRESHOLD=") {
                    guard_conf = true;
                    if let Ok(value) = value.trim().parse::<u32>() {
                        guard_threshold = value.max(1);
                    }
                } else if let Some(value) = line.strip_prefix("WINDOW=") {
                    guard_conf = true;
                    if let Ok(value) = value.trim().parse::<u64>() {
                        guard_window_mins = value.max(1);
                    }
                } else if line.eq_ignore_ascii_case("CRON yes") {
                    guard_cron = true;
                }
            }
            _ => {
                // 防火墙规则行：提取来源 IP（iptables -S / firewalld rich rule 也在此节）
                if matches!(firewall.as_str(), "ufw" | "firewalld" | "iptables") {
                    if let Some(ip) = find_ip(line) {
                        if !blocked.iter().any(|b| b == &ip) {
                            blocked.push(ip);
                        }
                    }
                }
            }
        }
    }

    // 防火墙节出现在 ###FW 之后、###F2B 之前，上面的默认分支即可覆盖。
    let mut failed: Vec<FailedLogin> = failed_map
        .into_iter()
        .map(|((user, ip), count)| FailedLogin { user, ip, count })
        .collect();
    failed.sort_by(|a, b| b.count.cmp(&a.count));
    failed.truncate(40);

    let mut notes = Vec::new();
    if !is_root && !has_sudo {
        notes.push(
            "当前账号没有 root / 免密 sudo 权限：日志与防火墙信息可能不完整，拉黑操作可能失败".to_string(),
        );
    }
    if failed.is_empty() {
        notes.push("未读取到失败登录记录（可能缺少日志权限，或系统未记录）".to_string());
    }

    SecurityReport {
        is_root,
        has_sudo,
        firewall,
        blocked,
        guard_enabled: guard_conf && guard_cron,
        guard_threshold,
        guard_window_mins,
        failed,
        success,
        sessions,
        scanned_at: now_string(),
        sshd,
        notes,
    }
}

/// 解析 `who` 输出行：`user tty login-time (from)`。
fn parse_who_line(line: &str) -> Option<OnlineSession> {
    let mut parts = line.split_whitespace();
    let user = parts.next()?.to_string();
    let tty = parts.next()?.to_string();
    let rest = parts.collect::<Vec<_>>().join(" ");
    if user.is_empty() || tty.is_empty() || rest.is_empty() {
        return None;
    }
    let (login_at, from) = match rest.rfind('(') {
        Some(idx) => (
            rest[..idx].trim().to_string(),
            rest[idx..]
                .trim_matches(|c| c == '(' || c == ')')
                .trim()
                .to_string(),
        ),
        None => (rest.trim().to_string(), String::new()),
    };
    Some(OnlineSession {
        user,
        tty,
        login_at,
        from,
    })
}

/// 校验会话终端名，避免命令拼接注入。
fn validate_tty(tty: &str) -> Result<()> {
    let valid = !tty.is_empty()
        && tty.len() <= 64
        && tty
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':'));
    if valid {
        Ok(())
    } else {
        Err(CoreError::config(format!("无效的会话终端: {tty}")))
    }
}

/// 强制踢出某个在线会话（按 tty）。
pub async fn kick_session(
    server: &ServerConfig,
    connect_timeout_secs: u64,
    tty: &str,
) -> Result<String> {
    validate_tty(tty)?;
    let client = SshClient::connect(server, connect_timeout_secs).await?;
    let command = format!("pkill -KILL -t {} 2>/dev/null", shell_quote(tty));
    let result = client.exec_capture(&command, 30).await;
    client.disconnect().await;
    let (code, _output) = result?;
    if code != 0 {
        return Err(CoreError::ssh(format!(
            "踢出会话失败（会话可能已结束或权限不足）: {tty}"
        )));
    }
    Ok(format!("已踢出会话 {tty}"))
}

fn parse_failed_login(line: &str) -> Option<(String, String)> {
    let lower = line.to_ascii_lowercase();
    if let Some(idx) = lower.find("failed password for ") {
        let rest = &line[idx + "failed password for ".len()..];
        let rest = strip_invalid_user(rest);
        let (user, tail) = split_at_from(rest);
        let ip = find_ip(tail).or_else(|| find_ip(line))?;
        return Some((user, ip));
    }
    if let Some(idx) = lower.find("invalid user ") {
        let rest = &line[idx + "invalid user ".len()..];
        let (user, tail) = split_at_from(rest);
        let ip = find_ip(tail).or_else(|| find_ip(line))?;
        return Some((user, ip));
    }
    None
}

/// 去掉 "invalid user " 前缀，避免把 "invalid user admin" 误当成账号名。
fn strip_invalid_user(rest: &str) -> &str {
    let lower = rest.to_ascii_lowercase();
    if lower.starts_with("invalid user ") {
        &rest["invalid user ".len()..]
    } else {
        rest
    }
}

/// 从 "root from 1.2.3.4 port 22 ssh2" 中拆出账号与余下部分。
fn split_at_from(rest: &str) -> (String, &str) {
    let lower = rest.to_ascii_lowercase();
    if let Some(pos) = lower.find(" from ") {
        (rest[..pos].trim().to_string(), &rest[pos + 6..])
    } else {
        (rest.trim().to_string(), rest)
    }
}

/// 在行内查找第一个 IP 形式的 token（兼容 ufw / iptables / firewalld / 日志格式）。
fn find_ip(line: &str) -> Option<String> {
    for token in line.split(|c: char| c.is_whitespace() || c == ',' || c == '\'' || c == '"') {
        let token = token.split('/').next().unwrap_or(token);
        let token = token.trim_matches(|c: char| {
            !(c.is_ascii_hexdigit() || c == '.' || c == ':')
        });
        if is_ip(token) {
            return Some(token.to_string());
        }
    }
    None
}

fn is_ip(token: &str) -> bool {
    // 兼容日志里的 [IPv6] 写法。
    let token = token.trim_matches(|c| c == '[' || c == ']');
    if token.is_empty() || token.len() > 45 {
        return false;
    }
    // 仅接受链路本地 IPv6 的 zone id（fe80::/10），并校验 zone 字符集。
    let token = match token.split_once('%') {
        Some((addr, zone)) => {
            let zone_ok = !zone.is_empty()
                && zone.len() <= 32
                && zone
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
            match addr.parse::<std::net::Ipv6Addr>() {
                Ok(parsed) if zone_ok && (parsed.segments()[0] & 0xffc0) == 0xfe80 => addr,
                _ => return false,
            }
        }
        None => token,
    };
    // 使用标准库解析，拒绝 "::::"、前导零等非法写法；同时排除 0.0.0.0 / ::（防火墙规则里的“任意地址”）。
    match token.parse::<std::net::IpAddr>() {
        Ok(addr) => !addr.is_unspecified(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ip_accepts_valid_forms() {
        assert!(is_ip("1.2.3.4"));
        assert!(is_ip("2001:db8::1"));
        assert!(is_ip("[2001:db8::1]"));
        assert!(is_ip("fe80::1%eth0"));
    }

    #[test]
    fn is_ip_rejects_invalid_and_unspecified() {
        assert!(!is_ip(""));
        assert!(!is_ip("0.0.0.0"));
        assert!(!is_ip("::"));
        assert!(!is_ip(":::::"));
        assert!(!is_ip("1:2"));
        assert!(!is_ip("999.1.1.1"));
        assert!(!is_ip("01.2.3.4"));
        assert!(!is_ip("1.2.3"));
    }

    #[test]
    fn find_ip_skips_unspecified_addresses() {
        assert_eq!(find_ip("ufw deny from 0.0.0.0"), None);
        assert_eq!(
            find_ip("Failed password for root from 10.0.0.8 port 22"),
            Some("10.0.0.8".to_string())
        );
    }

    #[test]
    fn is_ip_zone_id_restrictions() {
        assert!(is_ip("fe80::1%eth0"));
        assert!(!is_ip("1.2.3.4%eth0"));
        assert!(!is_ip("2001:db8::1%eth0"));
        assert!(!is_ip("fe80::1%"));
        assert!(!is_ip("fe80::1%bad zone"));
    }

    #[test]
    fn parse_failed_login_strips_invalid_user_prefix() {
        let line = "Sep 12 10:00 host sshd[1]: Failed password for invalid user admin from 1.2.3.4 port 22 ssh2";
        let (user, ip) = parse_failed_login(line).unwrap();
        assert_eq!(user, "admin");
        assert_eq!(ip, "1.2.3.4");
    }

    #[test]
    fn clean_output_keeps_stderr_text() {
        let text = clean_output("[stderr] 解除失败：没有可用的防火墙\n已解除 1.2.3.4\n");
        assert!(text.contains("解除失败：没有可用的防火墙"));
        assert!(text.contains("已解除 1.2.3.4"));
    }

    #[test]
    fn parse_who_line_extracts_session_fields() {
        let session = parse_who_line("root     pts/0        2026-09-12 10:20 (1.2.3.4)").unwrap();
        assert_eq!(session.user, "root");
        assert_eq!(session.tty, "pts/0");
        assert_eq!(session.login_at, "2026-09-12 10:20");
        assert_eq!(session.from, "1.2.3.4");

        let local = parse_who_line("admin    tty1         2026-09-12 09:00").unwrap();
        assert_eq!(local.from, "");
    }

    #[test]
    fn validate_tty_rejects_injection() {
        assert!(validate_tty("pts/0").is_ok());
        assert!(validate_tty("tty1").is_ok());
        assert!(validate_tty("pts/0; rm -rf /").is_err());
        assert!(validate_tty("").is_err());
        assert!(validate_tty("pts/0 $(id)").is_err());
    }
}
