//! 控制机（备份 agent）协议与管理端。
//!
//! 一台服务器常驻一个 `deploy-agent` 进程（`crates/deploy-agent`），它负责数据库备份与
//! 容器备份的**定时触发与编排**，备份包留在它自己的磁盘上。本机客户端只做三件事：
//! 把配置和其余服务器的 SSH 凭据下发过去、发起一次执行并接收事件流、按需回读执行记录。
//!
//! 三条约束（都是有意为之，改动前请先读）：
//!
//! 1. **不开任何监听端口**：所有交互都走客户端已有的 SSH 通道（`SshClient`），控制机上
//!    没有任何对外的服务端口。安装、下发、回读、即时执行全部是「一次 ssh exec」。
//! 2. **版本握手**：`proto` 是整数，客户端每次下发前先跑 `deploy-agent --version` 比对，
//!    不一致就拒绝下发、只提示升级 agent。宁可功能不可用，也不要旧 agent 按新语义跑备份。
//! 3. **下发的是配置子集，不是整份 config.json**：见 [`build_bundle`]。仓库/部署/Pages 配置
//!    与本机 Token 都不过去，agent 用不着；三条 `*_last_run` 也不过去，那是 agent 自己的事
//!    （它记在 `schedule-state.json`，否则每次同步都会把「今天已经跑过」冲掉）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::disk;
use crate::error::{CoreError, Result};
use crate::models::{
    AppConfig, BackupEvent, BackupRecord, ContainerEvent, ContainerRecord, Settings,
};
use crate::process::shell_quote;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;
use crate::util::human_size;

/// 协议版本：agent 与客户端任一侧不一致时，客户端拒绝下发配置。
///
/// 只要 `trigger` 的事件流格式、`status` 字段含义、备份/恢复的落盘语义发生变化就要 +1。
/// **加字段同样要 +1**：[`AgentStatus`] 与回读的记录都是严格 serde 结构（没有 `#[serde(default)]`），
/// 一侧多了字段而另一侧不认识，整份 JSON 就解析失败 —— 与其让它以「控制机返回的内容无法解析」
/// 收场，不如握手时就拦下来。给字段加 default 是另一种做法，但那会让真正写坏的输出被静默兜成
/// 空值，这里宁可响亮地失败。
///
/// 唯一的例外是 `service_active`：它带 default，而且**从来不指望 agent 提供**（由客户端问
/// systemd 填，见 [`parse_service_state`]），所以新旧 agent 都不会因为它而解析失败，不需要 +1。
pub const PROTO: u32 = 1;

/// systemd 服务名，同时是 journal 的过滤条件。
pub const SERVICE: &str = "deploy-agent";
pub const BIN_PATH: &str = "/usr/local/bin/deploy-agent";
pub const DATA_DIR: &str = "/var/lib/deploycode";
pub const UNIT_PATH: &str = "/etc/systemd/system/deploy-agent.service";
/// 回读执行日志的默认行数。
pub const DEFAULT_LOG_LINES: usize = 200;

/// 手放 agent 可执行文件的位置：`<数据目录>/agent/deploy-agent`，安装包没内置时（开发期交叉编译）用这一档。
pub const LOCAL_BINARY_SUBDIR: &str = "agent";

/// 控制机上数据库导出包的兜底份数（本机 `db_bundle_keep` = 0 时用它）。
///
/// 本机那个 0 是「别往我盘上写」，而控制机不留包就等于这次备份白跑：既不能恢复也不能迁移。
/// 2 份够回滚到昨天和前天，再由磁盘水位闸兜住容量。
pub const AGENT_DB_BUNDLE_KEEP: usize = 2;

/// `deploy-agent --version` 的输出与该结构一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersion {
    pub proto: u32,
    pub version: String,
}

impl AgentVersion {
    /// agent 打印的那一行：`deploy-agent <version> proto=<n>`。
    pub fn line(&self) -> String {
        format!("{} {} proto={}", SERVICE, self.version, self.proto)
    }
}

/// 解析 `--version` 输出；取文本里第一个能认出来的版本行，容忍 shell profile 的杂项输出。
pub fn parse_version(text: &str) -> Option<AgentVersion> {
    for raw in text.lines() {
        let line = raw.trim();
        // 认不出来的行直接跳过：登录 shell 的欢迎语、`bash: deploy-agent: command not found`
        // 这种半途像又不像的输出，都不该让整次握手失败得看不懂。
        let Some(rest) = line.strip_prefix(SERVICE) else {
            continue;
        };
        let Some((version, tail)) = rest.trim().split_once(' ') else {
            continue;
        };
        let Some(proto) = tail
            .split_whitespace()
            .find_map(|part| part.strip_prefix("proto="))
        else {
            continue;
        };
        let Ok(proto) = proto.parse::<u32>() else {
            continue;
        };
        if version.is_empty() {
            continue;
        }
        return Some(AgentVersion {
            proto,
            version: version.to_string(),
        });
    }
    None
}

/// 解析 `date +%z` 那类输出里的 UTC 偏移，返回分钟数（东为正）：`+0800` → 480。
///
/// 容忍前后有杂项（登录 shell 的欢迎语、`\r`）：只认第一个形如 `±HHMM` 的片段。
fn parse_utc_offset(text: &str) -> Option<i32> {
    for (index, byte) in text.as_bytes().iter().enumerate() {
        if !matches!(byte, b'+' | b'-') {
            continue;
        }
        if let Some(offset) = offset_after(text, index, *byte) {
            return Some(offset);
        }
    }
    None
}

/// 从 `index` 处那个 +/- 开始读偏移：`+0800`、`+08:00`、`+8` 都认；认不出来返回 None，
/// 让外层继续往后找（前面可能是 shell 打的无关字符）。
fn offset_after(text: &str, index: usize, sign: u8) -> Option<i32> {
    let tail = &text[index + 1..];
    let run: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    let (hour, minute) = if run.len() >= 3 {
        // `+HHMM`：末两位是分。
        (run[..run.len() - 2].parse().ok()?, run[run.len() - 2..].parse().ok()?)
    } else {
        // `+HH` 或 `+HH:MM`。
        let hours: i32 = run.parse().ok()?;
        let after = tail[run.len()..].strip_prefix(':').unwrap_or("");
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let minutes: i32 = if digits.is_empty() { 0 } else { digits.parse().ok()? };
        (hours, minutes)
    };
    if hour > 14 || minute > 59 {
        return None;
    }
    let value = hour * 60 + minute;
    Some(if sign == b'-' { -value } else { value })
}

/// 版本不匹配时给用户的说法（面向界面文案，中文）。
pub fn version_mismatch(local: &AgentVersion, remote: &AgentVersion) -> String {
    format!(
        "控制机上的 agent 版本是 {}（协议 {}），本机客户端要求协议 {}。请重新构建 agent 并点「安装/更新」后再同步。",
        remote.version, remote.proto, local.proto
    )
}

