//! 终端里的文件上传 / 下载：走已经在线的那条常驻会话，用的是 SFTP。
//!
//! 为什么不接 `rz` / `sz` 那套 ZMODEM：那是让**终端程序**在 PTY 上现场扮演协议对端——嗅探
//! 握手、接管这条通道的输出不再喂界面、按帧解出文件、再算 CRC。这里刻意不做，只做「用户
//! 真正要的那件事」：把本机文件放到服务器上、把远端文件取回来，路径由界面给。远端真跑了
//! `rz`，[`crate::terminal`] 会认出那串 ZRQINIT 并发一条提示事件，不让它干等到用户以为软件
//! 死了。
//!
//! 三条跟着来的口径：
//! 1. **复用会话**：SFTP 通道开在 [`crate::hostlink`] 那条常驻会话上，不再认证一次（同交互式
//!    终端那条理由：认证次数是敏感项）。代价是它和隧道转发、探针、PTY 抢同一份 sshd
//!    `MaxSessions`，所以宁可拒绝也不挤满：每台一条、全局三条。
//! 2. **半截包不许落在正式名字上**：上传先写 `<目录>/.<名字>.<标志>.part`，整包到了才
//!    rename；下载先写本机 `.part`，落地才改名。取消与出错都要把那个临时名删掉——留下的
//!    半截 gzip 从名字上看不出来，拿去解压才炸。删不掉时把路径说进错误里，不静默。
//!    唯一够不到的一条是退出：会话先被 `stop_all()` 收掉，那时临时名还躺在目标目录里。
//! 3. **收尾事件只发一条**：取消与任务本身会在同一瞬间都想给这次传输定性，`settled` 那个
//!    闸门决定谁说话，输的一方一个事件都不发、也不动临时名。
//!
//! ⚠️ 传输内容本身不进任何日志与记录：那是用户的文件，不是任务输出。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::hostlink::clip;
use crate::ssh::SshClient;

/// 一台服务器同时允许的传输数。一路 SFTP 就吃掉一个 session 槽，与隧道、探针、终端同账。
pub const MAX_TRANSFERS_PER_SERVER: usize = 1;
/// 整个软件同时允许的传输数。
pub const MAX_TRANSFERS_TOTAL: usize = 3;

type Emit = dyn Fn(TransferEvent) + Send + Sync + 'static;

/// 传输方向。展示用的中文名在前端取。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    Upload,
    Download,
}

/// 一次传输的对外信息（界面对账与取消入口都靠它）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferInfo {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub direction: TransferDirection,
    /// 展示用的文件名：上传取本机文件的名，下载取远端路径的名。
    pub file_name: String,
    /// 最终落到远端的那个绝对路径（不是临时名）。
    pub remote_path: String,
}

/// 传输事件，走 `transfer://event`。
///
/// 同 `TerminalEvent`：变体名靠 `rename_all`，字段要靠 `rename_all_fields`，否则前端读到
/// undefined。进度不给百分比，`done`/`total` 原样给，前端一份口径算就行。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TransferEvent {
    Started {
        id: String,
        server_id: String,
        server_name: String,
        direction: TransferDirection,
        file_name: String,
        remote_path: String,
    },
    Progress {
        id: String,
        done: u64,
        total: u64,
    },
    Finished {
        id: String,
    },
    /// 用户主动取消。与失败分开：那条文件根本没落地，界面不该画成红色。
    Cancelled {
        id: String,
    },
    Failed {
        id: String,
        reason: String,
    },
}

