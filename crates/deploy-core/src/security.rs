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
    /// 免封白名单（服务器 `ALLOW=` 配置里的条目，单个 IP）。
    pub whitelist: Vec<String>,
    /// 本次扫描这条 SSH 连接的来源 IP（未知时为空）。
    pub self_ip: String,
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
const SCAN_SCRIPT: &str = r#"SELF="${SSH_CLIENT%% *}"
case "$SELF" in *[!0-9a-fA-F:.]*) SELF="" ;; esac
echo '###SELF '"$SELF"
SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
echo '###ROOT '$(id -u 2>/dev/null || echo '?')
if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then echo '###SUDO yes'; else echo '###SUDO no'; fi
if command -v ufw >/dev/null 2>&1 && $SUDO ufw status 2>/dev/null | grep -qi 'Status: active'; then
  echo '###FW ufw'
  $SUDO ufw status 2>/dev/null | grep -Ei 'DENY|REJECT' || true
elif command -v firewall-cmd >/dev/null 2>&1 && $SUDO firewall-cmd --state 2>/dev/null | grep -qx running; then
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
grep -E '^(THRESHOLD|WINDOW|ALLOW)=' /etc/deploycode-guard.conf 2>/dev/null || true
if [ -f /etc/cron.d/deploycode-guard ] && grep -q deploycode-guard /etc/cron.d/deploycode-guard 2>/dev/null; then echo 'CRON yes'; else echo 'CRON no'; fi
echo '###DONE'"#;

/// 连接服务器并采集安全检查报告。
pub async fn collect(server: &ServerConfig, connect_timeout_secs: u64) -> Result<SecurityReport> {
    let client = SshClient::connect(server, connect_timeout_secs).await?;
    let result = client.exec_capture(SCAN_SCRIPT, 60).await;
    client.disconnect().await;
    let (code, output) = result?;
    // 脚本正常结束时最后一条 echo 返回 0；非 0（含连接中断的 -1）说明中途失败，
    // 避免把残缺报告当成功展示。
    if code != 0 {
        let clean = clean_output(&output);
        return Err(CoreError::ssh(format!(
            "安全扫描脚本执行失败（退出码 {code}）：{}",
            if clean.is_empty() { "无输出" } else { clean.as_str() }
        )));
    }
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
///
/// 脚本里查「已封」与下「封禁」只共用一个判据（`backend()`），三个后端都按「真在跑」来选：
/// 曾经 `blocked_ips()` 按 `command -v firewall-cmd` 认 firewalld、`block()` 又是一条 elif 链，
/// 于是「装了 firewalld 但没跑」的机器上守护每分钟失败一次，而界面读的是另一套后端 ——
/// 两边各说各话，防护等于没开。改这里时要同步 `SCAN_SCRIPT` 与 `rule_script` 的判据。
const GUARD_SCRIPT: &str = r#"#!/bin/sh
# DeployCode server-side guard (installed by the DeployCode app).
[ -f /etc/deploycode-guard.conf ] && . /etc/deploycode-guard.conf
THRESHOLD=${THRESHOLD:-5}
WINDOW=${WINDOW:-10}
# 免封白名单（逗号分隔的单个 IP）：命中就不计数，也不会被拉黑。
ALLOW=${ALLOW:-}
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

whitelisted() {
  for entry in $(printf '%s' "$ALLOW" | tr ',' ' '); do
    [ "$entry" = "$1" ] && return 0
  done
  return 1
}

# 三个后端只在这里判一次：查已封与下封禁必须落在同一个后端上，否则界面与守护各说各话。
backend() {
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi 'Status: active'; then
    echo ufw
  elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state 2>/dev/null | grep -qx running; then
    echo firewalld
  else
    echo iptables
  fi
}

blocked_ips() {
  case "$(backend)" in
    ufw)
      ufw status 2>/dev/null | grep -i deny | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}|[0-9a-fA-F]{0,4}(:[0-9a-fA-F]{0,4}){2,}'
      ;;
    firewalld)
      # 只认 drop 那一条：富规则里 accept / log / masquerade 也带 source address，
      # 少了这层过滤就会把「放行过谁」当成「已经封了谁」，那个 IP 从此再也封不掉。
      firewall-cmd --list-rich-rules 2>/dev/null | grep -i drop | grep -oE 'address="[^"]+"' | cut -d'"' -f2
      ;;
    *)
      { iptables -S INPUT 2>/dev/null; ip6tables -S INPUT 2>/dev/null; } | grep -Ei 'DROP|REJECT' | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}|[0-9a-fA-F]{0,4}(:[0-9a-fA-F]{0,4}){2,}'
      ;;
  esac
}

