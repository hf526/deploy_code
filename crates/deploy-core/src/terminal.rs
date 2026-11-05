//! 交互式终端：在常驻的那条 SSH 会话上再开一条 PTY 通道。
//!
//! 为什么复用会话，而不是每个标签各连一次：认证次数在本项目里是敏感项——服务器上的
//! 自动防护（[`crate::security`]）按窗口内的失败次数拉黑来源 IP，「每开一个标签认证一次」
//! 正好把成功与失败的认证堆进同一分钟。复用之后，开一个标签只是在已有会话上多开一条通道，
//! 代价换成 sshd 的 `MaxSessions`（OpenSSH 默认 10）——而隧道转发与资源探针用的是同一份
//! 槽位。所以这里给每台设了上限：宁可拒绝开新标签，也不能把已有会话的通道挤满，那会让这一台
//! 的隧道和探针一起打不开，而会话看着完全健康。
//!
//! 输出走回调而不是直接发事件：这个 crate 不许依赖 GUI，`terminal://event` 那一句由
//! `src-tauri` 的命令层发。
//!
//! ⚠️ 红线：终端里的输入与输出都不落盘、不进任何记录。那是用户自己的屏幕，不是日志。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose;
use russh::client::Msg;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf};
use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::ssh::SshClient;

/// 一台服务器同时允许的终端标签数。与隧道规则、探针通道一起算，留出 sshd 默认 10 的余量。
pub const MAX_TERMINALS_PER_SERVER: usize = 3;
/// 整个软件同时允许的终端标签数。
pub const MAX_TERMINALS_TOTAL: usize = 8;
/// 一次键盘输入的最大字节数：粘贴一大段文本要能过去，但不许把排队变成无界。
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// 一个终端标签的对外信息（界面重载后用它对账哪些标签还活着）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub cols: u16,
    pub rows: u16,
}

/// 终端事件，走 `terminal://event`。
///
/// 枚举上的 `rename_all` 只管变体名，字段要靠 `rename_all_fields`（同 `DeployEvent` 那三件），
/// 否则 `server_id` 会原样出去，前端读 `serverId` 拿到 undefined。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TerminalEvent {
    Opened {
        id: String,
        server_id: String,
        server_name: String,
    },
    /// `bytes` 是 base64：PTY 输出是任意字节，按 UTF-8 有损转换会把 vim 的控制序列打成
    /// 乱码，而界面那一侧要的正是这些控制序列。
    Data { id: String, bytes: String },
    /// `reason` 为 `None` 表示远端 shell 自己退出了，没出别的岔子。
    Closed { id: String, reason: Option<String> },
}

/// 通道的写的那一半：键盘输入与窗口尺寸都从这里出去。
type ShellWriter = ChannelWriteHalf<Msg>;

struct ActiveTerminal {
    info: TerminalInfo,
    writer: Arc<ShellWriter>,
    reader: tokio::task::AbortHandle,
}

/// 终端登记表。与部署/备份那四类长任务不同，终端**不占任务名额**：多标签并发是常态，
/// 而它只是已有会话上的一条通道，与正在跑的任务不互斥（同扫描类命令那个口径）。
#[derive(Default)]
pub struct TerminalManager {
    running: Mutex<HashMap<String, ActiveTerminal>>,
    /// 开标签这条路串行：名额检查与登记之间隔着一次网络往返，两个标签同时点进来时不能
    /// 都通过检查再一起超线。
    opening: tokio::sync::Mutex<()>,
}