/// 界面填的远端路径 → 最终落点。
///
/// 只收绝对路径：相对路径交给 SFTP 是按**那一路会话的当前目录**解释的，而那一轮会话的当前
/// 目录我们既不知道也不保证（用户在终端里 `cd` 过），同一句 `a.tar.gz` 两次点下去能落到两个
/// 地方。`~` 同理是字面量，界面要拿 [`SshClient::remote_home`] 先展开成绝对路径。
pub fn transfer_target(remote: &str, file_name: &str) -> Result<String> {
    let trimmed = remote.trim();
    if trimmed.is_empty() || trimmed == "~" || trimmed.starts_with("~/") {
        return Err(CoreError::config(format!(
            "远端路径要写以 / 开头的绝对路径（~ 不会被展开），例如 /root/{file_name}"
        )));
    }
    if !trimmed.starts_with('/') {
        return Err(CoreError::config(format!(
            "远端路径「{trimmed}」不是绝对路径：SFTP 那一路的当前目录不由界面上的终端决定，请写成 / 开头的完整路径"
        )));
    }
    if trimmed.split('/').any(|part| part == "..") {
        return Err(CoreError::config(format!(
            "远端路径「{trimmed}」里有 ..，请改成一个确定的完整路径"
        )));
    }
    // 末尾带斜杠 = 用户给的是目录，文件名跟着落进去；不带就当成完整文件名。
    if trimmed.ends_with('/') {
        Ok(format!("{trimmed}{file_name}"))
    } else {
        Ok(trimmed.to_string())
    }
}

/// 远端的临时名：与正式名同一个目录（rename 才是同盘改名），前置 `.` 免得用户 `ls` 时把
/// 半截包当成正经文件。
pub fn remote_part_path(final_path: &str, token: &str) -> String {
    match final_path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/.{name}.{token}.part"),
        // 理论上到不了这里：transfer_target 保证了以 / 开头。
        None => format!("./.{final_path}.{token}.part"),
    }
}

/// 本机的临时名，规则同远端。
pub fn local_part_path(final_path: &Path, token: &str) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "transfer".to_string());
    let part = format!(".{name}.{token}.part");
    match final_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(part),
        _ => PathBuf::from(part),
    }
}

/// 路径最后那一段，用作展示名与「目录 + 文件名」拼装的那个文件名。
pub fn file_name_of(path: &Path) -> Result<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty() && name != "." && name != "..")
        .ok_or_else(|| CoreError::config(format!("{} 不是一个能传输的文件路径", path.display())))
}

/// 本机那一个东西能不能上传：不存在、是目录都不行。
///
/// 这一眼必须在看远端之前：`/tmp/` 取到的名字是 `tmp`，SFTP 那边会先建出一个叫 `tmp` 的
/// 文件来，而本机这条路径根本不是一个文件。
pub fn check_uploadable(path: &Path) -> Result<String> {
    let name = file_name_of(path)?;
    let meta = std::fs::metadata(path)
        .map_err(|err| CoreError::config(format!("要上传的文件读不到：{}（{err}）", path.display())))?;
    if !meta.is_file() {
        return Err(CoreError::config(format!(
            "{} 不是一个普通文件，目录不能上传，请选一个具体的文件",
            path.display()
        )));
    }
    Ok(name)
}

/// 从远端绝对路径取文件名（下载方向的展示名）。
///
/// 取不到名字就报错，不兜一个「远端文件」出去：那个字符串会进 TransferInfo.fileName，
/// 而界面把它拼进 toast 与条带里，一句中文假名字在英文界面上就是一处没人翻译的文案。
pub fn remote_file_name(final_path: &str) -> Result<String> {
    let name = final_path.rsplit('/').next().unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." {
        return Err(CoreError::config(format!(
            "远端路径「{final_path}」没有指到一个具体的文件，请写到文件名那一层"
        )));
    }
    Ok(name.to_string())
}

/// 进度上报的闸门：小文件按 2% 一档，总数未知的按 1 MiB 一档，收尾那一条必发。
///
/// 不节流的话一次大文件上传会刷出上千个事件，而界面每一格都要过一遍 React。
pub fn should_emit_progress(last_emit: u64, done: u64, total: u64) -> bool {
    if done == total || last_emit == 0 {
        return true;
    }
    if total == 0 {
        return done.saturating_sub(last_emit) >= 1024 * 1024;
    }
    let step = (total / 50).max(1);
    done.saturating_sub(last_emit) >= step
}

