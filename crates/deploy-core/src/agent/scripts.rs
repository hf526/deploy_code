//! systemd unit 与安装 / 配置下发 / 卸载脚本的生成，以及 systemd 服务状态探测。

use crate::process::shell_quote;
use crate::ssh::SshClient;

use super::{BIN_PATH, DATA_DIR, SERVICE, UNIT_PATH};

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
pub(super) fn install_script(binary_tmp: &str, unit: &str) -> String {
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
pub(super) fn config_install_script(remote_tmp: &str) -> String {
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
pub(super) fn uninstall_script() -> String {
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
pub(super) async fn service_state(client: &SshClient) -> Option<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