/// 客户端这一侧的协议声明（版本号取 workspace 常量，两侧同仓构建时天然对齐）。
pub fn local_version() -> AgentVersion {
    AgentVersion {
        proto: PROTO,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// systemd unit。root 运行：要 `docker exec` 进各业务的容器、要写 `/var/lib/deploycode`。
///
/// `MemoryMax` / `CPUQuota` 是护栏之一：一次容器快照是几十分钟的 tar，
/// 不限流会把同机的业务容器饿死。`Restart=always` + `RestartSec` 让进程崩了能回来，
/// 也因此「触发日期必须落盘」（见 [`crate::schedule::ScheduleState`]）。
///
/// `StartLimit*` 是配套的刹车：没有上限时，一个确定性崩溃就是每 30 秒一次的启动风暴，
/// journal 被刷满也等不来人来修；到限就让它停在 failed 状态。
pub fn unit_template() -> String {
    format!(
        r#"[Unit]
Description=DeployCode backup agent (control plane)
After=network-online.target docker.service
Wants=network-online.target
StartLimitBurst=5
StartLimitIntervalSec=600

[Service]
Type=simple
ExecStart={bin} run --data-dir {dir}
Restart=always
RestartSec=30
User=root
MemoryMax=1G
CPUQuota=200%
LimitNOFILE=4096
SyslogIdentifier={service}
# 日志不在这里配额：unit 里写不出「本服务的 journal 上限」（journald 的配额是全局的
# SystemMaxUse），一行非法指令只会被静默忽略。回读时一句 journalctl -u {service} 就够。

[Install]
WantedBy=multi-user.target
"#,
        bin = BIN_PATH,
        dir = DATA_DIR,
        service = SERVICE,
    )
}

/// 安装时执行的那段 shell：停旧进程、落二进制与 unit、daemon-reload、enable --now。
///
/// `install -m 755` 的源文件是 SFTP 先传到 `/tmp` 的那份；这里只做搬动与权限。
fn install_script(binary_tmp: &str, unit: &str) -> String {
    format!(
        r#"set -e
SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
# 必须先停：`enable --now` 对已经在跑的服务什么都不做，旧进程会接着按旧代码调度，
# 而下面那行 `--version` 打的是刚落盘的新二进制 —— 界面与握手都会以为「升级成功」。
# 停在半路的那个任务不假装成功：agent 启动时用 reconcile_interrupted 把它收成失败。
$SUDO systemctl stop {service} 2>/dev/null || true
$SUDO install -m 755 {tmp} {bin}
$SUDO mkdir -p {dir}
$SUDO chmod 700 {dir}
$SUDO tee {unit_path} >/dev/null <<'AGENT_UNIT_EOF'
{unit}
AGENT_UNIT_EOF
$SUDO chmod 644 {unit_path}
$SUDO systemctl daemon-reload
$SUDO systemctl enable --now {service}
$SUDO systemctl is-active --quiet {service}
{bin} --version
"#,
        tmp = binary_tmp,
        bin = BIN_PATH,
        dir = DATA_DIR,
        unit_path = UNIT_PATH,
        unit = unit.trim(),
        service = SERVICE,
    )
}

/// 下发配置时执行的那段 shell：把 `/tmp` 那份搬成 `{DATA_DIR}/config.json`。
///
/// 先 `cp` 到同目录的 staging 名再 `mv -f` —— 同文件系统内的 `mv` 是 rename，读者要么看到
/// 旧的一份、要么看到新的一份。直接覆盖目标文件（`install`/`cp` 到 config.json）是原地截断，
/// 而 agent 每 20 秒就重读一次，正好跨过定时点时读到的半截 JSON 会把那一晚整个吞掉。
/// 上传的临时文件在 `/tmp`，跨设备 `mv` 会退化成复制，所以必须先在数据目录里落一版。
fn config_install_script(remote_tmp: &str) -> String {
    format!(
        r#"set -e
SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
$SUDO mkdir -p {dir}
$SUDO rm -f {dir}/config.json.staging
$SUDO cp {tmp} {dir}/config.json.staging
$SUDO chmod 600 {dir}/config.json.staging
$SUDO mv -f {dir}/config.json.staging {dir}/config.json
"#,
        dir = DATA_DIR,
        tmp = shell_quote(remote_tmp),
    )
}

/// 卸载：停服务、删 unit 与二进制，**保留** `/var/lib/deploycode` 里的备份包与记录，
/// 但必须抹掉那份 `config.json` —— 它是下发时写进去的，含全部源机的明文 SSH 口令与数据库密码，
/// 留着等于把一整套凭据丢在一台已经不再为我们工作的机器上。
fn uninstall_script() -> String {
    format!(
        r#"set -e
SUDO=""
[ "$(id -u)" != "0" ] 2>/dev/null && SUDO="sudo -n"
$SUDO systemctl disable --now {service} 2>/dev/null || true
$SUDO systemctl stop {service} 2>/dev/null || true
$SUDO rm -f {unit_path} {bin}
$SUDO rm -f {dir}/config.json {dir}/config.json.staging
$SUDO systemctl daemon-reload
echo 'kept:{dir}'
"#,
        service = SERVICE,
        unit_path = UNIT_PATH,
        bin = BIN_PATH,
        dir = DATA_DIR,
    )
}

/// 问 systemd：控制机上那份常驻服务在不在跑。
///
/// 没让 agent 的 `status` 自己报，有两个原因：这条命令是临时 exec 出来的进程，它对「那个
/// 常驻进程活着没」天生答不了；而控制机上装的可能是没有这个能力的新旧二进制 —— 由客户端问
/// systemd，答案与二进制版本无关。
async fn service_state(client: &SshClient) -> Option<bool> {
    let command = format!(
        "if command -v systemctl >/dev/null 2>&1; then systemctl is-active {service} 2>/dev/null; fi",
        service = SERVICE
    );
    let (_, out) = client.exec_capture(&command, 20).await.ok()?;
    parse_service_state(&out)
}

/// 取 `systemctl is-active` 那一行状态词。
fn parse_service_state(text: &str) -> Option<bool> {
    let line = text.lines().rev().map(str::trim).find(|line| !line.is_empty())?;
    match line {
        "active" | "activating" | "reloading" => Some(true),
        "inactive" | "failed" | "deactivating" => Some(false),
        // 没有 systemctl 的机器什么都不印；登录 shell 的欢迎语、`[stderr] ...` 这类杂音一律
        // 当「不知道」——不该让一句怪输出换来一个红色的「服务没跑」。
        _ => None,
    }
}

/// `deploy-agent status` 的返回。界面据此画控制机卡片。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub proto: u32,
    pub version: String,
    /// agent 主机的时区偏移（`+0800`）。定时时间按它的本地时区解释，界面必须展示，
    /// 否则用户没法判断设的 03:00 到底是哪一个 03:00。
    pub timezone: String,
    /// agent 主机的当前本地时间：与时区偏移一起，让用户一眼看出定时会不会跑偏。
    pub local_time: String,
    pub data_dir: String,
    /// 数据目录所在文件盘的总量 / 余量 / 水位线（字节）。
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub floor_bytes: u64,
    /// 已留存的备份包占用（两个目录合计）。
    pub bundle_bytes: u64,
    pub bundle_count: usize,
    pub servers: usize,
    pub backup_configs: usize,
    pub container_configs: usize,
    pub backup_enabled: bool,
    pub backup_time: String,
    pub backup_config_name: String,
    pub backup_last_run: String,
    pub container_enabled: bool,
    pub container_time: String,
    pub container_queue: usize,
    pub container_last_run: String,
    /// 控制机上那份**常驻服务**活着没有（systemd 的 `is-active`）。
    ///
    /// 由客户端在同一条 SSH 会话里问 systemd，不由 agent 自己报：`status` 那条命令是临时
    /// exec 起来的进程，它当然在跑，而「到点有没有人执行」问的正是那个 unit。缺了这一个字段，
    /// 服务被 StartLimitBurst 打死在 failed 状态时界面照样一片绿。
    /// `None` = 这台机器上读不到 systemd 答案，宁缺不猜。
    ///
    /// 它**不属于协议**：每一版 agent 都不会输出它（总是由客户端填），所以加它不需要动 [`PROTO`]。
    #[serde(default)]
    pub service_active: Option<bool>,
}

/// 一次下发（注入）的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSyncReport {
    pub servers: usize,
    pub backup_configs: usize,
    pub container_configs: usize,
    /// 需要用户注意但没有拦下本次同步的事项（例如定时指向了一条本机执行的配置）。
    pub warnings: Vec<String>,
    pub status: AgentStatus,
}

/// 上次成功下发到控制机的配置指纹（落盘在 `<数据目录>/agent-sync.json`）。
///
/// 存在的理由只有一个：定时循环要分清「这条配置的让位有没有人接」。执行位写成控制机
/// 却从没下发过（或下发时它还不存在、后来又改了名），控制机上就没有那一份 ——
/// 此时本机若照样让位，这一晚两头都不跑，而且一声不吭。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSyncState {
    /// 指纹是对哪台服务器记的：换了控制机必须重新下发才算数。
    #[serde(default)]
    pub server_id: String,
    /// 下发成功的时刻（展示用，界面告诉你控制机上那份有多旧）。
    #[serde(default)]
    pub synced_at: String,
    #[serde(default)]
    pub backup_config_ids: Vec<String>,
    #[serde(default)]
    pub container_config_ids: Vec<String>,
    /// 下发那一刻的定时备份口径（**本机**写下的值，不是折算到控制机时区之后的 HH:MM）。
    ///
    /// 开关、时间点、选中项都被 `build_bundle` 烘进 config.json，可它们不是「几条配置」，
    /// 所以条数对得上的时候界面一片绿，而控制机照旧跑上次那份：用户关掉的定时没关掉，
    /// 换掉的那条永远不备份。指纹记下这一份，界面与调度才说得出「那份是旧的」。
    #[serde(default)]
    pub backup_schedule: String,
    #[serde(default)]
    pub container_schedule: String,
    /// 已经不是当前控制机、但 agent 没收回来的那台（换控制机时旧那台连不上就会留在这里）。
    /// 它读的是自己盘上那份 config.json，到点照跑，而且那里面有全部源机的明文口令 ——
    /// 所以 [`AgentSyncState::clear`] 不动它，界面上要一直挂着直到收回成功。
    #[serde(default)]
    pub orphan_server_ids: Vec<String>,
}

impl AgentSyncState {
    /// 记一台待收回的旧控制机（去重）。
    pub fn add_orphan(&mut self, server_id: &str) {
        let id = server_id.trim();
        if !id.is_empty() && !self.orphan_server_ids.iter().any(|item| item == id) {
            self.orphan_server_ids.push(id.to_string());
        }
    }

    /// 收回成功（或那台服务器被删掉）之后撤掉这条待办。
    pub fn remove_orphan(&mut self, server_id: &str) {
        self.orphan_server_ids.retain(|item| item != server_id);
    }
    /// 这份指纹是不是给当前那台控制机记的。
    pub fn is_for(&self, server_id: &str) -> bool {
        !self.server_id.trim().is_empty() && self.server_id == server_id
    }

    pub fn holds_backup(&self, server_id: &str, config_id: &str) -> bool {
        self.is_for(server_id) && self.backup_config_ids.iter().any(|id| id == config_id)
    }

    pub fn holds_container(&self, server_id: &str, config_id: &str) -> bool {
        self.is_for(server_id) && self.container_config_ids.iter().any(|id| id == config_id)
    }

    /// 控制机上那份定时（数据库备份）与本机当前设置不一致。
    ///
    /// 从没下发过（`backup_schedule` 是空）不算「陈旧」而是「未下发」，由
    /// [`AgentSyncState::holds_backup`] 那条路报错，两头不重复提醒。
    pub fn backup_schedule_stale(&self, server_id: &str, key: &str) -> bool {
        self.is_for(server_id) && !self.backup_schedule.is_empty() && self.backup_schedule != key
    }

    pub fn container_schedule_stale(&self, server_id: &str, key: &str) -> bool {
        self.is_for(server_id) && !self.container_schedule.is_empty() && self.container_schedule != key
    }

    /// 控制机还持有、本机却已经不再交给它的配置名（执行位改回本机，或配置已被删除）。
    ///
    /// 那台机器读的是自己盘上的 config.json，不会自己停：不补一次下发，改回本机的那条
    /// 今晚两边各跑一次（两份包落在两台机器上，而跨机没有任务锁），被删掉的那条则继续
    /// 每晚替一个本机已经没有的配置导出。
    pub fn stragglers(&self, config: &AppConfig) -> Vec<String> {
        let mut names = Vec::new();
        for id in &self.backup_config_ids {
            match config.backup_configs.iter().find(|item| &item.id == id) {
                None => names.push(format!("已删除的备份配置 {id}")),
                Some(item) if !item.run_location.is_remote() => {
                    names.push(format!("「{}」已改回本机", item.name))
                }
                Some(_) => {}
            }
        }
        for id in &self.container_config_ids {
            match config.container_configs.iter().find(|item| &item.id == id) {
                None => names.push(format!("已删除的容器配置 {id}")),
                Some(item) if !item.run_location.is_remote() => {
                    names.push(format!("「{}」已改回本机", item.name))
                }
                Some(_) => {}
            }
        }
        names
    }

    /// 清空（卸载时调用）：留着一个指向不存在的服务器的指纹，比没有指纹更容易骗过让位判定。
    ///
    /// `orphan_server_ids` 不在清空范围内：它不是指纹，而是「还有哪台机器上没收回 agent」的待办，
    /// 卸载/换机把现役指纹清掉时那份待办得留着，否则旧控制机就悄悄没人管了。
    pub fn clear(&mut self) {
        self.server_id = String::new();
        self.synced_at = String::new();
        self.backup_config_ids = Vec::new();
        self.container_config_ids = Vec::new();
        self.backup_schedule = String::new();
        self.container_schedule = String::new();
    }
}

/// 下发那一刻的定时备份口径（本机值）。参与指纹比较，不参与折算。
pub fn backup_schedule_key(config: &AppConfig) -> String {
    format!(
        "{}|{}|{}",
        config.settings.scheduled_backup_enabled,
        config.settings.scheduled_backup_time.trim(),
        config
            .settings
            .scheduled_backup_config_id
            .as_deref()
            .unwrap_or("")
            .trim()
    )
}