/// 收尾闸门：抢到的人才发事件、才动临时名。
fn claim(settled: &AtomicBool) -> bool {
    !settled.swap(true, Ordering::AcqRel)
}

/// 一次传输的名字们：正式落点 + 那个先占位的临时名（同目录）。
#[derive(Clone)]
struct TransferPaths {
    local: PathBuf,
    remote: String,
    /// 上传时的远端临时名。
    remote_part: Option<String>,
    /// 下载时的本机临时名。
    local_part: Option<PathBuf>,
    file_name: String,
}

impl TransferPaths {
    /// 先占位的那个临时名：上传落在远端、下载落在本机，只给错误文案用。
    fn part_label(&self) -> String {
        match (&self.remote_part, &self.local_part) {
            (Some(part), _) => part.clone(),
            (None, Some(part)) => part.display().to_string(),
            (None, None) => "（这一次没有临时名）".to_string(),
        }
    }
}

struct ActiveTransfer {
    info: TransferInfo,
    client: Arc<SshClient>,
    emit: Arc<Emit>,
    settled: Arc<AtomicBool>,
    paths: TransferPaths,
    task: tokio::task::AbortHandle,
}

/// 传输登记表。与终端同例：**不占任务名额**，跟部署/备份/容器任务不互斥。
#[derive(Default)]
pub struct TransferManager {
    running: Mutex<HashMap<String, ActiveTransfer>>,
    /// 上限检查与登记之间隔着建通道那一步，两次点击不能都通过检查再一起超线。
    opening: tokio::sync::Mutex<()>,
}

