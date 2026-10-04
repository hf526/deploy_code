//! SSH 管理端：安装、下发、回读与即时触发，全部以「一次 SSH 会话」为单位，用完即断。

use std::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::models::{
    AppConfig, BackupEvent, BackupRecord, ContainerEvent, ContainerRecord, Settings,
};
use crate::process::shell_quote;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;
use crate::util::human_size;

use super::binary::resolve_binary;
use super::bundle::{build_bundle, bundle_warnings, parse_utc_offset};
use super::scripts::{
    config_install_script, install_script, service_state, uninstall_script, unit_template,
};
use super::sync::{agent_server_id, backup_schedule_key, container_schedule_key, AgentSyncState};
use super::{check_proto, parse_version, AgentStatus, AgentVersion, BIN_PATH, DATA_DIR, SERVICE};

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
    pub(super) fn remember_sync(&self, server_id: &str, bundle: &AppConfig, config: &AppConfig) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{backup_config, container_config, tempfile};
    use crate::agent::AgentSyncState;
    use crate::models::DeployStatus;

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
}
