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
//!
//! 文件分工：本文件是协议常量、版本握手与 `AgentStatus`；`bundle` 是配置子集的构建与
//! 下发前检查；`sync` 是同步指纹与陈旧判定；`binary` 是 agent 可执行文件的查找与校验；
//! `scripts` 是 systemd unit 与安装脚本生成；`control` 是 SSH 管理端。

mod binary;
mod bundle;
mod control;
mod scripts;
#[cfg(test)]
mod testutil;
mod sync;

pub use binary::{binary_info, locate_binary, resolve_binary, AgentBinaryInfo, AgentBinarySource};
pub use bundle::{build_bundle, bundle_warnings};
pub use control::{AgentControl, AgentSyncReport};
pub use scripts::unit_template;
pub use sync::{
    agent_server_id, backup_schedule_key, container_schedule_key, staleness, AgentStaleness,
    AgentSyncState,
};

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::disk;
use crate::error::{CoreError, Result};

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

/// `deploy-agent prune` 的返回：删掉的孤儿包个数与释放的字节数。
///
/// 客户端经 SSH exec 调起（见 [`AgentControl::prune`]），agent 往 stdout 打这一个 JSON 文档。
/// 判据与本机设置页的「未认领备份包」一致：只认两个备份包目录的直接子文件、删前重核记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPruneReport {
    pub deleted: usize,
    pub freed_bytes: u64,
}

/// 协议对不上就拒绝任何下发（装的是哪一份都得先过这一关）。
///
/// 下发过去的 config.json 是按当前语义写的：旧协议那份读它，字段含义可能已经改过，
/// 而 `remember_sync` 一记上本机就撒手 —— 到点跑出一份谁也没预期的东西。
/// 握手那两条命令（`sync` / `status`）都过这里，`install` 也必须过：它紧接着自己 push_bundle。
pub(super) fn check_proto(remote: &AgentVersion) -> Result<()> {
    let local = local_version();
    if remote.proto != local.proto {
        return Err(CoreError::config(version_mismatch(&local, remote)));
    }
    Ok(())
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
}