impl TerminalManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 在 `client` 这条已在线的会话上开一个终端，并把输出交给 `on_event`。
    ///
    /// 要 `&Arc<Self>`：读的那一半跑完时必须把这一条从登记表里摘掉，否则死标签会一直占着
    /// 名额，而界面看到的是「这台已经开满 3 个」。
    pub async fn open<F>(
        self: &Arc<Self>,
        client: Arc<SshClient>,
        server_id: &str,
        server_name: &str,
        cols: u16,
        rows: u16,
        on_event: F,
    ) -> Result<TerminalInfo>
    where
        F: Fn(TerminalEvent) + Send + Sync + 'static,
    {
        let _guard = self.opening.lock().await;
        if let Some(message) = self.cap_message(server_id, server_name) {
            return Err(CoreError::ssh(message));
        }

        let (reader, writer) = client.open_shell(cols, rows).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let info = TerminalInfo {
            id: id.clone(),
            server_id: server_id.to_string(),
            server_name: server_name.to_string(),
            cols,
            rows,
        };
        let writer = Arc::new(writer);
        let emit = Arc::new(on_event);
        // 先发 Opened 再起读任务：反过来时「服务器当场关掉通道」那一种（拒绝分配 PTY 就是
        // 这一种）会让 Closed 排在 Opened 前面，界面收到一个不认识的 id 只能忽略，随后又
        // 把这个已经死掉的标签当成打开状态加进列表——空白标签就是这么来的。
        emit(TerminalEvent::Opened {
            id: id.clone(),
            server_id: info.server_id.clone(),
            server_name: info.server_name.clone(),
        });
        let handle = tokio::spawn(read_loop(
            self.clone(),
            id.clone(),
            reader,
            Arc::clone(&emit),
        ));
        lock(&self.running).insert(
            id.clone(),
            ActiveTerminal {
                info: info.clone(),
                writer,
                reader: handle.abort_handle(),
            },
        );
        // 登记与读任务自己摘除之间隔着一次线程调度：任务如果在我们登记之前就已完成（通道
        // 一开就没了），它那次摘除摘的是空表，这条就已经没人收的僵尸会永久占着一个名额，
        // 而界面从此写着「这台已开满」。跑完了就由这里收掉；没跑完的，之后由它自己摘。
        if handle.is_finished() {
            lock(&self.running).remove(&id);
        }
        Ok(info)
    }

    /// 把界面敲进来（或粘贴进来）的字节送到远端。
    pub async fn send_input(&self, id: &str, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(CoreError::ssh(format!(
                "一次输入不能超过 {} MB，请分批粘贴",
                MAX_INPUT_BYTES / (1024 * 1024)
            )));
        }
        let writer = self.writer(id)?;
        // 远端窗口用完时这里会等着：宁可让这一次输入慢下来，也不丢按键。
        writer
            .data_bytes(bytes)
            .await
            .map_err(|e| CoreError::ssh(format!("终端输入发送失败：{e}")))
    }

    /// 窗口尺寸变了（界面缩放、切标签回来都要报一次）。
    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let writer = self.writer(id)?;
        writer
            .window_change(u32::from(cols), u32::from(rows), 0, 0)
            .await
            .map_err(|e| CoreError::ssh(format!("终端调整窗口失败：{e}")))?;
        if let Some(entry) = lock(&self.running).get_mut(id) {
            entry.info.cols = cols;
            entry.info.rows = rows;
        }
        Ok(())
    }

    /// 关掉一个标签（用户点标签上的 ×，或界面重载后清理孤儿标签）。
    ///
    /// 没有「挨个关掉所有标签」那一步：退出清理只要会话一收（`hosts.stop_all()`），远端
    /// 那些 shell 就随传输一起没了，本机通道也当场读不到东西。
    pub async fn close(&self, id: &str) -> Result<()> {
        let entry = self.take(id)?;
        shutdown_terminal(entry).await;
        Ok(())
    }

    /// 当前还活着的标签，供界面对账。
    pub fn list(&self) -> Vec<TerminalInfo> {
        let mut list: Vec<TerminalInfo> = lock(&self.running)
            .values()
            .map(|entry| entry.info.clone())
            .collect();
        // 标签顺序由界面自己排，这里只给一个稳定顺序，避免每次读回来都在换位置。
        list.sort_by(|a, b| {
            a.server_name
                .cmp(&b.server_name)
                .then_with(|| a.id.cmp(&b.id))
        });
        list
    }

    /// 上限判定：先看这一台，再看全局。
    ///
    /// 分组只认 `server_id`：名字是给用户看的，两台同名机器很常见，按名字算会让第一台把
    /// 第二台的名额吃掉，而错误信息还指着错的机器。
    fn cap_message(&self, server_id: &str, server_name: &str) -> Option<String> {
        let infos: Vec<TerminalInfo> = lock(&self.running)
            .values()
            .map(|entry| entry.info.clone())
            .collect();
        cap_message_for(count_for_server(&infos, server_id), infos.len(), server_name)
    }

    fn writer(&self, id: &str) -> Result<Arc<ShellWriter>> {
        lock(&self.running)
            .get(id)
            .map(|entry| entry.writer.clone())
            .ok_or_else(|| CoreError::not_found("终端已关闭，请重新打开标签"))
    }

    fn take(&self, id: &str) -> Result<ActiveTerminal> {
        lock(&self.running)
            .remove(id)
            .ok_or_else(|| CoreError::not_found("终端已关闭，请重新打开标签"))
    }

    /// 通道结束后由读任务自己摘掉这一条。任务被 abort 时走不到这里，所以调用方（`close`）
    /// 在 abort 之前已经把条目取走了。
    fn remove_quietly(&self, id: &str) {
        lock(&self.running).remove(id);
    }
}