impl TransferManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 起一次传输。命令返回的是登记信息，进度与结果走 `transfer://event`。
    pub async fn start<F>(
        self: &Arc<Self>,
        client: Arc<SshClient>,
        server_id: &str,
        server_name: &str,
        direction: TransferDirection,
        local: PathBuf,
        remote: String,
        on_event: F,
    ) -> Result<TransferInfo>
    where
        F: Fn(TransferEvent) + Send + Sync + 'static,
    {
        // 路径与文件名的校验排在建链之前：填错了不该先占一个名额、起一条通道，再让界面收
        // 一对 started + failed。
        //
        // 本机路径同样只收绝对路径（与部署那边 env 文件的 `local_path` 一条理由）：相对路径是
        // 按**当前进程的工作目录**解释的，同一份输入换个启动目录就指向另一个文件，而那个文件
        // 一旦被当成要传的那一个，用户看到的就是「传了个没打算传的东西」。
        if !local.as_os_str().is_empty() && !local.is_absolute() {
            return Err(CoreError::config(format!(
                "本机路径「{}」不是绝对路径：它要按本机的当前目录解释，同一个名字换个启动目录就是另一个文件",
                local.display()
            )));
        }
        let paths = match direction {
            TransferDirection::Upload => {
                let file_name = check_uploadable(&local)?;
                let remote = transfer_target(&remote, &file_name)?;
                TransferPaths {
                    local,
                    file_name,
                    remote: remote.clone(),
                    remote_part: Some(remote_part_path(&remote, &token())),
                    local_part: None,
                }
            }
            TransferDirection::Download => {
                if local.as_os_str().is_empty() {
                    return Err(CoreError::config("还没有选择要保存到本机的哪个文件"));
                }
                file_name_of(&local)?;
                // 目录不能下载：SFTP 那一路没有「把整个目录打包取回来」这一步，界面上也不给
                // 这个入口，所以在这里就说清楚要的是一个文件。
                let raw = remote.trim();
                if raw.ends_with('/') {
                    return Err(CoreError::config(format!(
                        "下载要指明远端那一个文件，例如 {raw}app.tar.gz"
                    )));
                }
                let remote = transfer_target(raw, "app.tar.gz")?;
                let file_name = remote_file_name(&remote)?;
                TransferPaths {
                    file_name,
                    local_part: Some(local_part_path(&local, &token())),
                    remote,
                    local,
                    remote_part: None,
                }
            }
        };

        let _guard = self.opening.lock().await;
        if let Some(message) = self.cap_message(server_id, server_name) {
            return Err(CoreError::busy(message));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let info = TransferInfo {
            id: id.clone(),
            server_id: server_id.to_string(),
            server_name: server_name.to_string(),
            direction,
            file_name: paths.file_name.clone(),
            remote_path: paths.remote.clone(),
        };
        let emit: Arc<Emit> = Arc::new(on_event);
        let settled = Arc::new(AtomicBool::new(false));
        // 同 terminal.rs：Started 必须在传输任务起来之前发，否则「一建通道就失败」那一种会让
        // Failed 排在 Started 前面，界面收到一个不认识的 id 只能忽略，这次传输从此没有归宿。
        emit(TransferEvent::Started {
            id: id.clone(),
            server_id: info.server_id.clone(),
            server_name: info.server_name.clone(),
            direction,
            file_name: info.file_name.clone(),
            remote_path: info.remote_path.clone(),
        });

        let handle = tokio::spawn(run(
            self.clone(),
            id.clone(),
            direction,
            paths.clone(),
            Arc::clone(&client),
            Arc::clone(&emit),
            Arc::clone(&settled),
        ));
        lock(&self.running).insert(
            id.clone(),
            ActiveTransfer {
                info: info.clone(),
                client,
                emit,
                settled,
                paths,
                task: handle.abort_handle(),
            },
        );
        // 任务也可能在登记之前就失败收场（本机文件不存在那一种），它那次自摘摘的是空表，
        // 这条就永远占着一个名额了。
        if handle.is_finished() {
            lock(&self.running).remove(&id);
        }
        Ok(info)
    }

    /// 取消一次传输：先摘登记（名额当场还回去），再掐任务，最后收掉半截包。
    pub async fn cancel(&self, id: &str) -> Result<()> {
        let entry = self.take(id)?;
        // 抢不到闸门说明任务自己已经发了 Finished/Failed，那时临时名也归它处理——这里再删
        // 一次就是在删一个已经改过名的文件，还会让界面收到两条定性的事件。
        let claimed = claim(&entry.settled);
        entry.task.abort();
        if !claimed {
            return Ok(());
        }
        // 先摘条带再收临时名：清理那两步各带 120 秒超时，会话正在死的时候不能让用户对着
        // 一个其实已经停下来的进度条干等。要提醒的那句话仍由命令的 Err 带回去，不会丢。
        (entry.emit)(TransferEvent::Cancelled { id: id.to_string() });
        let (part_was_there, leftover) = discard_part(&entry.client, &entry.paths).await;
        // abort 是协作式的，抢门的这一刻任务可能正好把最后那一步改名做完了。改名是拿走临时名
        // 的唯一正常步骤，所以「临时名已经不在了」要说出来——但只说到「请核对」：建文件之前就
        // 失败的那一种同样不会有临时名，界面不能替用户断定这一次到底传完没传完。
        let note = match leftover {
            Some(message) => Some(format!("已取消，但{message}")),
            None if !part_was_there => Some(format!(
                "已取消；临时名 {} 当时已经不在了，如果那是改名拿走的，{} 可能就是刚传完的那一份，请核对一下",
                entry.paths.part_label(),
                entry.paths.remote
            )),
            None => None,
        };
        match note {
            None => Ok(()),
            Some(message) => Err(CoreError::ssh(message)),
        }
    }

    /// 当前还在进行的传输，供界面重载后对账。
    pub fn list(&self) -> Vec<TransferInfo> {
        let mut list: Vec<TransferInfo> = lock(&self.running)
            .values()
            .map(|entry| entry.info.clone())
            .collect();
        list.sort_by(|a, b| a.server_name.cmp(&b.server_name).then_with(|| a.id.cmp(&b.id)));
        list
    }

    fn cap_message(&self, server_id: &str, server_name: &str) -> Option<String> {
        let infos: Vec<TransferInfo> = lock(&self.running)
            .values()
            .map(|entry| entry.info.clone())
            .collect();
        cap_message_for(count_for_server(&infos, server_id), infos.len(), server_name)
    }

    fn take(&self, id: &str) -> Result<ActiveTransfer> {
        lock(&self.running)
            .remove(id)
            .ok_or_else(|| CoreError::not_found("这次传输已经结束或不存在"))
    }

    fn remove_quietly(&self, id: &str) {
        lock(&self.running).remove(id);
    }
}