/// 容器那边同理，只是选中项是一个列表（顺序即当晚执行顺序）。
pub fn container_schedule_key(config: &AppConfig) -> String {
    format!(
        "{}|{}|{}",
        config.settings.scheduled_container_enabled,
        config.settings.scheduled_container_time.trim(),
        config
            .settings
            .scheduled_container_config_ids
            .iter()
            .map(|id| id.trim())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// 「控制机上那份与本机设置已经不一致」的本机视图：纯读盘，不连服务器。
///
/// 存在的理由：`syncNeeded` 只比配置条数，看不出定时设置的改动，也看不出某条配置被改回
/// 本机 / 被删掉之后控制机还留着它那一半。这两类都会让控制机在夜里做出与界面相反的事。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStaleness {
    /// 定时的数据库备份改动还没下发（控制机上跑的是上次那份）。
    pub backup_schedule_stale: bool,
    /// 定时的容器备份改动还没下发。
    pub container_schedule_stale: bool,
    /// 控制机还持有、本机已不再交给它的配置（点名是谁）。
    pub stragglers: Vec<String>,
    /// 上次成功下发的时刻；空串 = 从没下发过。
    pub synced_at: String,
}

/// 只读本机的 config.json 与 agent-sync.json，供界面与调度提醒用。
pub fn staleness(store: &Store) -> AgentStaleness {
    let Ok(config) = store.load_config() else {
        // 读不到本机配置就当没有可提醒的：这一栏是提示，不该把设置页拖成报错。
        return AgentStaleness::default();
    };
    let sync = store.load_agent_sync();
    let server_id = config.settings.agent_server_id.trim();
    if !sync.is_for(server_id) {
        // 指纹不是当前这台控制机的（没装、换过机器、刚卸载）：那时是「未下发」，
        // 由让位判定报失败，这里不重复报陈旧。
        return AgentStaleness::default();
    }
    AgentStaleness {
        backup_schedule_stale: sync.backup_schedule_stale(server_id, &backup_schedule_key(&config)),
        container_schedule_stale: sync
            .container_schedule_stale(server_id, &container_schedule_key(&config)),
        stragglers: sync.stragglers(&config),
        synced_at: sync.synced_at.clone(),
    }
}

/// 本机要用的 agent 可执行文件是从哪一档来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentBinarySource {
    /// 客户端安装包自带的（`<资源目录>/agent/deploy-agent`），正常安装就是这一档。
    Bundled,
    /// `<数据目录>/agent/` 下手放的那份（开发期换产物用，压过内置）。
    Manual,
    /// 一处都没有：点「安装/更新」会报错。
    Missing,
}

/// 界面那一行「agent 可执行文件」的素材：只读展示，不再让用户敲路径。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBinaryInfo {
    pub source: AgentBinarySource,
    /// 绝对路径；[`AgentBinarySource::Missing`] 时是空串。
    pub path: String,
    pub size_bytes: u64,
    /// 一份都没有、或放了却不能用时的原因；空串表示可用。
    ///
    /// 不能只报「没内置」：用户在 `<数据目录>/agent/` 手放了一份 `.exe` 时，真实原因是那份不能用的
    /// 文件挡住了内置那份，把路径与原因写出来他才知道该删掉哪个（AGENTS.md：谁在用哪个要直接写出来）。
    #[serde(default)]
    pub error: String,
}

/// 这份文件能不能当 Linux agent 用：能用返回 `None`，不能用返回一句说明原因的话。
///
/// 只查 4 字节魔数是不够的 —— ARM 或 32 位的 ELF 也会通过，装上之后 systemd 起不来，
/// 而那时旧服务已经被 stop 掉了（安装脚本先 stop 再落盘）。x86_64 Linux 的组合是固定的：
/// ELFCLASS64 + 小端 + `e_machine = 0x3e`。
fn binary_problem(path: &Path) -> Option<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Some("文件读不出来（可能被别的进程占用）".to_string()),
    };
    let mut head = [0u8; 20];
    if std::io::Read::read_exact(&mut file, &mut head).is_err() {
        return Some("文件太短，不像可执行文件".to_string());
    }
    if head[..4] != [0x7f, b'E', b'L', b'F'] {
        return Some("文件头不是 ELF（本机编出来的 deploy-agent.exe 或占位文件都是这样）".to_string());
    }
    if head[4] != 2 {
        return Some("这是 32 位 ELF，控制机要 64 位".to_string());
    }
    if head[5] != 1 {
        return Some("这是大端 ELF，控制机要小端（x86_64）".to_string());
    }
    let machine = u16::from_le_bytes([head[18], head[19]]);
    if machine != 0x3e {
        return Some(format!(
            "这是架构 {machine:#04x} 的 ELF，控制机要 x86_64（0x3e）—— 用 --target x86_64-unknown-linux-musl 重新编一份"
        ));
    }
    None
}

/// 一个候选路径的两道检查：在不在、是不是能跑在 x86_64 Linux 上。
fn accept_binary(path: PathBuf, source: AgentBinarySource) -> Result<(PathBuf, AgentBinarySource)> {
    if !path.is_file() {
        return Err(CoreError::config(format!(
            "找不到 agent 可执行文件: {}",
            path.display()
        )));
    }
    if let Some(problem) = binary_problem(&path) {
        return Err(CoreError::config(format!(
            "{} 不能当 agent 用：{problem}。agent 跑在控制机（Linux）上，本机那份即使能双击也传不过去。",
            path.display()
        )));
    }
    Ok((path, source))
}

/// 找 agent 可执行文件：`<数据目录>/agent/` 下手放的那份 → 安装包内置的那份。
///
/// 刻意没有「设置里指一个路径」这一档：那是这台机器上的文件位置，写进配置就会被导出/导入
/// 带到别的机器上，而界面上早就没有输入框了，指错一路都清不掉。开发期要换一份测试，
/// 就把它放到 `<数据目录>/agent/deploy-agent` —— 这一档排在内置之前，否则安装包里的
/// 那份永远压着，本地新编的产物根本传不上去。谁在用哪个，界面的那一行会直接写出来，
/// 所以「手放的旧文件赢了」不是静默降级。
pub fn locate_binary(store: &Store) -> Result<(PathBuf, AgentBinarySource)> {
    let dir = store.base_dir().join(LOCAL_BINARY_SUBDIR);
    for name in ["deploy-agent", "deploy-agent.exe", "deploy-agent-linux"] {
        let path = dir.join(name);
        if path.is_file() {
            return accept_binary(path, AgentBinarySource::Manual);
        }
    }
    if let Some(bundled) = store.bundled_agent_binary() {
        let bundled = bundled.to_path_buf();
        if bundled.is_file() {
            return accept_binary(bundled, AgentBinarySource::Bundled);
        }
    }
    Err(CoreError::config(format!(
        "本机没有可用的 agent 可执行文件：安装包没内置（旧版客户端会这样），{} 下也没有。装最新客户端，或构建一份放进去：\n  cargo build -p deploy-agent --release --target x86_64-unknown-linux-musl\n（产物在 target/x86_64-unknown-linux-musl/release/deploy-agent）",
        dir.display()
    )))
}

/// 界面用的只读视图：找不到不报错，`source` 会是 `missing`、`error` 里写清楚为什么，
/// 让页面自己决定怎么提示。
pub fn binary_info(store: &Store) -> AgentBinaryInfo {
    match locate_binary(store) {
        Ok((path, source)) => AgentBinaryInfo {
            source,
            size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            error: String::new(),
            path: path.display().to_string(),
        },
        Err(err) => AgentBinaryInfo {
            source: AgentBinarySource::Missing,
            path: String::new(),
            size_bytes: 0,
            error: err.to_string(),
        },
    }
}

/// 本机要用的 agent 可执行文件在哪：见 [`locate_binary`]。
pub fn resolve_binary(store: &Store) -> Result<PathBuf> {
    locate_binary(store).map(|(path, _)| path)
}

/// 控制机是哪台：`settings.agent_server_id`。
pub fn agent_server_id(settings: &Settings) -> Result<String> {
    let id = settings.agent_server_id.trim();
    if id.is_empty() {
        return Err(CoreError::config("还没有指定控制机，请先在控制机页面选择一台服务器"));
    }
    Ok(id.to_string())
}

/// 从整份配置里挑出 agent 需要的那部分。
///
/// 过去的是「执行备份所需的最小集合」：服务器（含 SSH 凭据）、备份目标、
/// 执行位为远端的备份/容器配置，以及与定时相关的那几项设置。
/// 仓库、部署配置、Pages 配置、各类 Token、主密码一概不过去。
///
/// `agent_offset_minutes` 是控制机相对 UTC 的偏移（分钟，东为正），读不到时传 None：
/// 那样就原样下发，等于按控制机的时区解释用户写在本机时区里的时间点，
/// 而 [`bundle_warnings`] 会把「没折算」这件事讲出来 —— 静默地把备份挪到业务高峰是不能接受的。
pub fn build_bundle(config: &AppConfig, agent_offset_minutes: Option<i32>) -> AppConfig {
    let mut bundle = AppConfig::default();
    bundle.servers = config.servers.clone();
    bundle.backup_targets = config.backup_targets.clone();
    bundle.backup_configs = config
        .backup_configs
        .iter()
        .filter(|item| item.run_location.is_remote())
        .cloned()
        .collect();
    bundle.container_configs = config
        .container_configs
        .iter()
        .filter(|item| item.run_location.is_remote())
        .cloned()
        .collect();

    let local = &config.settings;
    let mut settings = Settings::default();
    settings.connect_timeout_secs = local.connect_timeout_secs;
    settings.script_timeout_secs = local.script_timeout_secs;
    settings.backup_timeout_secs = local.backup_timeout_secs;
    settings.container_timeout_secs = local.container_timeout_secs;
    settings.backup_history_limit = local.backup_history_limit;
    settings.container_history_limit = local.container_history_limit;
    // 本机默认不留导出包（`db_bundle_keep` = 0，与加这个功能之前一致），但控制机必须留：
    // 不留就没有可恢复的产物，「恢复到另一台」和轮转都无从谈起。0 在这里的含义是
    // 「别烦我本机」，不是「控制机也别留」。
    settings.db_bundle_keep = if local.db_bundle_keep == 0 {
        AGENT_DB_BUNDLE_KEEP
    } else {
        local.db_bundle_keep
    };
    settings.container_bundle_keep = local.container_bundle_keep;
    settings.supabase_url = local.supabase_url.clone();
    settings.default_backup_target_id = local.default_backup_target_id.clone();
    settings.scheduled_container_enabled = local.scheduled_container_enabled;
    settings.scheduled_container_time =
        shift_for_agent(&local.scheduled_container_time, agent_offset_minutes)
            .unwrap_or_else(|| local.scheduled_container_time.clone());
    settings.scheduled_container_config_ids = local
        .scheduled_container_config_ids
        .iter()
        .filter(|id| {
            bundle
                .container_configs
                .iter()
                .any(|item| &item.id == *id && !item.name.trim().is_empty())
        })
        .cloned()
        .collect();
    // 定时数据库备份只在「选中的那条配置是远端执行」时才下发，否则两边都会以为对方负责，
    // 结果是这一晚谁都不跑。
    settings.scheduled_backup_enabled = local.scheduled_backup_enabled
        && local
            .scheduled_backup_config_id
            .as_deref()
            .is_some_and(|id| bundle.backup_configs.iter().any(|item| item.id == id));
    settings.scheduled_backup_config_id = settings
        .scheduled_backup_enabled
        .then(|| local.scheduled_backup_config_id.clone())
        .flatten();
    settings.scheduled_backup_time =
        shift_for_agent(&local.scheduled_backup_time, agent_offset_minutes)
            .unwrap_or_else(|| local.scheduled_backup_time.clone());
    settings.agent_server_id = local.agent_server_id.clone();
    bundle.settings = settings;
    bundle
}