/// 两台上限的判定：`per_server` 是这一台已经开着的标签数，`total` 是全部标签数。
fn cap_message_for(per_server: usize, total: usize, server_name: &str) -> Option<String> {
    if per_server >= MAX_TERMINALS_PER_SERVER {
        return Some(format!(
            "{server_name} 最多同时开 {MAX_TERMINALS_PER_SERVER} 个终端：这些标签与隧道转发、资源探针共用同一条 SSH 会话的通道数，请先关闭一个标签"
        ));
    }
    if total >= MAX_TERMINALS_TOTAL {
        return Some(format!(
            "终端标签总数已达上限 {MAX_TERMINALS_TOTAL}，请先关闭一些标签"
        ));
    }
    None
}

/// 这一台已经占着的标签数。
fn count_for_server(infos: &[TerminalInfo], server_id: &str) -> usize {
    infos.iter().filter(|info| info.server_id == server_id).count()
}

/// 读循环：把远端吐出来的字节交给回调，直到这条通道结束。
async fn read_loop<F>(
    manager: Arc<TerminalManager>,
    id: String,
    mut reader: ChannelReadHalf,
    on_event: Arc<F>,
) where
    F: Fn(TerminalEvent) + Send + Sync + 'static,
{
    let mut reason: Option<String> = None;
    loop {
        match reader.wait().await {
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                on_event(TerminalEvent::Data {
                    id: id.clone(),
                    bytes: general_purpose::STANDARD.encode(data),
                });
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                reason = Some(format!("远端 shell 已退出（退出码 {exit_status}）"));
            }
            Some(ChannelMsg::Close) => break,
            // 通道没了下文：多半是常驻会话重连了，这条通道随旧会话一起作废。
            None => {
                reason.get_or_insert_with(|| "连接已断开，终端会话已结束".to_string());
                break;
            }
            Some(_) => {}
        }
    }
    // 先摘条目再发事件：名额当场还回去，而界面收到 Closed 时也不会在登记表里看到僵尸标签。
    manager.remove_quietly(&id);
    // 这里不主动发 CHANNEL_CLOSE：会话已经不在了，发也发不出去。
    on_event(TerminalEvent::Closed { id, reason });
}