/// 两台上限的判定（同 terminal.rs 的口径：先这一台、再全局，报的是该去关哪一台）。
fn cap_message_for(per_server: usize, total: usize, server_name: &str) -> Option<String> {
    if per_server >= MAX_TRANSFERS_PER_SERVER {
        return Some(format!(
            "{server_name} 已有一个文件传输在进行：传输与隧道、探针、终端共用同一条 SSH 会话的通道数，请等它结束或先取消"
        ));
    }
    if total >= MAX_TRANSFERS_TOTAL {
        return Some(format!(
            "同时在进行的传输已达上限 {MAX_TRANSFERS_TOTAL}，请等其中一个结束"
        ));
    }
    None
}

fn count_for_server(infos: &[TransferInfo], server_id: &str) -> usize {
    infos.iter().filter(|info| info.server_id == server_id).count()
}

/// 一次传输的正文：先写临时名，成功后改名落到正式名字上。
async fn run(
    manager: Arc<TransferManager>,
    id: String,
    direction: TransferDirection,
    paths: TransferPaths,
    client: Arc<SshClient>,
    emit: Arc<Emit>,
    settled: Arc<AtomicBool>,
) {
    // 名额的归还挂在 Drop 上：这条路万一 panic（或被 abort 在半路），条目留在登记表里就把
    // 这一台的唯一一个名额卡到软件重启，而界面上一个进行中的传输都看不见。
    let _unregister = Unregister(manager, id.clone());
    let outcome = transfer(&direction, &paths, &client, &emit, &id).await;

    // 取消那一路可能已经抢先把这次定性了，那就一个字都不再说，临时名也归它收。
    if !claim(&settled) {
        return;
    }
    match outcome {
        Ok(()) => emit(TransferEvent::Finished { id }),
        Err(err) => {
            // 远端回来的错误串长度由服务器决定（SFTP 的状态文本可以很长），进 toast 之前
            // 一律截断，同 hostlink 那份状态字段的口径。
            let mut reason = clip(&err.to_string()).unwrap_or_else(|| err.to_string());
            // 谁留下谁负责删（同数据库备份那条远端导出包的口径）。
            let (_, leftover) = discard_part(&client, &paths).await;
            if let Some(message) = leftover {
                reason.push('；');
                reason.push_str(&message);
            }
            emit(TransferEvent::Failed { id, reason });
        }
    }
}

/// 见 `run`：登记表的兜底摘除。
struct Unregister(Arc<TransferManager>, String);

impl Drop for Unregister {
    fn drop(&mut self) {
        self.0.remove_quietly(&self.1);
    }
}