/// 把一个「本机时区的墙上时刻」平移成控制机当地的墙上时刻（按它下一次发生时的本机偏移算）。
///
/// 返回 None 表示没能折算：时间点写法不对，或拿不到控制机偏移。调用方按原样下发。
fn shift_for_agent(value: &str, agent_offset_minutes: Option<i32>) -> Option<String> {
    let agent_offset = agent_offset_minutes?;
    let local_offset = crate::schedule::local_offset_at_next(value, &chrono::Local::now())?;
    crate::schedule::shift_hhmm(value, i64::from(agent_offset - local_offset))
}

/// 时间点被平移过时给用户的说法；没平移（同区、或压根没折算成）就不吭声。
fn shifted_note(label: &str, local: &str, agent: &str) -> Option<String> {
    let local = local.trim();
    let agent = agent.trim();
    (local != agent && !local.is_empty() && !agent.is_empty()).then(|| {
        format!("{label}的 {local}（本机时区）已折算成控制机当地的 {agent}。")
    })
}

/// 下发前的一致性检查：把「跑不起来但不算错误」的情况先讲清楚。
///
/// `agent_offset_minutes` 要传进来是因为它决定时间点有没有被折算（见 [`build_bundle`]）：
/// 折算过、以及想折算却没读到偏移，两种情况都得让用户看见，否则他会以为控制机跑的是自己设的那个点。
pub fn bundle_warnings(
    config: &AppConfig,
    bundle: &AppConfig,
    agent_offset_minutes: Option<i32>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let local = &config.settings;
    if local.scheduled_backup_enabled && !bundle.settings.scheduled_backup_enabled {
        if let Some(id) = local.scheduled_backup_config_id.as_deref() {
            let name = config
                .backup_configs
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.name.clone())
                .unwrap_or_else(|| id.to_string());
            warnings.push(format!(
                "定时备份选的是「{name}」，它的执行位是本机，控制机不会接手这项定时。"
            ));
        }
    }
    let skipped = local
        .scheduled_container_config_ids
        .iter()
        .filter(|id| {
            !bundle
                .settings
                .scheduled_container_config_ids
                .iter()
                .any(|keep| keep == *id)
        })
        .count();
    if local.scheduled_container_enabled && skipped > 0 {
        warnings.push(format!(
            "容器定时里有 {skipped} 条配置的执行位是本机，控制机不会接手它们。"
        ));
    }
    if bundle.backup_configs.is_empty() && bundle.container_configs.is_empty() {
        warnings.push("当前没有任何配置的执行位是「控制机」，下发过去只有服务器凭据，不会自动跑备份。".to_string());
    }
    // 交给控制机的配置必须自带目标：`resolve_backup` 不许它兜底到服务器绑定 / 全局默认（那等于换库，
    // 见 backup.rs），所以这种配置到点必失败。下发时就点名，别让用户从当晚的失败记录里倒推。
    let ownerless: Vec<String> = bundle
        .backup_configs
        .iter()
        .filter(|item| {
            let has_target = item
                .target_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let has_url = item
                .supabase_url
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            !(has_target || has_url)
        })
        .map(|item| item.name.clone())
        .collect();
    if !ownerless.is_empty() {
        warnings.push(format!(
            "控制机上的备份配置「{}」没有自己的备份目标，到点会失败（不会退到服务器绑定或全局默认，那等于换库）。请在数据库备份页为它选一个目标再下发。",
            ownerless.join("」「")
        ));
    }
    // 时间点折算过就要说出来：控制机的钟和本机的钟不一样时，用户设的 03:00 会落在别的时刻。
    if let Some(note) = shifted_note(
        "定时数据库备份",
        &local.scheduled_backup_time,
        &bundle.settings.scheduled_backup_time,
    ) {
        warnings.push(note);
    }
    if let Some(note) = shifted_note(
        "定时容器备份",
        &local.scheduled_container_time,
        &bundle.settings.scheduled_container_time,
    ) {
        warnings.push(note);
    }
    if agent_offset_minutes.is_none()
        && (bundle.settings.scheduled_backup_enabled || bundle.settings.scheduled_container_enabled)
    {
        // 只有一种情况需要提醒：读不到控制机时区，于是两个时间点都是照原样发过去的。
        warnings.push("没读到控制机的时区，定时时间按控制机当地解释；若两机时区不同，实际时刻会与设置里不同。".to_string());
    }
    warnings
}

/// 协议对不上就拒绝任何下发（装的是哪一份都得先过这一关）。
///
/// 下发过去的 config.json 是按当前语义写的：旧协议那份读它，字段含义可能已经改过，
/// 而 `remember_sync` 一记上本机就撒手 —— 到点跑出一份谁也没预期的东西。
/// 握手那两条命令（`sync` / `status`）都过这里，`install` 也必须过：它紧接着自己 push_bundle。
fn check_proto(remote: &AgentVersion) -> Result<()> {
    let local = local_version();
    if remote.proto != local.proto {
        return Err(CoreError::config(version_mismatch(&local, remote)));
    }
    Ok(())
}

/// 管理端：所有动作都以「一次 SSH 会话」为单位，用完即断。
pub struct AgentControl {
    store: Arc<Store>,
}

impl AgentControl {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    fn settings(&self) -> Result<Settings> {
        Ok(self.store.load_config()?.settings)
    }

    /// 这次操作对哪台服务器：显式传进来的优先，否则取设置里记着的那台控制机。
    fn resolve_server_id(&self, server_id: Option<&str>) -> Result<String> {
        let explicit = server_id.unwrap_or("").trim().to_string();
        if !explicit.is_empty() {
            return Ok(explicit);
        }
        agent_server_id(&self.settings()?)
    }

    /// 连到控制机。`server_id` 为空时取设置里指定的那台。
    async fn connect(&self, server_id: Option<&str>) -> Result<SshClient> {
        let config = self.store.load_config()?;
        let key = self.resolve_server_id(server_id)?;
        let server = Store::find_server(&config, &key)?.clone();
        SshClient::connect(&server, config.settings.connect_timeout_secs).await
    }

    /// 执行一条命令并把 stdout 当成一个 JSON 文档解析。
    ///
    /// 走 `exec_stream` 而不是 `exec_capture`：后者会把 stderr 行冠上 `[stderr]` 再拼进同一段
    /// 文本，那样就没法直接喂给 serde_json。
    async fn exec_json<T, F>(
        client: &SshClient,
        command: &str,
        timeout: u64,
        action: &str,
        parse: F,
    ) -> Result<T>
    where
        F: FnOnce(&str) -> std::result::Result<T, String>,
    {
        let mut stdout = String::new();
        let mut stderr = String::new();
        let code = client
            .exec_stream(command, timeout, &mut |kind, line| {
                match kind {
                    OutputKind::Stdout => {
                        stdout.push_str(&line);
                        stdout.push('\n');
                    }
                    OutputKind::Stderr => {
                        stderr.push_str(&line);
                        stderr.push('\n');
                    }
                }
            })
            .await?;
        if code != 0 {
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            return Err(CoreError::ssh(format!(
                "{action}失败（退出码 {code}）: {}",
                if detail.is_empty() {
                    "远端没有输出可读信息".to_string()
                } else {
                    detail.to_string()
                }
            )));
        }
        parse(stdout.trim()).map_err(|err| {
            CoreError::ssh(format!(
                "{action}失败：控制机返回的内容无法解析（{err}）。请确认 agent 版本与客户端匹配。"
            ))
        })
    }

    /// 读 agent 版本。没装的时候给一条能看懂的话，而不是 `command not found`。
    pub async fn fetch_version(&self, client: &SshClient) -> Result<AgentVersion> {
        let mut stdout = String::new();
        let code = client
            .exec_stream(
                &format!("{BIN_PATH} --version 2>&1 || echo NOT_INSTALLED"),
                60,
                &mut |_, line| {
                    stdout.push_str(&line);
                    stdout.push('\n');
                },
            )
            .await?;
        if code != 0 {
            return Err(CoreError::ssh(format!(
                "读取 agent 版本失败（退出码 {code}）: {}",
                stdout.trim()
            )));
        }
        parse_version(&stdout).ok_or_else(|| {
            CoreError::config(format!(
                "这台服务器上还没有安装 {SERVICE}（或版本过旧认不出协议行）。请先点「安装/更新」。"
            ))
        })
    }

    /// 握手：远端协议必须与本机一致，否则拒绝后续任何下发。
    async fn handshake(&self, client: &SshClient) -> Result<AgentVersion> {
        let remote = self.fetch_version(client).await?;
        check_proto(&remote)?;
        Ok(remote)
    }