block() {
  ip=$1
  case "$ip" in
    *:*) family=ipv6 ;;
    *) family=ipv4 ;;
  esac
  case "$(backend)" in
    ufw)
      ufw deny from "$ip" >/dev/null 2>&1
      ;;
    firewalld)
      firewall-cmd --permanent --add-rich-rule="rule family=$family source address=$ip drop" >/dev/null 2>&1 &&
        firewall-cmd --reload >/dev/null 2>&1
      ;;
    *)
      if [ "$family" = "ipv6" ] && command -v ip6tables >/dev/null 2>&1; then
        ip6tables -I INPUT -s "$ip" -j DROP >/dev/null 2>&1
      else
        iptables -I INPUT -s "$ip" -j DROP >/dev/null 2>&1
      fi
      ;;
  esac
}

BLOCKED=$(blocked_ips)
failed | grep -oE 'from [0-9a-fA-F:.]+' | awk '{print $2}' | sort | uniq -c | while read count ip; do
  whitelisted "$ip" && continue
  [ "${count}" -ge "${THRESHOLD}" ] 2>/dev/null || continue
  echo "$BLOCKED" | grep -qxF "$ip" && continue
  if block "$ip"; then
    echo "$(date '+%F %T') blocked $ip after ${count} failed logins" >> "$LOG"
  else
    echo "$(date '+%F %T') FAILED to block $ip after ${count} failed logins" >> "$LOG"
  fi
done
"#;

/// 启用服务器端自动防护：写入守护脚本 + 配置（含免封白名单）+ 每分钟 cron 任务。
pub async fn enable_guard(
    server: &ServerConfig,
    connect_timeout_secs: u64,
    threshold: u32,
    window_mins: u64,
    whitelist: &[String],
) -> Result<String> {
    let threshold = threshold.clamp(1, 100);
    let window_mins = window_mins.clamp(1, 1440);
    let allow = normalize_whitelist(whitelist)?.join(",");
    let command = enable_script(threshold, window_mins, &allow);
    run_rule_command(server, connect_timeout_secs, &command, "启用自动防护").await
}