async fn transfer(
    direction: &TransferDirection,
    paths: &TransferPaths,
    client: &SshClient,
    emit: &Arc<Emit>,
    id: &str,
) -> Result<()> {
    let mut last_emit = 0u64;
    let mut on_progress = |done: u64, total: u64| {
        if should_emit_progress(last_emit, done, total) {
            last_emit = done;
            emit(TransferEvent::Progress {
                id: id.to_string(),
                done,
                total,
            });
        }
    };
    match direction {
        TransferDirection::Upload => {
            let part = paths
                .remote_part
                .as_deref()
                .unwrap_or(paths.remote.as_str());
            client.upload_file(&paths.local, part, &mut on_progress).await?;
            if part != paths.remote {
                client.rename_remote(part, &paths.remote).await?;
            }
            Ok(())
        }
        TransferDirection::Download => {
            let part = paths
                .local_part
                .clone()
                .unwrap_or_else(|| paths.local.clone());
            client.download_file(&paths.remote, &part, &mut on_progress).await?;
            if part != paths.local {
                std::fs::rename(&part, &paths.local)
                    .map_err(|err| CoreError::io_path(&part, err))?;
            }
            Ok(())
        }
    }
}

/// 收掉那一个先占位的临时名。返回「它本来在不在」与删不掉时要交给用户的那句话。
///
/// 「本来不在」不是错误：改名成功、或建文件之前就失败，都会是这个样子。调用方据此决定要不要
/// 多说一句，不能一上来就报「没能删掉」——那会让人去手动清一个根本不存在的文件。
async fn discard_part(client: &SshClient, paths: &TransferPaths) -> (bool, Option<String>) {
    if let Some(part) = &paths.remote_part {
        // 先查再删：NoSuchFile 那种失败不该被算成「删不掉」。
        let existed = client.remote_exists(part).await.unwrap_or(true);
        if !existed {
            return (false, None);
        }
        match client.remove_remote(part).await {
            Ok(()) => (true, None),
            Err(err) => (
                true,
                Some(format!("远端临时文件 {part} 没能删掉，请手动清理（{err}）")),
            ),
        }
    } else if let Some(part) = &paths.local_part {
        let existed = part.exists();
        if !existed {
            return (false, None);
        }
        match std::fs::remove_file(part) {
            Ok(()) => (true, None),
            Err(err) => (
                true,
                Some(format!(
                    "本机临时文件 {} 没能删掉，请手动清理（{err}）",
                    part.display()
                )),
            ),
        }
    } else {
        (false, None)
    }
}