    /// 安装或更新：上传二进制 + 写 unit + 起服务 + 下发一次配置。
    pub async fn install(&self, server_id: Option<&str>) -> Result<AgentStatus> {
        let binary = resolve_binary(&self.store)?;
        let server_id = self.resolve_server_id(server_id)?;
        let client = self.connect(Some(&server_id)).await?;
        let result = async {
            // 先握手再看版本：装了旧协议 agent 的机器，覆盖二进制之前就该提醒用户，
            // 但覆盖本身是允许的（新二进制正是用户想装上去的东西），所以这里只记录版本。
            let tmp = format!("/tmp/deploy-agent-{}.bin", uuid::Uuid::new_v4());
            client
                .upload_file(&binary, &tmp, &mut |_, _| {})
                .await
                .map_err(|err| {
                    CoreError::ssh(format!(
                        "上传 agent 可执行文件失败（{}，{}）: {err}",
                        binary.display(),
                        human_size(
                            std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0)
                        )
                    ))
                })?;
            let command = install_script(&tmp, &unit_template());
            let version = Self::exec_json(
                &client,
                &command,
                300,
                "安装 agent",
                |text| parse_version(text).ok_or_else(|| "控制机没有打印版本行".to_string()),
            )
            .await?;
            let _ = client
                .exec_capture(&format!("rm -f {}", shell_quote(&tmp)), 30)
                .await;
            // 覆盖安装本身是允许的（用户要的就是这份新二进制），但下发不行：
            // 手放在 `<数据目录>/agent/` 的旧协议产物，会走这条路收到新语义的配置。
            check_proto(&version)?;
            let report = self.push_bundle(&client, &server_id).await?;
            Ok::<AgentStatus, CoreError>(AgentStatus {
                version: version.version,
                ..report.status
            })
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 装好并设为控制机，同时把原来那台收回来。
    ///
    /// 顺序是有讲究的：先把 `agent_server_id` 换成新机、再对旧机下发卸载。因为
    /// `Store::forget_agent_server` 只肯动「当前那台」（防止顺手清掉现役指纹），换过去之后
    /// 卸旧机就不会把新机刚记下的指纹和执行位一起收掉；而旧机上那份 agent 读的是它自己
    /// 盘上的 config.json，没人叫停的话它照旧每晚执行，上面还挂着全部源机的明文口令。
    /// 收回失败（旧机重装过、口令改了）不拦这次切换 —— 拦了用户就卡在两台之间，
    /// 改成记一笔待收回、界面红在那里，直到手动收回成功。
    pub async fn promote(&self, server_id: &str) -> Result<AgentStatus> {
        let previous = self
            .store
            .load_config()?
            .settings
            .agent_server_id
            .trim()
            .to_string();
        let status = self.install(Some(server_id)).await?;
        self.store.mutate_config(|config| {
            config.settings.agent_server_id = server_id.to_string();
            Ok(())
        })?;
        if !previous.is_empty() && previous != server_id {
            if let Err(err) = self.uninstall(Some(&previous)).await {
                self.store.remember_orphan_agent(&previous)?;
                // 说清下一步做什么：那台机器现在既不在我们名下、又还在自行执行。
                return Err(CoreError::ssh(format!(
                    "已把控制机换成这台，但旧控制机上的 agent 没能收回（{err}）。已记下待办，请在控制机页面把它收回，或先解除那台机器的 agent 服务。"
                )));
            }
        }
        Ok(status)
    }

    /// 卸载：只停服务、删 unit 与二进制，备份包与记录留在 `/var/lib/deploycode`。
    ///
    /// 卸的是设置里那台控制机时，本机这份也要一起收摊：把「执行位 = 控制机」的配置退回本机、
    /// 清掉同步指纹。否则定时循环会照旧让位，而那些配置在控制机上已经不存在了 ——
    /// 用户看到的就是「设了定时，那一晚什么都没跑」。
    pub async fn uninstall(&self, server_id: Option<&str>) -> Result<String> {
        let server_id = self.resolve_server_id(server_id)?;
        let client = self.connect(Some(&server_id)).await?;
        let result = async {
            let mut out = String::new();
            let code = client
                .exec_stream(&uninstall_script(), 120, &mut |_, line| {
                    out.push_str(line.trim());
                    out.push(' ');
                })
                .await?;
            if code != 0 {
                return Err(CoreError::ssh(format!(
                    "卸载 agent 失败（退出码 {code}）: {}",
                    out.trim()
                )));
            }
            let text = out.trim().to_string();
            // forget_agent → Store::forget_agent_server 里会顺手撤掉这台的「待收回」记录。
            let moved = self.forget_agent(&server_id)?;
            // 条数只有本机侧关心（CLI 会原样打印出来）：不说的话，用户以为退回本机那部分还在跑。
            Ok(if moved > 0 {
                format!("{text}（{moved} 条配置的执行位已退回本机）")
            } else {
                text
            })
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 卸掉控制机之后收回本机这一侧的状态，返回被改回本机的配置条数。
    fn forget_agent(&self, server_id: &str) -> Result<usize> {
        // 与「删除这台服务器」共用 `Store::forget_agent_server`，两条路口径不能分叉。
        self.store.forget_agent_server(server_id)
    }

    /// 下发配置（注入其余服务器的凭据与远端执行的备份配置）。
    ///
    /// agent 每个轮询都重读 `config.json`，所以下发之后**不需要重启服务**：
    /// 下一个 tick（20 秒内）就按新配置走。
    pub async fn sync(&self, server_id: Option<&str>) -> Result<AgentSyncReport> {
        let server_id = self.resolve_server_id(server_id)?;
        let client = self.connect(Some(&server_id)).await?;
        let result = async {
            self.handshake(&client).await?;
            self.push_bundle(&client, &server_id).await
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 写 bundle 到控制机并回读一次状态。上传走 SFTP + `mv`：
    /// 直接覆盖正在被 agent 读的那份文件会读到半截 JSON。
    ///
    /// `server_id` 是这次下发对的那台服务器，写成功之后按它记同步指纹（见 [`AgentSyncState`]）。
    async fn push_bundle(&self, client: &SshClient, server_id: &str) -> Result<AgentSyncReport> {
        let config = self.store.load_config()?;
        // 折算定时时间要用的偏移必须在**写之前**拿到：那份 HH:MM 落到 config.json 里就得已经是
        // 控制机当地的时刻。用 `date +%z` 而不是 `deploy-agent status`：刚装完的机器上还没有
        // config.json，status 会失败，而时区是操作系统属性，跟 agent 有没有配置无关。
        let agent_offset = Self::remote_utc_offset(client).await;
        let bundle = build_bundle(&config, agent_offset);
        let warnings = bundle_warnings(&config, &bundle, agent_offset);
        let json = serde_json::to_string_pretty(&bundle)?;

        let tmp = client
            .exec_capture("mktemp /tmp/deploycode-bundle.XXXXXX", 30)
            .await
            .map_err(|err| CoreError::ssh(format!("在控制机上建临时文件失败: {err}")))?;
        let (code, out) = tmp;
        let remote_tmp = out.trim().to_string();
        if code != 0 || remote_tmp.is_empty() {
            return Err(CoreError::ssh("在控制机上建临时文件失败"));
        }
        let local_tmp = self.store.prepare_temp_file(&format!(
            "agent-bundle-{}.json",
            uuid::Uuid::new_v4()
        ))?;
        let result = async {
            std::fs::write(&local_tmp, json.as_bytes()).map_err(|e| CoreError::io_path(&local_tmp, e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&local_tmp, std::fs::Permissions::from_mode(0o600));
            }
            client
                .upload_file(&local_tmp, &remote_tmp, &mut |_, _| {})
                .await
                .map_err(|err| CoreError::ssh(format!("上传配置失败: {err}")))?;
            let command = config_install_script(&remote_tmp);
            let (code, out) = client.exec_capture(&command, 60).await?;
            if code != 0 {
                return Err(CoreError::ssh(format!(
                    "写入控制机配置失败: {}",
                    out.trim()
                )));
            }
            // 安装脚本返回 0 就是「控制机上那份已经存在」：指纹在这一刻记下才准。
            // 排在状态回读之后的话，status 一失败本机就不认这次下发（判成「未下发」而拒绝定时），
            // 可控制机当晚照跑 —— 用户看到的是一句和事实相反的报错。
            self.remember_sync(server_id, &bundle, &config)?;
            let status = self.status_via(client).await.map_err(|err| {
                CoreError::ssh(format!(
                    "配置已下发到控制机，但读不到它的状态（{err}）；本机按已下发处理，定时交给控制机执行"
                ))
            })?;
            Ok::<AgentSyncReport, CoreError>(AgentSyncReport {
                servers: bundle.servers.len(),
                backup_configs: bundle.backup_configs.len(),
                container_configs: bundle.container_configs.len(),
                warnings,
                status,
            })
        }
        .await;
        std::fs::remove_file(&local_tmp).ok();
        let _ = client
            .exec_capture(
                &format!("rm -f {}", shell_quote(&remote_tmp)),
                30,
            )
            .await;
        result
    }

    /// 读控制机相对 UTC 的偏移（分钟，东为正）：`date +%z` 输出 `+0800` / `-0330` 这种形态。
    ///
    /// 读不到就返回 None（远端没有 `date`、或输出被 shell 配置污染），调用方按「不折算」下发，
    /// 并由 [`bundle_warnings`] 把这件事讲给用户。时间点差几小时是看得见的，静默猜一个才是问题。
    async fn remote_utc_offset(client: &SshClient) -> Option<i32> {
        let (code, out) = client.exec_capture("date +%z", 30).await.ok()?;
        if code != 0 {
            return None;
        }
        parse_utc_offset(&out)
    }

    /// 记下「控制机上现在有这些配置」，供定时循环判断该不该让位。
    ///
    /// 定时口径按**本机当前值**记（不是 bundle 里折算过的那份）：比较的对象是用户界面上
    /// 看到的 HH:MM 与勾选，折算只影响控制机何时执行，不影响「改没改过」。
    fn remember_sync(&self, server_id: &str, bundle: &AppConfig, config: &AppConfig) -> Result<()> {
        let mut state = AgentSyncState {
            server_id: server_id.to_string(),
            synced_at: crate::models::now_string(),
            backup_config_ids: bundle.backup_configs.iter().map(|item| item.id.clone()).collect(),
            container_config_ids: bundle
                .container_configs
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            backup_schedule: backup_schedule_key(config),
            container_schedule: container_schedule_key(config),
            ..Default::default()
        };
        // 待收回那份不是指纹：重新下发不该顺手把「旧那台还没收回 agent」这件事清掉。
        state.orphan_server_ids = self.store.load_agent_sync().orphan_server_ids;
        self.store.save_agent_sync(&state)
    }

    /// 回读状态（含握手校验）。
    pub async fn status(&self, server_id: Option<&str>) -> Result<AgentStatus> {
        let client = self.connect(server_id).await?;
        let result = async {
            self.handshake(&client).await?;
            self.status_via(&client).await
        }
        .await;
        client.disconnect().await;
        result
    }

    async fn status_via(&self, client: &SshClient) -> Result<AgentStatus> {
        let command = format!("{BIN_PATH} status --data-dir {DATA_DIR}");
        let mut status: AgentStatus =
            Self::exec_json(client, &command, 90, "读取控制机状态", |text| {
                serde_json::from_str(text).map_err(|err| err.to_string())
            })
            .await?;
        // 上面那份是「这份 exec 出来的进程怎么描述自己」，回答不了「到点有没有人跑」。
        // 常驻服务活着与否只能问 systemd —— 而且由客户端来问，老版本 agent 二进制不重装也报得准。
        status.service_active = service_state(client).await;
        Ok(status)
    }

    /// 回读 agent 自身的运行日志（journald）。
    pub async fn logs(&self, lines: usize, server_id: Option<&str>) -> Result<Vec<String>> {
        let client = self.connect(server_id).await?;
        let result = async {
            let limit = lines.clamp(1, 2000);
            let command = format!(
                "journalctl -o cat -n {limit} --no-pager -u {service} 2>&1 || true",
                service = SERVICE
            );
            let mut out = Vec::new();
            let code = client
                .exec_stream(&command, 60, &mut |kind, line| {
                    if kind == OutputKind::Stdout && !line.trim().is_empty() {
                        out.push(line);
                    }
                })
                .await?;
            if code != 0 && out.is_empty() {
                return Err(CoreError::ssh("读取控制机日志失败（这台机器上可能没有 journald）"));
            }
            Ok(out)
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 回读控制机上的数据库备份记录。
    pub async fn backup_records(
        &self,
        limit: usize,
        server_id: Option<&str>,
    ) -> Result<Vec<BackupRecord>> {
        let client = self.connect(server_id).await?;
        let result = async {
            let command = format!(
                "{BIN_PATH} records --data-dir {DATA_DIR} --kind backup --limit {limit}",
                limit = limit.clamp(1, 1000)
            );
            Self::exec_json(&client, &command, 90, "读取控制机备份记录", |text| {
                serde_json::from_str(text).map_err(|err| err.to_string())
            })
            .await
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 回读控制机上的容器备份记录。
    pub async fn container_records(
        &self,
        limit: usize,
        server_id: Option<&str>,
    ) -> Result<Vec<ContainerRecord>> {
        let client = self.connect(server_id).await?;
        let result = async {
            let command = format!(
                "{BIN_PATH} records --data-dir {DATA_DIR} --kind container --limit {limit}",
                limit = limit.clamp(1, 1000)
            );
            Self::exec_json(&client, &command, 90, "读取控制机容器备份记录", |text| {
                serde_json::from_str(text).map_err(|err| err.to_string())
            })
            .await
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 让控制机立刻跑一条数据库备份配置，事件逐条回调给调用方。
    pub async fn run_backup_config(
        &self,
        config_id: &str,
        on_event: &mut (dyn FnMut(BackupEvent) + Send),
        server_id: Option<&str>,
    ) -> Result<()> {
        let client = self.connect(server_id).await?;
        let argument = format!("--config {}", shell_quote(config_id));
        let result = self
            .stream_events(&client, "trigger", false, &argument, |line| {
                serde_json::from_str(line).ok()
            }, on_event)
            .await;
        client.disconnect().await;
        result
    }

    /// 让控制机立刻跑一条容器备份配置。
    pub async fn run_container_config(
        &self,
        config_id: &str,
        on_event: &mut (dyn FnMut(ContainerEvent) + Send),
        server_id: Option<&str>,
    ) -> Result<()> {
        let client = self.connect(server_id).await?;
        let argument = format!("--config {}", shell_quote(config_id));
        let result = self
            .stream_events(&client, "trigger", true, &argument, |line| {
                serde_json::from_str(line).ok()
            }, on_event)
            .await;
        client.disconnect().await;
        result
    }

    /// 用控制机上已有的备份包恢复到另一台服务器（包在控制机上，所以由它出面搬运）。
    pub async fn restore_container_bundle(
        &self,
        record_id: &str,
        target_server_id: &str,
        target_dir: &str,
        start_services: bool,
        on_event: &mut (dyn FnMut(ContainerEvent) + Send),
        server_id: Option<&str>,
    ) -> Result<()> {
        let client = self.connect(server_id).await?;
        let argument = format!(
            "--record {} --target {} --dir {} --start {}",
            shell_quote(record_id),
            shell_quote(target_server_id),
            shell_quote(target_dir),
            if start_services { "1" } else { "0" }
        );
        let result = self
            .stream_events(&client, "restore", true, &argument, |line| {
                serde_json::from_str(line).ok()
            }, on_event)
            .await;
        client.disconnect().await;
        result
    }

    /// 起一个 `deploy-agent` 子命令并把它的 stdout 当成事件流逐条回调。
    ///
    /// 约定：agent 在 trigger / restore 模式下**只往 stdout 打 JSON 行**，人读的错误走 stderr +
    /// 非零退出码，所以这里能安静地把每一行喂给 serde_json，而不必挑掉杂项输出。
    async fn stream_events<E, P>(
        &self,
        client: &SshClient,
        verb: &str,
        container: bool,
        argument: &str,
        parse: P,
        on_event: &mut (dyn FnMut(E) + Send),
    ) -> Result<()>
    where
        P: Fn(&str) -> Option<E> + Sync,
    {
        let settings = self.settings()?;
        // 单次任务的上限取对应业务的超时设置，再留 5 分钟余量：agent 侧还有连接、打包、
        // 下载这些不在超时内的开销。
        let base = if container {
            settings.container_timeout_secs
        } else {
            settings.backup_timeout_secs
        };
        let timeout = base.max(300) + 300;
        let command = format!(
            "{bin} {verb} --kind {kind} {argument} --data-dir {dir}",
            bin = BIN_PATH,
            kind = if container { "container" } else { "backup" },
            verb = verb,
            argument = argument,
            dir = DATA_DIR
        );
        let mut stderr = String::new();
        let mut got_event = false;
        let code = client
            .exec_stream(&command, timeout, &mut |kind, line| {
                match kind {
                    OutputKind::Stdout => {
                        if let Some(event) = parse(line.trim()) {
                            got_event = true;
                            on_event(event);
                        }
                    }
                    OutputKind::Stderr => {
                        stderr.push_str(&line);
                        stderr.push('\n');
                    }
                }
            })
            .await?;
        if code != 0 {
            let detail = stderr.trim();
            let message = if detail.is_empty() {
                if got_event {
                    "任务中断，详见控制机日志".to_string()
                } else {
                    "控制机没有接下这次执行，详见控制机日志".to_string()
                }
            } else {
                detail.to_string()
            };
            // 退出码 3 是 agent 说的「名额被占」（deploy-agent main.rs 把 Busy 映射成 3）。
            // 必须还原成 CoreError::Busy，调用方才能按项目红线用 matches! 分辨「稍后重试」与
            // 真失败；压成 ssh 错就等于把这条红线改成了「读文案」。
            return Err(if code == 3 {
                CoreError::busy(message)
            } else {
                CoreError::ssh(format!("控制机执行失败（退出码 {code}）: {message}"))
            });
        }
        if !got_event {
            return Err(CoreError::ssh("控制机没有回传任何事件，请检查 agent 日志"));
        }
        Ok(())
    }
}

/// 让 `records` 与 `status` 的输出解析更直白：只接受一个 JSON 值。
///
/// `exec_json` 的 parse 闭包签名要求返回 `Result<T, serde_json::Error>`，而事件流那边用
/// `Option`（一行解析失败就跳过，不能让杂项输出打断整场任务）。这里留一个 seed 用的类型
/// 别名，避免调用点写满泛型。
#[allow(dead_code)]
/// 磁盘水位摘要（`status` 里那三个字段的来源，agent 与客户端共用这一份计算）。
pub fn disk_summary(dir: &Path) -> Result<(u64, u64, u64)> {
    match disk::check(dir)? {
        Some(head) => Ok((head.total_bytes, head.free_bytes, head.floor_bytes)),
        None => Ok((0, 0, 0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        BackupConfig, BackupTarget, ContainerConfig, ContainerTarget, DbBackupSource, DeployStatus,
        RunLocation, ServerConfig, SshAuth,
    };

    #[test]
    fn service_state_only_answers_known_systemd_words() {
        assert_eq!(parse_service_state("active\n"), Some(true));
        assert_eq!(parse_service_state("  activating  "), Some(true));
        // StartLimitBurst 打死之后停在这里 —— 这正是原来界面上看不见的那个状态。
        assert_eq!(parse_service_state("failed\n"), Some(false));
        assert_eq!(parse_service_state("inactive"), Some(false));
        // 没有 systemctl 的机器什么都不印；shell 欢迎语与 [stderr] 行都不该被读成「没跑」。
        assert_eq!(parse_service_state(""), None);
        assert_eq!(parse_service_state("Welcome to Ubuntu 24.04\n"), None);
        assert_eq!(parse_service_state("[stderr] Failed to connect to bus\n"), None);
    }

    /// 这一格是客户端填的，所以 agent 那份 JSON 里**没有**它也必须解得开（老二进制）。
    #[test]
    fn status_without_service_active_field_still_parses() {
        let json = r#"{"proto":1,"version":"0.1.4","timezone":"+0800","localTime":"2026-10-04 03:00:00",
            "dataDir":"/var/lib/deploycode","totalBytes":1,"freeBytes":1,"floorBytes":1,
            "bundleBytes":0,"bundleCount":0,"servers":1,"backupConfigs":1,"containerConfigs":0,
            "backupEnabled":true,"backupTime":"03:00","backupConfigName":"zhu","backupLastRun":"",
            "containerEnabled":false,"containerTime":"","containerQueue":0,"containerLastRun":""}"#;
        let status: AgentStatus = serde_json::from_str(json).expect("老 agent 的输出该照样解得开");
        assert_eq!(status.service_active, None);
    }

    fn server(id: &str, name: &str) -> ServerConfig {
        let mut item = ServerConfig::new(
            name.to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: "secret".to_string(),
            },
        );
        item.id = id.to_string();
        item
    }

    fn backup_config(id: &str, name: &str, remote: bool) -> BackupConfig {
        BackupConfig {
            id: id.to_string(),
            name: name.to_string(),
            server_id: "s1".to_string(),
            source: DbBackupSource::default(),
            target_id: None,
            supabase_url: None,
            run_location: if remote {
                RunLocation::Remote
            } else {
                RunLocation::Local
            },
        }
    }

    fn container_config(id: &str, name: &str, remote: bool) -> ContainerConfig {
        ContainerConfig {
            id: id.to_string(),
            name: name.to_string(),
            server_id: "s1".to_string(),
            project: name.to_string(),
            pause_source: false,
            include_volumes: true,
            include_images: true,
            target: Some(ContainerTarget {
                server_id: "s2".to_string(),
                target_dir: "/srv/app".to_string(),
                start_services: true,
            }),
            created_at: String::new(),
            run_location: if remote {
                RunLocation::Remote
            } else {
                RunLocation::Local
            },
        }
    }

    #[test]
    fn version_line_round_trips_and_survives_noise() {
        let version = local_version();
        assert_eq!(parse_version(&version.line()).as_ref(), Some(&version));
        // 登录 shell 的欢迎语与 CRLF 都不能让解析失败。
        let noisy = format!("Welcome!\r\n{}\r\n", version.line());
        assert_eq!(parse_version(&noisy).as_ref(), Some(&version));
    }

    #[test]
    fn version_parse_rejects_foreign_output() {
        assert!(parse_version("").is_none());
        assert!(parse_version("deploy-agent 1.0").is_none());
        assert!(parse_version("bash: deploy-agent: command not found").is_none());
        // 版本号在、proto 缺失或非法：按未安装处理，宁可不下发。
        assert!(parse_version("deploy-agent 1.0 proto=abc").is_none());
        assert!(parse_version("other-agent 1.0 proto=1").is_none());
        // 这句以服务名开头，但根本不是版本行：不能误认成 agent 已安装。
        assert!(parse_version("deploy-agent: command not found").is_none());
        assert!(parse_version("deploy-agent 0.1.2 proto=1 extra").is_some());
    }

    #[test]
    fn unit_template_pins_paths_and_limits() {
        let unit = unit_template();
        assert!(unit.contains(&format!("ExecStart={BIN_PATH} run --data-dir {DATA_DIR}")));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("MemoryMax=1G"));
        assert!(unit.contains("CPUQuota=200%"));
        assert!(unit.contains("SyslogIdentifier=deploy-agent"));
        // 服务不该监听任何端口。
        assert!(!unit.contains("ExecStart=/bin/sh -c 'nc "));
    }

    #[test]
    fn install_script_writes_unit_and_starts_service() {
        let script = install_script("/tmp/agent.bin", &unit_template());
        assert!(script.contains("install -m 755 /tmp/agent.bin"));
        assert!(script.contains("systemctl enable --now deploy-agent"));
        assert!(script.contains("AGENT_UNIT_EOF"));
        // 数据目录只让 root 进（里面是全服务器的 SSH 口令）。
        assert!(script.contains(&format!("chmod 700 {DATA_DIR}")));
        // 覆盖二进制之前必须先停服务：`enable --now` 不会重启在跑的服务，
        // 那样旧进程会继续按旧代码调度，而脚本末尾的版本行却是新二进制的 —— 假升级成功。
        let stop = script
            .find("systemctl stop")
            .expect("安装脚本要先 systemctl stop");
        let install = script
            .find("install -m 755")
            .expect("安装脚本要落二进制");
        assert!(stop < install, "stop 必须发生在覆盖二进制之前：{script}");
    }

    #[test]
    fn unit_caps_restart_storms_and_avoids_fake_directives() {
        let unit = unit_template();
        // 崩到上限就留在 failed：没有 StartLimit 时 RestartSec=30 就是每半分钟一次的启动风暴。
        assert!(unit.contains("StartLimitBurst="), "{unit}");
        assert!(unit.contains("StartLimitIntervalSec="), "{unit}");
        assert!(unit.contains("[Unit]"), "{unit}");
        // JournalSizeMax 不是 [Service] 的合法指令，写上去只会被静默忽略（比不写更误导）。
        assert!(!unit.contains("JournalSizeMax"), "{unit}");
        assert!(unit.contains("MemoryMax=1G"), "{unit}");
    }

    #[test]
    fn config_push_swaps_the_file_instead_of_truncating_it() {
        let script = config_install_script("/tmp/deploycode-bundle.abc");
        let staging = format!("{DATA_DIR}/config.json.staging");
        // 必须是在数据目录内 rename：/tmp 与 /var/lib 通常不同文件系统，跨设备 mv 会退化成复制。
        assert!(
            script.contains(&format!("cp /tmp/deploycode-bundle.abc {staging}")),
            "{script}"
        );
        assert!(
            script.contains(&format!("mv -f {staging} {DATA_DIR}/config.json")),
            "{script}"
        );
        assert!(script.contains(&format!("chmod 600 {staging}")), "{script}");
        // 任何直接写目标文件的形式都是原地截断，读者会撞上半截 JSON。
        assert!(!script.contains(&format!("> {DATA_DIR}/config.json")), "{script}");
        assert!(
            !script.contains(&format!("install -m 600 {DATA_DIR}/config.json")),
            "{script}"
        );
        // 远端路径照样过 shell_quote：带空格或引号的临时名不能裸拼进命令。
        let quoted = config_install_script("/tmp/it's a bundle");
        assert!(quoted.contains("cp '/tmp/it'\\''s a bundle' "), "{quoted}");
    }

    #[test]
    fn uninstall_keeps_the_data_dir() {
        let script = uninstall_script();
        assert!(script.contains(&format!("rm -f {UNIT_PATH} {BIN_PATH}")));
        // 卸载不删备份包：那是用户的数据。
        assert!(!script.contains(&format!("rm -rf {DATA_DIR}")));
        assert!(script.contains(&format!("echo 'kept:{DATA_DIR}'")));
    }

    #[test]
    fn bundle_keeps_only_remote_configs_and_credentials() {
        let mut config = AppConfig::default();
        config.servers = vec![server("s1", "源机"), server("s2", "目标机")];
        config.backup_configs = vec![
            backup_config("b1", "远端库", true),
            backup_config("b2", "本机库", false),
        ];
        config.container_configs = vec![
            container_config("c1", "远端项目", true),
            container_config("c2", "本机项目", false),
        ];
        config.settings.github_token = "ghp_secret".to_string();
        config.settings.cloudflare_api_token = "cf_secret".to_string();
        config.settings.master_password_hash = Some("hash".to_string());
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_time = "02:00".to_string();
        config.settings.scheduled_backup_config_id = Some("b1".to_string());
        config.settings.scheduled_container_enabled = true;
        config.settings.scheduled_container_config_ids = vec!["c1".to_string(), "c2".to_string()];
        config.settings.scheduled_backup_last_run = "2026-09-27".to_string();

        let bundle = build_bundle(&config, None);
        assert_eq!(bundle.backup_configs.len(), 1);
        assert_eq!(bundle.backup_configs[0].id, "b1");
        assert_eq!(bundle.container_configs.len(), 1);
        assert_eq!(bundle.container_configs[0].id, "c1");
        // 定时队列里剔掉本机执行的那条，否则控制机会替它再跑一遍。
        assert_eq!(bundle.settings.scheduled_container_config_ids, vec!["c1".to_string()]);
        assert!(bundle.settings.scheduled_backup_enabled);
        // 拿不到控制机时区 → 原样下发（bundle_warnings 会说明这一点）。
        assert_eq!(bundle.settings.scheduled_backup_time, "02:00");
        // 本机那份是不留包的（0），但控制机必须留，否则远端执行没有产物。
        assert_eq!(bundle.settings.db_bundle_keep, AGENT_DB_BUNDLE_KEEP);
        // 凭据下发只覆盖备份要用的部分。
        assert_eq!(bundle.servers.len(), 2);
        assert!(bundle.settings.github_token.is_empty());
        assert!(bundle.settings.cloudflare_api_token.is_empty());
        assert!(bundle.settings.master_password_hash.is_none());
        // agent 自己的触发日期不能被顶掉（它记在 schedule-state.json）。
        assert!(bundle.settings.scheduled_backup_last_run.is_empty());
        assert!(bundle.repos.is_empty());
        assert!(bundle.deploy_configs.is_empty());
    }

    #[test]
    fn bundle_drops_backup_schedule_pointing_at_a_local_config() {
        let mut config = AppConfig::default();
        config.backup_configs = vec![backup_config("b2", "本机库", false)];
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_config_id = Some("b2".to_string());
        let bundle = build_bundle(&config, None);
        assert!(!bundle.settings.scheduled_backup_enabled);
        assert!(bundle.settings.scheduled_backup_config_id.is_none());

        let warnings = bundle_warnings(&config, &bundle, None);
        assert!(
            warnings.iter().any(|item| item.contains("本机库")),
            "{warnings:?}"
        );
    }

    /// 执行位交给控制机、却没给自己留备份目标的配置：控制机到点必失败（不许兜底换库），
    /// 所以下发那一刻就要点名，而不是让用户从当晚的失败记录里倒推。
    #[test]
    fn bundle_warns_about_remote_configs_without_a_target() {
        let mut config = AppConfig::default();
        let mut with_target = backup_config("b1", "带目标", true);
        with_target.target_id = Some("t1".to_string());
        let mut with_url = backup_config("b2", "带连接串", true);
        with_url.supabase_url = Some("postgres://u:p@h/db".to_string());
        config.backup_configs = vec![with_target, with_url, backup_config("b3", "裸配置", true)];
        config.backup_targets = vec![BackupTarget::new(
            "目标库".to_string(),
            "postgres://u:p@h/db".to_string(),
        )];

        let warnings = bundle_warnings(&config, &build_bundle(&config, None), None);
        let flagged: Vec<&String> = warnings
            .iter()
            .filter(|item| item.contains("没有自己的备份目标"))
            .collect();
        assert_eq!(flagged.len(), 1, "{warnings:?}");
        // 只点名缺目标的那条：另两条各有自己的来源，不该被牵连。
        assert!(
            flagged[0].contains("裸配置")
                && !flagged[0].contains("带目标")
                && !flagged[0].contains("带连接串"),
            "{flagged:?}"
        );
    }

    #[test]
    fn bundle_keeps_the_users_own_db_bundle_keep_when_set() {
        let mut config = AppConfig::default();
        config.settings.db_bundle_keep = 5;
        let bundle = build_bundle(&config, None);
        // 兜底只兜 0：用户显式设过的份数要照发过去。
        assert_eq!(bundle.settings.db_bundle_keep, 5);
    }

    #[test]
    fn bundle_shifts_schedule_to_the_agents_wall_clock() {
        let mut config = AppConfig::default();
        config.backup_configs = vec![backup_config("b1", "夜间库", true)];
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_config_id = Some("b1".to_string());
        config.settings.scheduled_backup_time = "03:00".to_string();

        // 本机偏移是环境相关的（测试跑在哪台机器上不知道），所以断言差值而不是绝对值：
        // 控制机比本机东边 5 小时，控制机那份时间点就该写成 08:00（同一瞬间）。
        let local = crate::schedule::local_offset_at_next("03:00", &chrono::Local::now()).unwrap();
        let bundle = build_bundle(&config, Some(local + 5 * 60));
        assert_eq!(bundle.settings.scheduled_backup_time, "08:00");
        // 同区时不平移。
        let same = build_bundle(&config, Some(local));
        assert_eq!(same.settings.scheduled_backup_time, "03:00");
        // 折算过就要在同步结果里说清楚，不能让用户以为控制机跑的是 03:00。
        let warnings = bundle_warnings(&config, &bundle, Some(local + 5 * 60));
        assert!(
            warnings.iter().any(|item| item.contains("03:00") && item.contains("08:00")),
            "{warnings:?}"
        );
        // 读不到偏移时也要说：那时候时间点没动，但两机可能并不同时区。
        let blind = bundle_warnings(&config, &build_bundle(&config, None), None);
        assert!(blind.iter().any(|item| item.contains("时区")), "{blind:?}");
    }

    #[test]
    fn parse_utc_offset_reads_date_plus_z() {
        assert_eq!(parse_utc_offset("+0800\n"), Some(480));
        assert_eq!(parse_utc_offset("-0330"), Some(-210));
        assert_eq!(parse_utc_offset("+08:00"), Some(480));
        assert_eq!(parse_utc_offset("+8"), Some(480));
        // 登录 shell 的杂项输出前面有别的正负号也要能认出来。
        assert_eq!(parse_utc_offset("welcome\ncwd: /var/tmp\n+0800"), Some(480));
        assert_eq!(parse_utc_offset("GMT"), None);
        assert_eq!(parse_utc_offset(""), None);
        // 不存在的偏移不当数：认错了会把定时挪到完全错误的时刻。
        assert_eq!(parse_utc_offset("+9900"), None);
    }

    #[test]
    fn forget_agent_moves_remote_configs_back_to_local() {
        let dir = tempfile();
        let store = Arc::new(Store::new(&dir));
        let mut config = AppConfig::default();
        config.settings.agent_server_id = "s1".to_string();
        config.backup_configs = vec![
            backup_config("b1", "夜间库", true),
            backup_config("b2", "本机库", false),
        ];
        config.container_configs = vec![container_config("c1", "远端项目", true)];
        store.save_config(&config).unwrap();
        store
            .save_agent_sync(&AgentSyncState {
                server_id: "s1".to_string(),
                backup_config_ids: vec!["b1".to_string()],
                ..Default::default()
            })
            .unwrap();

        let control = AgentControl::new(store.clone());
        // 卸的不是当前控制机（比如顺手清掉另一台上的旧 agent）：指向和执行位都不能动。
        assert_eq!(control.forget_agent("s2").unwrap(), 0);
        assert!(store.load_config().unwrap().backup_configs[0].run_location.is_remote());
        // 现役那条的指纹也不能跟着陪葬：清了它，本机就判定「未下发」而撒手，
        // 而控制机照旧按它盘上那份跑 —— 一晚两头都不跑。
        assert_eq!(store.load_agent_sync().server_id, "s1");
        assert_eq!(store.load_agent_sync().backup_config_ids, vec!["b1".to_string()]);

        // 远端的两条（一条备份 + 一条容器）退回本机，本来就是本机的那条不被动。
        assert_eq!(control.forget_agent("s1").unwrap(), 2);
        let after = store.load_config().unwrap();
        assert!(after.settings.agent_server_id.is_empty());
        assert!(after
            .backup_configs
            .iter()
            .all(|item| !item.run_location.is_remote()));
        assert!(after
            .container_configs
            .iter()
            .all(|item| !item.run_location.is_remote()));
        // 指纹一起清掉：留着指向已卸载机器的指纹，比压根没有指纹更容易骗过让位判定。
        assert!(store.load_agent_sync().backup_config_ids.is_empty());
        assert!(store.load_agent_sync().server_id.is_empty());
    }

    #[test]
    fn sync_state_only_answers_for_the_machine_it_was_recorded_on() {
        let mut state = AgentSyncState {
            server_id: "s1".to_string(),
            synced_at: "2026-09-27 03:00:00".to_string(),
            backup_config_ids: vec!["b1".to_string()],
            container_config_ids: vec!["c1".to_string()],
            ..Default::default()
        };
        assert!(state.holds_backup("s1", "b1"));
        assert!(state.holds_container("s1", "c1"));
        // 换了控制机（或从来没同步过）就不能让位：那台机器上没有这一份。
        assert!(!state.holds_backup("s2", "b1"));
        assert!(!state.holds_backup("s1", "b9"));
        assert!(!AgentSyncState::default().holds_backup("s1", "b1"));

        // 待收回那份是指纹之外的东西：作废指纹（卸载 / 换机）不该把「旧那台还没收回」一起清掉。
        state.add_orphan("s0");
        state.add_orphan("s0");
        assert_eq!(state.orphan_server_ids, vec!["s0".to_string()]);
        state.clear();
        assert!(state.server_id.is_empty() && state.backup_config_ids.is_empty());
        assert_eq!(state.orphan_server_ids, vec!["s0".to_string()]);
        state.remove_orphan("s0");
        assert!(state.orphan_server_ids.is_empty());
    }

    #[test]
    fn agent_server_id_requires_a_choice() {
        let settings = Settings::default();
        assert!(agent_server_id(&settings).is_err());
        let settings = Settings {
            agent_server_id: " s1 ".to_string(),
            ..Default::default()
        };
        assert_eq!(agent_server_id(&settings).unwrap(), "s1");
    }

    #[test]
    fn locate_binary_prefers_the_hand_placed_copy_over_the_bundled_one() {
        let dir = tempfile();
        let store = Store::new(&dir);
        // 两处都没有：报错要点明该往哪放、怎么构建，界面则按 missing 画一行提示。
        let err = resolve_binary(&store).unwrap_err().to_string();
        assert!(err.contains("agent"), "{err}");
        assert!(err.contains("deploy-agent"), "{err}");
        assert_eq!(binary_info(&store).source, AgentBinarySource::Missing);

        let bundled = dir.join("bundle").join("deploy-agent");
        write_fake_binary(&bundled);
        store.set_bundled_agent_binary(&bundled);
        assert_eq!(
            locate_binary(&store).unwrap(),
            (bundled.clone(), AgentBinarySource::Bundled)
        );

        // 开发期手放的那份必须压过安装包里的，否则本地新编的产物永远传不上去。
        let manual = dir.join("agent").join("deploy-agent");
        write_fake_binary(&manual);
        assert_eq!(
            locate_binary(&store).unwrap(),
            (manual.clone(), AgentBinarySource::Manual)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn locate_binary_rejects_files_that_are_not_linux_binaries() {
        let dir = tempfile();
        let store = Store::new(&dir);
        let binary = dir.join("agent").join("deploy-agent");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        // 本机 cargo build 出来的那份（PE）或随手放的占位文本，都不该被传上服务器。
        let mut pe = x86_64_head();
        pe[..4].copy_from_slice(b"MZ\x90\x03");
        std::fs::write(&binary, pe).unwrap();
        let err = resolve_binary(&store).unwrap_err().to_string();
        assert!(err.contains("ELF"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_binary_info_field_names_match_the_typescript_mirror() {
        let dir = tempfile();
        let store = Store::new(&dir);
        let json = serde_json::to_value(binary_info(&store)).unwrap();
        assert_eq!(json["source"], "missing");
        assert_eq!(json["path"], "");
        assert!(json.get("sizeBytes").is_some(), "{json}");
        // 一处都没有时也要把原因写出来，界面那一行直接画这句话。
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("本机没有可用的 agent 可执行文件"), "{json}");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn x86_64_head() -> [u8; 20] {
        let mut head = [0u8; 20];
        head[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        head[4] = 2; // ELFCLASS64
        head[5] = 1; // 小端
        head[18..20].copy_from_slice(&0x3eu16.to_le_bytes()); // EM_X86_64
        head
    }

    /// 只看 4 字节魔数是不够的：ARM / 32 位 / 大端的 ELF 都会通过，装上之后 systemd 起不来，
    /// 而安装脚本那时已经把旧服务 stop 掉了。
    #[test]
    fn binary_problem_rejects_elf_that_cannot_run_on_the_control_machine() {
        let dir = tempfile();
        let path = dir.join("deploy-agent");
        let check = |head: [u8; 20]| -> Option<String> {
            std::fs::write(&path, head).unwrap();
            binary_problem(&path)
        };

        assert_eq!(check(x86_64_head()), None);

        let mut arm = x86_64_head();
        arm[18..20].copy_from_slice(&0xb7u16.to_le_bytes()); // EM_AARCH64
        assert!(check(arm).unwrap().contains("架构"));

        let mut ilp32 = x86_64_head();
        ilp32[4] = 1; // ELFCLASS32
        assert!(check(ilp32).unwrap().contains("32 位"));

        let mut be = x86_64_head();
        be[5] = 2; // 大端
        assert!(check(be).unwrap().contains("大端"));

        let mut not_elf = x86_64_head();
        not_elf[..4].copy_from_slice(b"MZ\x90\x00"); // PE 头
        assert!(check(not_elf).unwrap().contains("ELF"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn event_line_parsing_skips_noise_without_breaking_the_stream() {
        // 触发路径用的是 serde_json::from_str(line).ok()：非 JSON 行必须安静跳过。
        let good = serde_json::to_string(&BackupEvent::Started {
            record_id: "abc".to_string(),
        })
        .unwrap();
        assert!(serde_json::from_str::<BackupEvent>(&good).is_ok());
        for noise in ["", "   ", "Connection to 10.0.0.1 closed.", "{broken"] {
            assert!(
                serde_json::from_str::<BackupEvent>(noise.trim()).is_err(),
                "{noise:?} 不该被当成事件"
            );
        }
        // Finished 里的多行日志必须还在同一行 JSON 内，否则事件流会被撕成两半。
        let record = BackupRecord {
            id: "abc".to_string(),
            server_id: "s1".to_string(),
            server_name: "源机".to_string(),
            database: "app".to_string(),
            schema: "public".to_string(),
            target_name: "supabase".to_string(),
            target: "postgres://u:***@h/db".to_string(),
            status: DeployStatus::Success,
            error: None,
            log: "第一行\n第二行".to_string(),
            dump_size: 10,
            bundle_path: "/var/lib/deploycode/backups/app-20260927.sql.gz".to_string(),
            started_at: "2026-09-27 03:00:00".to_string(),
            finished_at: Some("2026-09-27 03:01:00".to_string()),
            duration_ms: 60_000,
        };
        let line = serde_json::to_string(&BackupEvent::Finished { record }).unwrap();
        assert!(!line.contains('\n'));
        let parsed: BackupEvent = serde_json::from_str(&line).unwrap();
        match parsed {
            BackupEvent::Finished { record } => assert_eq!(record.log, "第一行\n第二行"),
            _ => panic!("应该是 Finished"),
        }
    }

    /// 临时目录（deploy-core 的测试没有引 tempfile crate，这里用最直白的方式）。
    /// 条数一样但内容已经变了：这一类改动以前两头都看不见（`syncNeeded` 只比条数），
    /// 而控制机那一晚会跑出与界面上相反的东西。
    #[test]
    fn staleness_names_schedule_changes_and_configs_the_agent_still_holds() {
        let dir = tempfile();
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        config.settings.agent_server_id = "s1".to_string();
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_time = "03:00".to_string();
        config.settings.scheduled_backup_config_id = Some("b1".to_string());
        config.settings.scheduled_container_enabled = true;
        config.settings.scheduled_container_time = "04:00".to_string();
        config.settings.scheduled_container_config_ids = vec!["c1".to_string()];
        config.backup_configs = vec![backup_config("b1", "夜间库", true)];
        config.container_configs = vec![container_config("c1", "远端项目", true)];
        store.save_config(&config).unwrap();

        // 下发那一刻：指纹由 remember_sync 写，两边都从它取，口径不会分叉。
        let bundle = build_bundle(&config, Some(480));
        let control = AgentControl::new(Arc::new(Store::new(&dir)));
        control
            .remember_sync("s1", &bundle, &store.load_config().unwrap())
            .expect("写指纹");
        let same = staleness(&store);
        assert!(
            !same.backup_schedule_stale && !same.container_schedule_stale,
            "一模一样不该报陈旧：{same:?}"
        );
        assert!(same.stragglers.is_empty() && !same.synced_at.is_empty());

        // 只改时间点：配置条数不变，界面上那个「与本机不一致」以前根本亮不起来。
        let mut moved = config.clone();
        moved.settings.scheduled_backup_time = "11:00".to_string();
        store.save_config(&moved).unwrap();
        let stale = staleness(&store);
        assert!(stale.backup_schedule_stale, "改了时间要报陈旧：{stale:?}");
        assert!(!stale.container_schedule_stale);

        // 执行位改回本机、以及控制机还持有的容器配置被删掉：两份都要点名。
        let mut revoked = moved.clone();
        revoked.backup_configs = vec![backup_config("b1", "夜间库", false)];
        revoked.container_configs = Vec::new();
        store.save_config(&revoked).unwrap();
        let names = staleness(&store).stragglers;
        assert!(
            names.iter().any(|item| item.contains("夜间库"))
                && names.iter().any(|item| item.contains("c1")),
            "改回本机与被删的都该点出来：{names:?}"
        );

        // 从没下发过 / 换了指向：那是「未下发」，由让位判定报失败，这里不重复报陈旧。
        let mut other = revoked.clone();
        other.settings.agent_server_id = "s9".to_string();
        store.save_config(&other).unwrap();
        let none = staleness(&store);
        assert!(
            !none.backup_schedule_stale && none.stragglers.is_empty(),
            "指纹不属于当前控制机时不该报陈旧：{none:?}"
        );
    }

    /// 旧协议的那份不能拿去执行新语义的配置，哪怕它是 install 刚装上去的。
    #[test]
    fn check_proto_refuses_mismatched_agent() {
        let same = AgentVersion {
            proto: PROTO,
            version: "0.0.1".to_string(),
        };
        assert!(check_proto(&same).is_ok());
        for proto in [PROTO - 1, PROTO + 1] {
            let err = check_proto(&AgentVersion {
                proto,
                version: "0.0.1".to_string(),
            })
            .expect_err("协议不一致必须拒绝");
            let text = err.to_string();
            assert!(text.contains("升级") || text.contains("版本"), "{text}");
        }
    }

    fn tempfile() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-agent-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 写一份能过 ELF 检查的假二进制（够 20 字节，`e_machine` 是 x86_64）。
    fn write_fake_binary(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, x86_64_head()).unwrap();
    }
}