/// 启用自动防护的远端脚本：来源地址硬闸 + 守护脚本本体 + 配置（阈值 / 窗口 / 白名单）+ cron 任务。
fn enable_script(threshold: u32, window_mins: u64, allow: &str) -> String {
    let quoted = shell_quote(allow);
    format!(
        r#"SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
# 自我封锁保护：下面最后一步会当场跑一遍守护（不等 cron 那一分钟），所以本次连接的来源
# 地址没在白名单里时，点「启用」就是自己把自己封掉。这里直接拒绝、一个字节都不写，
# 界面上那颗「加入白名单」按钮点完就能过。读不到 SSH_CLIENT 时不拒绝（与手动拉黑同口径），
# 那种情况下界面上另有「来源地址未知」的告警。
ALLOW={quoted}
SELF="${{SSH_CLIENT%% *}}"
if [ -n "$SELF" ]; then
  hit=0
  for entry in $(printf '%s' "$ALLOW" | tr ',' ' '); do
    [ "$entry" = "$SELF" ] && hit=1
  done
  if [ "$hit" = "0" ]; then
    echo "启用被拒绝：$SELF 是本次 SSH 连接的来源地址且不在免封白名单里，守护脚本一跑就会把它封掉。请先把它加入白名单。" >&2
    exit 1
  fi
fi
$SUDO tee /usr/local/bin/deploycode-guard >/dev/null <<'GUARD_EOF' || exit 1
{GUARD_SCRIPT}
GUARD_EOF
$SUDO tee /etc/deploycode-guard.conf >/dev/null <<'CONF_EOF' || exit 1
THRESHOLD={threshold}
WINDOW={window_mins}
ALLOW={allow}
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
    )
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
# 自我封锁保护：白名单命中、或封的正是本次连接的来源地址时直接拒绝。
# 用 grep 读那一行而不是 source 整份配置：`.` 打不开文件会让脚本直接退出，
# 配置没装过（第一次拉黑）时就会被这条莫名其妙的原因绊住。
ALLOW="$(grep -E '^ALLOW=' /etc/deploycode-guard.conf 2>/dev/null | tail -n 1 | cut -d= -f2-)"
SELF="${{SSH_CLIENT%% *}}"
if [ -n "$SELF" ] && [ "$SELF" = "$IP" ]; then
  echo "拉黑被拒绝：$IP 是本次 SSH 连接的来源地址，封掉会立刻断开自己" >&2
  exit 1
fi
for entry in $(printf '%s' "$ALLOW" | tr ',' ' '); do
  if [ "$entry" = "$IP" ]; then
    echo "拉黑被拒绝：$IP 在免封白名单里，请先从白名单移除" >&2
    exit 1
  fi
done
done=0
if command -v ufw >/dev/null 2>&1 && $SUDO ufw status 2>/dev/null | grep -qi 'Status: active'; then
  $SUDO ufw deny from "$IP" >/dev/null 2>&1 && done=1
fi
if [ "$done" = "0" ] && command -v firewall-cmd >/dev/null 2>&1 && $SUDO firewall-cmd --state 2>/dev/null | grep -qx running; then
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
  $SUDO ufw status 2>/dev/null | grep -Ei 'DENY|REJECT' | grep -qwF "$IP" && still=1
fi
if command -v firewall-cmd >/dev/null 2>&1; then
  $SUDO firewall-cmd --list-rich-rules 2>/dev/null | grep -i drop | grep -qwF "$IP" && still=1
fi
if command -v "$BIN" >/dev/null 2>&1; then
  $SUDO $BIN -S INPUT 2>/dev/null | grep -Ei 'DROP|REJECT' | grep -qwF "$IP" && still=1
fi
if [ "$still" != "0" ]; then
  echo "解除失败：IP 仍在拦截列表中（权限不足或规则来源未知）" >&2
  exit 1
fi
echo "已解除 $IP"
# 自动防护是每分钟一轮：刚放出来的人如果还在超阈值地失败，下一分钟就被原样封回去，
# 而界面那句「已解除」早在弹过了 —— 用户看到的现象是「放出来又锁死」，得当场讲清楚。
if [ -f /etc/cron.d/deploycode-guard ]; then
  . /etc/deploycode-guard.conf 2>/dev/null
  THRESHOLD=${{THRESHOLD:-5}}
  WINDOW=${{WINDOW:-10}}
  ALLOW="$(grep -E '^ALLOW=' /etc/deploycode-guard.conf 2>/dev/null | tail -n 1 | cut -d= -f2-)"
  hit=0
  for entry in $(printf '%s' "$ALLOW" | tr ',' ' '); do
    [ "$entry" = "$IP" ] && hit=1
  done
  if [ "$hit" = "0" ]; then
    cnt=$(journalctl -u sshd -u ssh --since=-${{WINDOW}}min --no-pager 2>/dev/null | grep -Ei 'Failed password|Invalid user' | grep -cF "from $IP")
    if [ "${{cnt:-0}}" -ge "$THRESHOLD" ] 2>/dev/null; then
      echo "注意：它在 $WINDOW 分钟内失败 $cnt 次，已达自动防护阈值 $THRESHOLD —— 下一轮守护会把它重新封掉。要长期放行，请把它加进免封白名单。"
    fi
  fi
fi
exit 0
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

/// 白名单条数上限：守护脚本每分钟都要遍历一次，够日常运维用就行。
const WHITELIST_MAX: usize = 64;

/// 校验并整理白名单：去空白、去重、限长，返回可写进配置的条目。
fn normalize_whitelist(entries: &[String]) -> Result<Vec<String>> {
    let mut result: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if !is_ip(entry) {
            return Err(CoreError::config(format!(
                "白名单只接受单个 IP 地址（不支持主机名与网段）: {entry}"
            )));
        }
        // 比对前先规范化：`2001:0db8::1` 与 `2001:db8::1` 在远端匹配时是同一个地址。
        let normalized = entry
            .trim_matches(|c| c == '[' || c == ']')
            .split('%')
            .next()
            .unwrap_or(entry)
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| entry.to_string());
        if result.iter().any(|kept| kept == &normalized) {
            continue;
        }
        result.push(normalized);
        if result.len() > WHITELIST_MAX {
            return Err(CoreError::config(format!(
                "白名单最多 {WHITELIST_MAX} 条，请删掉不再使用的地址"
            )));
        }
    }
    Ok(result)
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
    let mut whitelist: Vec<String> = Vec::new();
    let mut self_ip = String::new();

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
                // 只认这一行里的合法地址：脚本已按字符集过滤，这里再兜一道，
                // 免得被日志成串的文本污染 selfIp（界面拿它做预填和高亮）。
                "SELF" => {
                    if is_ip(value) {
                        self_ip = value.to_string();
                    }
                }
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
                    // 不能按 ':' 切分：IPv6 地址本身含冒号。
                    for token in line[idx..].split([' ', ',', '\t']) {
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
                } else if let Some(value) = line.strip_prefix("ALLOW=") {
                    // 老版本守护配置没有这一行；空值就是没配白名单。
                    for token in value.split(',') {
                        let token = token.trim();
                        if is_ip(token) && !whitelist.iter().any(|kept| kept == token) {
                            whitelist.push(token.to_string());
                        }
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
    // 白名单只挡住"以后"的拉黑；已经被封的（老配置、或封之后才加的白名单）要在这里点出来，
    // 否则界面列着一堆已封 IP，看不出哪个是必须马上解除的。
    let locked_out: Vec<String> = blocked
        .iter()
        .filter(|ip| whitelist.iter().any(|allowed| allowed == *ip))
        .cloned()
        .collect();
    if !locked_out.is_empty() {
        notes.push(format!(
            "白名单里的这些 IP 当前仍被拦截，需要手动解除：{}",
            locked_out.join("、")
        ));
    }

    SecurityReport {
        is_root,
        has_sudo,
        firewall,
        blocked,
        guard_enabled: guard_conf && guard_cron,
        guard_threshold,
        guard_window_mins,
        whitelist,
        self_ip,
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
    fn parse_report_collects_ipv6_banned_ips() {
        let report = parse_report(
            "###FW ufw\n###F2B\nBanned IP list: 1.2.3.4 2001:db8::1,fe80::2%eth0\n###DONE\n",
        );
        assert!(report.blocked.iter().any(|ip| ip == "1.2.3.4"));
        assert!(report.blocked.iter().any(|ip| ip == "2001:db8::1"));
        assert!(report.blocked.iter().any(|ip| ip == "fe80::2%eth0"));
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

    #[test]
    fn normalize_whitelist_keeps_plain_ips() {
        let entries = [" 1.2.3.4 ".to_string(), String::new(), "2001:db8::1".to_string()];
        assert_eq!(
            normalize_whitelist(&entries).unwrap(),
            vec!["1.2.3.4".to_string(), "2001:db8::1".to_string()]
        );
    }

    #[test]
    fn normalize_whitelist_dedups_by_normalized_address() {
        let entries = [
            "[2001:0db8::1]".to_string(),
            "2001:db8::1".to_string(),
            "fe80::1%eth0".to_string(),
        ];
        assert_eq!(
            normalize_whitelist(&entries).unwrap(),
            vec!["2001:db8::1".to_string(), "fe80::1".to_string()]
        );
    }

    #[test]
    fn normalize_whitelist_rejects_cidr_and_hostnames() {
        // 白名单只收单个 IP：网段匹配要在远端 sh 里另写一套 IPv6 逻辑，宁可拒绝也不静默收下。
        for bad in ["10.0.0.0/8", "example.com", "1.2.3", "0.0.0.0"] {
            assert!(
                normalize_whitelist(&[bad.to_string()]).is_err(),
                "{bad} 应当被拒绝"
            );
        }
    }

    #[test]
    fn normalize_whitelist_caps_entry_count() {
        let entries: Vec<String> = (0..=WHITELIST_MAX)
            .map(|i| format!("10.0.0.{}", i % 251 + 1))
            .collect();
        assert!(normalize_whitelist(&entries).is_err());
    }

    #[test]
    fn parse_report_reads_whitelist_and_self_ip() {
        let report = parse_report(
            "###SELF 9.9.9.9\n###ROOT 0\n###FW ufw\nDENY FROM 1.2.3.4\n###GUARD\nTHRESHOLD=3\nWINDOW=15\nALLOW=1.2.3.4,2001:db8::1\nCRON yes\n###DONE\n",
        );
        assert_eq!(report.self_ip, "9.9.9.9");
        assert_eq!(report.guard_threshold, 3);
        assert_eq!(report.guard_window_mins, 15);
        assert!(report.guard_enabled);
        assert_eq!(report.whitelist, vec!["1.2.3.4", "2001:db8::1"]);
        // ###SELF 那行不能被当成防火墙规则收进已封列表。
        assert!(!report.blocked.iter().any(|ip| ip == "9.9.9.9"));
        // 白名单里的 IP 仍在拦截列表里，必须靠 note 提醒（不自动解除）。
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("仍被拦截") && note.contains("1.2.3.4")));
    }

    #[test]
    fn parse_report_tolerates_guard_config_without_allow_line() {
        // 老版本装的守护配置没有 ALLOW=，白名单读成空，不能因此报错。
        let report = parse_report("###GUARD\nTHRESHOLD=5\nWINDOW=10\nCRON yes\n###DONE\n");
        assert!(report.whitelist.is_empty());
        assert!(report.self_ip.is_empty());
        assert!(report.guard_enabled);
    }

    #[test]
    fn parse_report_ignores_malformed_allow_and_self_values() {
        let report = parse_report(
            "###SELF not-an-ip\n###GUARD\nALLOW=1.2.3.4,,garbage\nCRON yes\n###DONE\n",
        );
        assert_eq!(report.whitelist, vec!["1.2.3.4"]);
        assert_eq!(report.self_ip, "");
    }

    #[test]
    fn guard_script_skips_whitelisted_ips() {
        assert!(GUARD_SCRIPT.contains("ALLOW=${ALLOW:-}"));
        assert!(GUARD_SCRIPT.contains("whitelisted \"$ip\" && continue"));
    }

    /// 查「已封」与下「封禁」必须落在同一个后端，而且只认 drop 那一条。
    ///
    /// 曾经守护的 firewalld 分支没有 drop 过滤（accept / log / masquerade 富规则里的源地址
    /// 也算「已封」），那个 IP 从此再也封不掉、还一行日志都不留；而 `block()` 只看
    /// `command -v firewall-cmd`，「装了 firewalld 但没跑」的机器上它绝不退到 iptables。
    #[test]
    fn guard_scan_and_manual_rule_agree_on_the_backend() {
        assert!(GUARD_SCRIPT
            .contains("firewall-cmd --list-rich-rules 2>/dev/null | grep -i drop | grep -oE"));
        assert!(SCAN_SCRIPT.contains("firewall-cmd --list-rich-rules 2>/dev/null | grep -i drop"));
        // 三处都按 --state 判活：界面显示的后端必须就是守护真能下手的那个。
        // 必须是 `-qx`：`firewall-cmd --state` 在没跑的时候打印 `not running`，
        // 少了整行匹配就等于「没跑的 firewalld」被选中，而它每条命令都会失败。
        assert!(GUARD_SCRIPT.contains("firewall-cmd --state 2>/dev/null | grep -qx running"));
        assert!(SCAN_SCRIPT.contains("firewall-cmd --state 2>/dev/null | grep -qx running"));
        let block = rule_script("203.0.113.9", true);
        assert!(block.contains("firewall-cmd --state 2>/dev/null | grep -qx running"), "{block}");
        // 选不中 firewalld 时要能退到 iptables/ip6tables（守护那条链同样要能退）。
        assert!(block.contains("-I INPUT -s \"$IP\" -j DROP"), "{block}");
        assert!(GUARD_SCRIPT.contains("-I INPUT -s \"$ip\" -j DROP"));
    }

    /// 脚本正文里绝不能混进 Rust 的 `///`：它会被原样 tee 到服务器上，而注释里的
    /// `blocked_ips()` 那种写法直接把整份守护打成语法错误 —— 防护静默失效。
    /// 同一条理由也约束 `SCAN_SCRIPT` 与 `enable_script`（后者整份嵌入 GUARD_SCRIPT）。
    #[test]
    fn shipped_scripts_carry_no_rust_doc_comments() {
        for (name, text) in [
            ("GUARD_SCRIPT", GUARD_SCRIPT),
            ("SCAN_SCRIPT", SCAN_SCRIPT),
            ("rule_script(block)", rule_script("1.2.3.4", true).as_str()),
            ("rule_script(unblock)", rule_script("1.2.3.4", false).as_str()),
            (
                "enable_script",
                enable_script(5, 10, "1.2.3.4").as_str(),
            ),
        ] {
            assert!(!text.contains("///"), "{name} 里混进了 Rust 文档注释");
            assert!(!text.contains("\\\\"), "{name} 里有反斜杠续行的可疑写法");
        }
    }

    /// 解除拉黑不碰免封白名单，而守护是每分钟一轮：仍在窗口里超阈值的 IP 会被原样封回，
    /// 界面上那句「已解除」就成了假象。这一层必须当场说出来。
    #[test]
    fn unblock_rule_warns_when_the_guard_will_reblock() {
        let script = rule_script("203.0.113.9", false);
        assert!(script.contains("/etc/cron.d/deploycode-guard"), "{script}");
        assert!(script.contains("重新封"), "{script}");
        // 已经在白名单里的那条不该吓用户：守护本来就封不了它。
        assert!(script.contains("[ \"$entry\" = \"$IP\" ] && hit=1"), "{script}");
        // 复核同样只认 drop 富规则：accept 里出现同一个地址不等于还在被封。
        assert!(
            script.contains("--list-rich-rules 2>/dev/null | grep -i drop | grep -qwF"),
            "{script}"
        );
        // ufw 的 ALLOW / iptables 的 ACCEPT 里出现同一个地址也不等于还封着：
        // 少了这层过滤，被放行过的 IP 会永远停在「解除失败」。
        assert!(
            script.contains("grep -Ei 'DENY|REJECT' | grep -qwF \"$IP\""),
            "{script}"
        );
        assert!(
            script.contains("grep -Ei 'DROP|REJECT' | grep -qwF \"$IP\""),
            "{script}"
        );
    }

    #[test]
    fn block_rule_refuses_self_and_whitelisted_before_touching_firewall() {
        let script = rule_script("1.2.3.4", true);
        // 来源地址取自本次连接的 SSH_CLIENT，白名单从服务器那份配置里 grep 出来。
        assert!(script.contains("${SSH_CLIENT%% *}"));
        assert!(script.contains("grep -E '^ALLOW=' /etc/deploycode-guard.conf"));
        // 检查必须排在防火墙分支之前。
        let guard_at = script.find("拉黑被拒绝").unwrap();
        let firewall_at = script.find("done=0").unwrap();
        assert!(guard_at < firewall_at);
        // 拼进远端 shell 的变量必须逐个过 shell_quote（安全红线）：日志里的 [IPv6] 写法
        // 带方括号，不加引号会被当成 glob 展开。
        assert!(script.contains("IP=1.2.3.4"));
        assert!(rule_script("[2001:db8::1]", true).contains("IP='[2001:db8::1]'"));
        // 解除方向不该有这些拒绝分支。
        assert!(!rule_script("1.2.3.4", false).contains("拉黑被拒绝"));
    }

    #[test]
    fn enable_script_writes_whitelist_into_guard_conf() {
        let script = enable_script(3, 15, "1.2.3.4,2001:db8::1");
        assert!(script.contains("THRESHOLD=3\nWINDOW=15\nALLOW=1.2.3.4,2001:db8::1\n"));
        // 清空白名单也必须写一行空的 ALLOW=：整份配置是重写而不是合并，
        // 否则服务器上残留的旧名单会继续给那批地址免封。
        assert!(enable_script(5, 10, "").contains("ALLOW=\n"));
    }

    /// 启用的最后一步会**当场**跑一遍守护脚本，所以来源地址硬闸必须排在写文件之前：
    /// 拦下时一个字节都不该落到服务器上，否则防护照样装上了、只是没提示。
    #[test]
    fn enable_script_refuses_self_before_installing_anything() {
        let script = enable_script(5, 10, "1.2.3.4");
        assert!(script.contains("SELF=\"${SSH_CLIENT%% *}\""));
        assert!(script.contains("启用被拒绝"));
        // 名单里就有来源地址时才会放行到写文件那一步，闸读的就是本次要写进去的那份名单。
        assert!(script.contains("ALLOW=1.2.3.4\nSELF="));
        let refuse_at = script.find("启用被拒绝").unwrap();
        let install_at = script.find("tee /usr/local/bin/deploycode-guard").unwrap();
        let run_at = script.find("/usr/local/bin/deploycode-guard >/dev/null 2>&1 || true").unwrap();
        assert!(
            refuse_at < install_at && install_at < run_at,
            "拒绝分支没排在安装与当场执行之前"
        );
        // 空白名单也要有这道闸：那时任何已知来源地址都不在名单里。
        assert!(enable_script(5, 10, "").contains("ALLOW=''"));
    }

    /// Rust 与 TS 是手工镜像的，字段名对不上编译器不报错、界面静默拿到 undefined。
    /// 这条把安全弹窗读到的键名钉死（尤其后两个新字段）。
    #[test]
    fn report_json_keys_match_the_frontend_types() {
        let report = parse_report(
            "###SELF 1.2.3.4\n###GUARD\nTHRESHOLD=5\nWINDOW=10\nALLOW=5.6.7.8\nCRON yes\n###DONE\n",
        );
        let value = serde_json::to_value(&report).expect("序列化");
        for key in [
            "isRoot",
            "hasSudo",
            "firewall",
            "blocked",
            "guardEnabled",
            "guardThreshold",
            "guardWindowMins",
            "whitelist",
            "selfIp",
            "failed",
            "success",
            "sessions",
            "scannedAt",
            "sshd",
            "notes",
        ] {
            assert!(value.get(key).is_some(), "缺字段 {key}");
        }
        assert_eq!(value["selfIp"].as_str(), Some("1.2.3.4"));
        assert_eq!(value["whitelist"].as_array().map(Vec::len), Some(1));
    }
}