/// 临时名后缀：一次传输一个，同目录里同时传两个同名文件也不会撞车。
fn token() -> String {
    let full = uuid::Uuid::new_v4().simple().to_string();
    full[..8].to_string()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_and_tilde_remote_paths_are_refused_with_an_example() {
        let err = transfer_target("a.tar.gz", "a.tar.gz").expect_err("相对路径要拒绝");
        assert!(err.to_string().contains("绝对路径"), "{err}");
        let err = transfer_target("~/a.tar.gz", "a.tar.gz").expect_err("~ 不会被展开");
        assert!(err.to_string().contains("/root/a.tar.gz"), "要举个例子：{err}");
        assert!(transfer_target("  ", "a.tar.gz").is_err());
    }

    /// `..` 在这里不是「方便」而是「歧义」：这个路径要拿去 rename、也要拿去删临时名，
    /// 落在哪个目录必须在提交时就确定下来。
    #[test]
    fn dot_dot_in_the_remote_path_is_refused() {
        let err = transfer_target("/tmp/../etc/passwd", "x").expect_err("要拒绝");
        assert!(err.to_string().contains(".."), "{err}");
        assert!(transfer_target("/tmp/app.tar.gz", "app.tar.gz").is_ok());
    }

    #[test]
    fn a_directory_input_gets_the_file_name_appended() {
        assert_eq!(transfer_target("/tmp/", "a.tgz").unwrap(), "/tmp/a.tgz");
        assert_eq!(transfer_target("/tmp", "a.tgz").unwrap(), "/tmp");
        assert_eq!(transfer_target("/tmp/a.tgz", "ignored").unwrap(), "/tmp/a.tgz");
    }

    /// 临时名必须与正式名同一个目录：跨盘的 rename 不是改名而是复制，那一步会把「先写
    /// 临时、成功后落地」整个变成空话。
    #[test]
    fn the_part_name_sits_in_the_same_directory_and_is_hidden() {
        assert_eq!(
            remote_part_path("/root/app.tar.gz", "abcd1234"),
            "/root/.app.tar.gz.abcd1234.part"
        );
        assert_eq!(
            local_part_path(Path::new("C:\\Users\\me\\Downloads\\a.tgz"), "abcd1234"),
            PathBuf::from("C:\\Users\\me\\Downloads\\.a.tgz.abcd1234.part")
        );
        assert_eq!(
            local_part_path(Path::new("a.tgz"), "abcd1234"),
            PathBuf::from(".a.tgz.abcd1234.part")
        );
    }

    #[test]
    fn progress_is_throttled_but_the_last_one_always_goes_out() {
        let total = 1_000_000u64;
        // 第一条必发：界面要立刻拿到总大小，否则只有个「传输中」看不出进度。
        assert!(should_emit_progress(0, 4096, total));
        assert!(!should_emit_progress(500_000, 501_000, total));
        assert!(should_emit_progress(500_000, 520_000, total));
        // 收尾那一条哪怕没到步长也要发，否则界面永远停在 98%。
        assert!(should_emit_progress(999_000, total, total));
        // 总数未知（远端还没报上来）时按 1 MiB 一档。
        assert!(!should_emit_progress(1024 * 1024, 2 * 1024 * 1024 - 1, 0));
        assert!(should_emit_progress(1024 * 1024, 2 * 1024 * 1024, 0));
    }

    #[test]
    fn per_server_cap_names_the_server_and_points_at_the_way_out() {
        let message = cap_message_for(MAX_TRANSFERS_PER_SERVER, 1, "深圳").expect("要排队");
        assert!(message.contains("深圳"), "{message}");
        assert!(message.contains("取消"), "要说下一步怎么办：{message}");
        assert!(cap_message_for(0, MAX_TRANSFERS_TOTAL - 1, "深圳").is_none());
        let message = cap_message_for(0, MAX_TRANSFERS_TOTAL, "深圳").expect("全局上限");
        assert!(!message.contains("深圳"), "这条该报的是总数：{message}");
    }

    /// 取消与任务收尾撞在一起时只能有一个说话：两个都发，界面就是先「已取消」又「成功」。
    #[test]
    fn only_the_first_settler_gets_to_report() {
        let settled = AtomicBool::new(false);
        assert!(claim(&settled), "第一次抢到");
        assert!(!claim(&settled), "第二次必须抢不到");
        assert!(!claim(&settled));
    }

    #[test]
    fn file_name_of_rejects_paths_that_are_not_files() {
        assert_eq!(file_name_of(Path::new("/tmp/a.tgz")).unwrap(), "a.tgz");
        assert_eq!(file_name_of(Path::new("C:\\a\\b.bin")).unwrap(), "b.bin");
        assert!(file_name_of(Path::new("..")).is_err());
        assert!(file_name_of(Path::new("")).is_err(), "空路径要在校验处就拦住");
        assert_eq!(remote_file_name("/opt/app/a.tgz").unwrap(), "a.tgz");
        // 取不到名字要报错，不能兜一个「远端文件」进 fileName：那句中文会跟着进 toast 与条带。
        let err = remote_file_name("/opt/app/").expect_err("目录没有文件名");
        assert!(err.to_string().contains("具体的文件"), "{err}");
        let err = remote_file_name("/..").expect_err(".. 不是文件名");
        assert!(err.to_string().contains("具体的文件"), "{err}");
    }

    /// 取消抢到闸门、而临时名当时已经不在了——那多半是改名刚做完。这句必须说「请核对」，
    /// 既不能安静地报「已取消」，也不能替用户断定传完了。
    #[test]
    fn the_part_label_points_at_whichever_side_holds_the_partial() {
        let upload = TransferPaths {
            local: PathBuf::from("C:\\a\\b.tgz"),
            remote: "/root/b.tgz".to_string(),
            remote_part: Some("/root/.b.tgz.1234.part".to_string()),
            local_part: None,
            file_name: "b.tgz".to_string(),
        };
        assert_eq!(upload.part_label(), "/root/.b.tgz.1234.part");
        let download = TransferPaths {
            local: PathBuf::from("C:\\a\\b.tgz"),
            remote: "/root/b.tgz".to_string(),
            remote_part: None,
            local_part: Some(PathBuf::from("C:\\a\\.b.tgz.1234.part")),
            file_name: "b.tgz".to_string(),
        };
        assert_eq!(download.part_label(), "C:\\a\\.b.tgz.1234.part");
    }

    /// 目录取到的名字是最后那一段（`/tmp/` → `tmp`），所以「是不是一个文件」只能在落盘之前
    /// 那一眼判——到了远端就会建出一个同名的假文件来。
    #[test]
    fn a_directory_is_refused_before_the_transfer_starts() {
        let dir = std::env::temp_dir().join(format!("deploycode-transfer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("临时目录");
        let file = dir.join("a.tgz");
        std::fs::write(&file, b"x").expect("临时文件");

        let err = check_uploadable(&dir).expect_err("目录不能上传");
        assert!(err.to_string().contains("不是一个普通文件"), "{err}");
        assert_eq!(check_uploadable(&file).unwrap(), "a.tgz");

        let missing = dir.join("没有这个文件");
        let err = check_uploadable(&missing).expect_err("不存在的文件要说读不到");
        assert!(err.to_string().contains("读不到"), "{err}");

        std::fs::remove_dir_all(&dir).expect("清理临时目录");
    }

    #[test]
    fn event_json_keys_match_the_frontend_types() {
        let started = serde_json::to_value(TransferEvent::Started {
            id: "t1".to_string(),
            server_id: "s1".to_string(),
            server_name: "深圳".to_string(),
            direction: TransferDirection::Upload,
            file_name: "a.tgz".to_string(),
            remote_path: "/root/a.tgz".to_string(),
        })
        .expect("序列化");
        assert_eq!(started["type"], "started");
        assert_eq!(started["serverId"], "s1");
        assert_eq!(started["fileName"], "a.tgz");
        assert_eq!(started["remotePath"], "/root/a.tgz");
        assert_eq!(started["direction"], "upload");

        let progress = serde_json::to_value(TransferEvent::Progress {
            id: "t1".to_string(),
            done: 5,
            total: 9,
        })
        .expect("序列化");
        assert_eq!(progress["type"], "progress");
        assert_eq!(progress["done"], 5);
        assert_eq!(progress["total"], 9);

        let cancelled =
            serde_json::to_value(TransferEvent::Cancelled { id: "t1".to_string() }).expect("序列化");
        assert_eq!(cancelled["type"], "cancelled");

        let failed = serde_json::to_value(TransferEvent::Failed {
            id: "t1".to_string(),
            reason: "远端拒绝".to_string(),
        })
        .expect("序列化");
        assert_eq!(failed["reason"], "远端拒绝");

        let listed = serde_json::to_value(TransferInfo {
            id: "t1".to_string(),
            server_id: "s1".to_string(),
            server_name: "深圳".to_string(),
            direction: TransferDirection::Download,
            file_name: "a.tgz".to_string(),
            remote_path: "/root/a.tgz".to_string(),
        })
        .expect("序列化");
        assert_eq!(listed["direction"], "download");
        for key in ["id", "serverId", "serverName", "fileName", "remotePath"] {
            assert!(listed.get(key).is_some(), "TransferInfo 缺字段 {key}");
        }
    }

    #[tokio::test]
    async fn unknown_transfer_is_reported_as_gone() {
        let manager = TransferManager::new();
        let err = manager.cancel("没有这一条").await.expect_err("要报错");
        assert!(matches!(err, CoreError::NotFound(_)), "{err}");
        assert!(manager.list().is_empty());
    }
}
