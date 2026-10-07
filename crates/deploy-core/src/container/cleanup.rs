//! 应用退出 / 任务取消时的远端清理：删掉残留文件，并顺手把「欠一次恢复」的源服务拉起来。

use std::time::Duration;

use crate::engine::kill_script;
use crate::error::{CoreError, Result};
use crate::models::{DeployStatus, ServerConfig};
use crate::process::shell_quote;
use crate::ssh::SshClient;

use super::discover::remote_home;
use super::ContainerEngine;

/// 一次远端清理的时间上限：建链（本机默认给 15s）+ 两次 kill_script（各带一次 `sleep 1`）
/// + 一次按标签的 `docker start`，实测要 6–8 秒。原来外面套的 5 秒会把整条 future 半路 drop，
/// 于是连「这里停过东西」都没写进记录 —— 站点停着而界面一声不吭。
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(12);
const RESUME_OK_NOTE: &str = "已替本次任务把来源服务重新拉起\n";
const RESUME_FAILED_NOTE: &str = "来源服务未能自动拉起，请到该服务器执行 compose up -d\n";
const RESUME_TIMEOUT_NOTE: &str =
    "远端清理超时，未能确认来源服务是否已拉起；若这次勾了「暂停源服务」，请到该服务器执行 compose up -d\n";

impl ContainerEngine {
    /// 应用退出 / 取消时清理某台服务器上本记录留下的远端文件。
    ///
    /// 顺手把「欠一次恢复」的那次快照收掉：脚本自己的 trap 现在接了 HUP，但 bash 要等当前
    /// 那条前台命令（可能是一整份 tar）结束才处理信号，而任务被 abort 时紧随其后的
    /// `kill_script` 从 TERM 到 SIGKILL 只留一秒，`compose start` 撑不完。站点就这么停着，
    /// 而这是备份替用户按下去的停止键。
    ///
    /// 这一步自己带时间上限（[`CLEANUP_TIMEOUT`]）：调用方的预算比它小就会把整条 future 半路
    /// drop 掉，那样连「这里停过东西」都没人写下过，界面上零提示。
    pub async fn cleanup_remote(&self, server: &ServerConfig, record_id: &str) -> Result<()> {
        match tokio::time::timeout(CLEANUP_TIMEOUT, self.cleanup_remote_inner(server, record_id))
            .await
        {
            Ok(result) => result,
            Err(_) => {
                self.note_source_resume(record_id, RESUME_TIMEOUT_NOTE);
                Err(CoreError::ssh(format!(
                    "容器远端清理超时（{} 秒），来源服务是否已恢复未能确认",
                    CLEANUP_TIMEOUT.as_secs()
                )))
            }
        }
    }