/// 关一条通道：必须显式发 CHANNEL_CLOSE。
///
/// russh 对被 drop 的普通通道不补发 CLOSE（只有 `into_stream()` 那条路会），不发的话远端
/// 那个 shell 会一直占着一个 session 槽——而槽位是和隧道转发、资源探针一起算的。
async fn shutdown_terminal(entry: ActiveTerminal) {
    let _ = entry.writer.close().await;
    // 读的那半在任务里，任务一停它随之释放；此时 CLOSE 已经发出去了。
    entry.reader.abort();
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infos(pairs: &[(&str, &str)]) -> Vec<TerminalInfo> {
        pairs
            .iter()
            .enumerate()
            .map(|(index, (server_id, server_name))| TerminalInfo {
                id: format!("t{index}"),
                server_id: server_id.to_string(),
                server_name: server_name.to_string(),
                cols: 80,
                rows: 24,
            })
            .collect()
    }

    /// 同名不同机：名额必须按 `server_id` 分。两台都叫「深圳轻量云」时，第一台开满不许把
    /// 第二台一起拦下来，也不能让错误信息指着没开满的那一台。
    #[test]
    fn same_named_servers_do_not_share_their_slots() {
        let running = infos(&[
            ("s1", "深圳轻量云"),
            ("s1", "深圳轻量云"),
            ("s1", "深圳轻量云"),
            ("s2", "深圳轻量云"),
        ]);
        assert_eq!(count_for_server(&running, "s1"), 3);
        assert_eq!(count_for_server(&running, "s2"), 1);
        assert!(
            cap_message_for(count_for_server(&running, "s2"), running.len(), "深圳轻量云").is_none(),
            "第二台只占着自己那一个，还能再开"
        );
        assert!(cap_message_for(count_for_server(&running, "s1"), running.len(), "深圳轻量云").is_some());
    }

    #[test]
    fn per_server_cap_names_the_server_and_the_way_out() {
        let message = cap_message_for(MAX_TERMINALS_PER_SERVER, MAX_TERMINALS_PER_SERVER + 1, "深圳")
            .expect("同一台的第四个标签要被拒绝");
        assert!(message.contains("深圳"), "要点名是哪一台：{message}");
        assert!(
            message.contains(&MAX_TERMINALS_PER_SERVER.to_string()),
            "要说清上限是多少：{message}"
        );
        assert!(
            cap_message_for(MAX_TERMINALS_PER_SERVER - 1, MAX_TERMINALS_TOTAL - 1, "香港").is_none(),
            "别的机器还有名额，就不该被拦"
        );
    }

    #[test]
    fn total_cap_blocks_even_when_each_server_is_under_its_own_limit() {
        // 四台各开两个：每台都没超，但总数到了上限。
        let message = cap_message_for(2, MAX_TERMINALS_TOTAL, "E").expect("总数到上限后要拦下来");
        assert!(message.contains(&MAX_TERMINALS_TOTAL.to_string()), "{message}");
        // 少一个就还能开。
        assert!(cap_message_for(2, MAX_TERMINALS_TOTAL - 1, "E").is_none());
    }

    /// 两台同时超线时先报这一台：报全局上限会让人去关别的机器的标签，白忙一场。
    #[test]
    fn per_server_cap_is_checked_before_the_total_one() {
        let message = cap_message_for(MAX_TERMINALS_PER_SERVER, MAX_TERMINALS_TOTAL, "深圳")
            .expect("要被拒绝");
        assert!(message.contains("深圳"), "{message}");
        assert!(!message.contains("总数"), "该报的是这一台：{message}");
    }

    /// 输出必须是原样的字节：控制序列与 UTF-8 边界都靠 base64 保住。
    #[test]
    fn data_event_carries_the_exact_bytes_as_base64() {
        let raw: Vec<u8> = vec![
            0x1b, 0x5b, 0x33, 0x31, 0x6d, // ESC [ 31 m
            0xe4, 0xb8, 0xad, 0xe6, 0x96, 0x87, // 「中文」
            0x0a, 0xff, 0xfe, // 换行 + 两个非法 UTF-8 字节
        ];
        let event = TerminalEvent::Data {
            id: "t1".to_string(),
            bytes: general_purpose::STANDARD.encode(&raw),
        };
        let json = serde_json::to_string(&event).expect("事件要能序列化");
        assert!(json.contains("\"type\":\"data\""), "枚举值走 camelCase：{json}");
        let decoded = match event {
            TerminalEvent::Data { bytes, .. } => {
                general_purpose::STANDARD.decode(bytes).expect("能解回来")
            }
            _ => unreachable!(),
        };
        assert_eq!(decoded, raw, "解回来必须逐字节相同，含 0xff 这种非法 UTF-8");
    }

    #[test]
    fn event_and_info_json_keys_match_the_frontend_types() {
        let opened = serde_json::to_value(TerminalEvent::Opened {
            id: "t1".to_string(),
            server_id: "s1".to_string(),
            server_name: "深圳".to_string(),
        })
        .expect("序列化");
        assert_eq!(opened["type"], "opened");
        assert_eq!(opened["serverId"], "s1");
        assert_eq!(opened["serverName"], "深圳");

        let closed = serde_json::to_value(TerminalEvent::Closed {
            id: "t1".to_string(),
            reason: Some("连接已断开".to_string()),
        })
        .expect("序列化");
        assert_eq!(closed["type"], "closed");
        assert_eq!(closed["reason"], "连接已断开");
        // 正常退出没有原因，字段要还在（前端按 null 判），不能整个键消失。
        let plain = serde_json::to_value(TerminalEvent::Closed {
            id: "t1".to_string(),
            reason: None,
        })
        .expect("序列化");
        assert!(plain.get("reason").is_some(), "reason 为 null 也要带键");

        let data = serde_json::to_value(TerminalEvent::Data {
            id: "t1".to_string(),
            bytes: "AA==".to_string(),
        })
        .expect("序列化");
        assert_eq!(data["bytes"], "AA==");

        let listed = serde_json::to_value(TerminalInfo {
            id: "t1".to_string(),
            server_id: "s1".to_string(),
            server_name: "深圳".to_string(),
            cols: 80,
            rows: 24,
        })
        .expect("序列化");
        for key in ["id", "serverId", "serverName", "cols", "rows"] {
            assert!(listed.get(key).is_some(), "TerminalInfo 缺字段 {key}");
        }
    }

    /// 一次输入超限要说得出口，不能默默截断：截断等于把用户粘的东西少发一截。
    #[tokio::test]
    async fn oversized_input_is_refused_before_touching_the_channel() {
        let manager = TerminalManager::new();
        let err = manager
            .send_input("不存在", vec![b'x'; MAX_INPUT_BYTES + 1])
            .await
            .expect_err("超限要报错");
        assert!(
            err.to_string().contains("分批粘贴"),
            "错误要说下一步怎么办：{err}"
        );
    }

    #[tokio::test]
    async fn unknown_terminal_is_reported_as_closed() {
        let manager = TerminalManager::new();
        let err = manager.close("没有这个标签").await.expect_err("要报错");
        assert!(matches!(err, CoreError::NotFound(_)), "{err}");
        let err = manager.resize("没有这个标签", 80, 24).await.expect_err("要报错");
        assert!(matches!(err, CoreError::NotFound(_)), "{err}");
        assert!(manager.list().is_empty(), "没有标签就是空列表，不是报错");
    }
}