    async fn cleanup_remote_inner(&self, server: &ServerConfig, record_id: &str) -> Result<()> {
        let settings = self.store.load_config()?.settings;
        let client = SshClient::connect(server, settings.connect_timeout_secs).await?;
        let result = async {
            // 退出清理拿不到项目目录，逐个候选根目录都扫一遍（root 只在其中之一）。
            let home = remote_home(&client).await.unwrap_or_default();
            let roots: Vec<String> = [
                (!home.is_empty()).then(|| format!("{home}/.deploycode/containers")),
                Some("/var/tmp/deploycode-containers".to_string()),
            ]
            .into_iter()
            .flatten()
            .collect();
            let mut resumed = false;
            let mut resume_failed = false;
            for root in roots {
                let id_dir = format!("{root}/{record_id}");
                let mark = format!("{root}/{record_id}.pause");
                let (_, text) = client
                    .exec_capture(
                        &format!(
                            "{}; {}; {}; rm -rf {} {}",
                            kill_script(&format!("{id_dir}/snapshot.pid")),
                            kill_script(&format!("{id_dir}/restore.pid")),
                            resume_pause_mark(&mark),
                            shell_quote(&id_dir),
                            shell_quote(&format!("{root}/{record_id}.tar"))
                        ),
                        30,
                    )
                    .await?;
                resumed |= text.contains("DEPLOYCODE_PAUSE_RESUMED");
                resume_failed |= text.contains("DEPLOYCODE_PAUSE_RESUME_FAILED");
            }
            // 起没起来都要说出来：这条记录是取消之后用户唯一还能看到的地方。
            if resumed || resume_failed {
                self.note_source_resume(
                    record_id,
                    if resumed {
                        RESUME_OK_NOTE
                    } else {
                        RESUME_FAILED_NOTE
                    },
                );
            }
            Ok::<(), CoreError>(())
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 把清理时拉起源服务的结果补进那条记录。
    fn note_source_resume(&self, record_id: &str, line: &str) {
        let Ok(mut record) = self.store.find_container_record(record_id) else {
            return;
        };
        // 已经不是「进行中」的说明这一轮早就收敛过了，不必再改它的日志。
        if record.status != DeployStatus::Running {
            return;
        }
        record.log.push_str(line);
        let limit = self
            .store
            .load_config()
            .map(|config| config.settings.container_history_limit)
            .unwrap_or(200);
        let _ = self.store.upsert_container(&record, limit);
    }
}

/// 按快照脚本留下的「欠一次恢复」凭据把源服务拉起来，并把凭据销掉；没有凭据时一声不吭。
///
/// 用 `docker start` 而不是 `compose start`：走到这条路上时手上只有记录 id 和文件里那个项目名，
/// 没有项目目录与 compose 文件（那一次扫描随任务一起被 abort 了），而重新扫一次要在本就只有
/// 十几秒的取消时间预算里多花一个来回。`compose stop` 停的正是这批带
/// `com.docker.compose.project` 标签的容器，按同一个标签起回来就是同一批。
/// 结果靠两行标记回传，由 [`ContainerEngine::note_source_resume`] 落到记录里。
fn resume_pause_mark(mark: &str) -> String {
    let file = shell_quote(mark);
    format!(
        "( if [ -f {file} ]; then \
           p=$(head -n 1 {file} 2>/dev/null); rm -f {file}; ok=0; \
           if [ -n \"$p\" ] && command -v docker >/dev/null 2>&1; then \
             ids=$(docker ps -aq --filter \"label=com.docker.compose.project=$p\" 2>/dev/null); \
             if [ -n \"$ids\" ] && docker start $ids >/dev/null 2>&1; then ok=1; fi; \
           fi; \
           if [ \"$ok\" = \"1\" ]; then echo DEPLOYCODE_PAUSE_RESUMED; \
           else echo DEPLOYCODE_PAUSE_RESUME_FAILED; fi; \
         fi )"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清理时补的那次拉起：没有凭据必须一声不吭，有凭据要把结果回传，并且两边都把凭据销掉
    /// （否则下一次清理同一条记录又会再拉一遍）。
    #[test]
    fn resume_fragment_only_fires_on_the_pause_marker() {
        let fragment = resume_pause_mark("/root/.deploycode/containers/abcd1234.pause");
        assert!(
            fragment.contains("/root/.deploycode/containers/abcd1234.pause"),
            "{fragment}"
        );
        assert!(fragment.contains("label=com.docker.compose.project=$p"), "{fragment}");
        assert!(fragment.contains("docker start $ids"));
        assert!(fragment.contains("DEPLOYCODE_PAUSE_RESUMED"));
        assert!(fragment.contains("DEPLOYCODE_PAUSE_RESUME_FAILED"));
        // 只在文件在场时才动手：`[ -f ... ]` 是这段的唯一入口。
        assert!(fragment.starts_with("( if [ -f "));

        // 整段包在子 shell 里：它跟在 kill_script 之后，不能因为这里判false 就把后面的 rm 带走。
        assert!(fragment.ends_with("fi )"), "{fragment}");
    }
}
