//! 备份执行体：远端脚本上传与执行、导出包下载与轮转。

use std::path::{Path, PathBuf};

use super::db_url::split_database_url;
use super::scripts::{build_backup_script, remote_dump_path};
use super::{BackupEngine, BackupOutcome};
use crate::container::{safe_component, short_id};
use crate::disk;
use crate::error::{CoreError, Result};
use crate::models::{BackupEvent, BackupRecord, DbBackupSource, ServerConfig};
use crate::process::shell_quote;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::human_size;

/// 本地临时脚本的清理守卫：无论正常返回还是 panic 都会删除文件。
struct TempScriptGuard(PathBuf);

impl Drop for TempScriptGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl BackupEngine {
    pub(super) async fn execute(
        &self,
        record: &BackupRecord,
        source: &DbBackupSource,
        target: &str,
        logger: &mut TaskLogger<BackupEvent>,
    ) -> Result<BackupOutcome> {
        let config = self.store.load_config()?;
        let settings = config.settings.clone();
        let server = Store::find_server(&config, &record.server_id)?.clone();
        // 留存开关：0 时脚本跑完照旧把远端导出删掉、本机什么都不留（改动前的行为）。
        let keep = settings.db_bundle_keep > 0;
        // 水位闸只拦控制机（见 `Store::agent_mode`）：笔电上少一道保险，总好过定时备份
        // 突然因为系统盘只剩十几个 GB 而启动即失败。
        if keep && self.store.agent_mode() {
            disk::ensure_room(&self.store.db_bundle_dir(), 0)?;
        }

        let source_label = if source.is_docker() {
            format!("docker 容器 {}", source.container)
        } else {
            "服务器本机 pg_dump".to_string()
        };
        logger.info(format!(
            "服务器: {} ({}@{})",
            server.name, server.username, server.host
        ));
        logger.info(format!("来源数据库: {} · {source_label}", source.database));
        logger.info(format!("目标 schema: {}", source.schema));

        // 连接前先拆分目标连接串与密码，避免出错时遗留 SSH 连接。
        let (target_url, target_password) = split_database_url(target)?;

        let timeout = settings.backup_timeout_secs.max(60);
        logger.info("正在连接服务器 ...");
        let client = SshClient::connect(&server, settings.connect_timeout_secs).await?;
        logger.success(format!("SSH 连接成功 ({})", client.label()));

        let script =
            build_backup_script(record, source, &target_url, target_password.as_deref(), keep);
        let remote_script = format!("/tmp/deploycode-backup-{}.sh", record.id);
        let remote_dump = remote_dump_path(&record.id);
        let result = async {
            let size = match self
                .run_script(
                    &client,
                    &server,
                    settings.connect_timeout_secs,
                    &remote_script,
                    &script,
                    timeout,
                    logger,
                    &record.id,
                )
                .await
            {
                Ok(size) => size,
                // KEEP=1 时脚本刻意放过导出文件（等本机来下载），所以失败路径必须由调用方来删它。
                // 最常见的失败是 STAGE:3/4 导入阶段非零退出 —— 那时全库 gzip 已经躺在源机 /tmp 上，
                // 而控制机那边 `db_bundle_keep` 兜底成 2，等于每晚失败一次就留一份、没人收。
                Err(err) => {
                    if keep {
                        let _ = client
                            .exec_capture(
                                &format!("rm -f {}", shell_quote(&remote_dump)),
                                30,
                            )
                            .await;
                    }
                    return Err(err);
                }
            };
            if !keep {
                return Ok(BackupOutcome {
                    dump_size: size,
                    bundle_path: String::new(),
                });
            }
            let bundle = self.bundle_path(record);
            let pulled = self
                .pull_dump(&client, &remote_dump, size, &bundle, logger)
                .await;
            // 无论下载成败都要删掉远端那份：源服务器不留备份，
            // 下载失败也不能在 /tmp 落一个 GB 级文件等人来收（脚本的 trap 这次特意放过它）。
            let _ = client
                .exec_capture(
                    &format!("rm -f {}", shell_quote(&remote_dump)),
                    30,
                )
                .await;
            let bundle = pulled?;
            Ok(BackupOutcome {
                dump_size: size,
                bundle_path: bundle.to_string_lossy().into_owned(),
            })
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 导出包落盘路径：`<数据目录>/backups/<服务器>-<库>-<schema>-<时间>-<记录前缀>.sql.gz`。
    ///
    /// 带上记录 id 前缀是为了同库同秒也不会撞名（手动补跑和定时撞在一起是可能的）。
    fn bundle_path(&self, record: &BackupRecord) -> PathBuf {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let name = format!(
            "{}-{}-{}-{}-{}.sql.gz",
            safe_component(&record.server_name),
            safe_component(&record.database),
            safe_component(&record.schema),
            stamp,
            short_id(&record.id)
        );
        self.store.db_bundle_dir().join(name)
    }

    /// 把远端导出包下载到本机：先下成 `.part`，大小对上才改名，避免留下一个看起来完整的半截包。
    async fn pull_dump(
        &self,
        client: &SshClient,
        remote: &str,
        expected: u64,
        bundle: &Path,
        logger: &mut TaskLogger<BackupEvent>,
    ) -> Result<PathBuf> {
        if let Some(parent) = bundle.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io_path(parent, e))?;
            // 导出完成才知道真实大小：按体积再查一次，别把盘写到爆（同样只拦控制机）。
            if self.store.agent_mode() {
                disk::ensure_room(parent, expected)?;
            }
        }
        logger.command("正在下载导出包到本机 ...");
        let part = crate::container::bundle_part_path(bundle);
        let mut cleanup = crate::container::PartGuard::new(part.clone());
        let outcome = async {
            client
                .download_file(remote, &part, &mut |received, total| {
                    // 脚本收尾时进度条已经走到 100，这里把它压在 96..100 之间往上走，不往回跳。
                    let percent = if total == 0 {
                        99
                    } else {
                        96 + ((received * 4) / total).min(3) as u8
                    };
                    logger.progress(percent, &format!("下载中 {percent}%"));
                })
                .await?;
            let local = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if expected > 0 && local != expected {
                return Err(CoreError::backup(format!(
                    "导出包下载不完整（远端 {expected} 字节，本机 {local} 字节）"
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // 导出包里是全库数据与建表语句，权限与含密码的脚本同级。
                let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&part, bundle).map_err(|e| CoreError::io_path(bundle, e))?;
            cleanup.disarm();
            Ok::<(), CoreError>(())
        }
        .await;
        // 半截的 .part 由 cleanup 在 Drop 里收掉：Err、被取消（future 被 drop）、panic 三种下场都盖到。
        if let Err(err) = outcome {
            return Err(err);
        }
        logger.success(format!("导出包已保存: {}", bundle.display()));
        Ok(bundle.to_path_buf())
    }

    /// 导出包轮转：同一个（服务器 + 库 + schema）只保留最近 `keep` 个包，窗口之外的删文件
    /// 并把那条记录的 `bundle_path` 抹空（界面按它判断有没有包可恢复，留着悬空路径只会点了才报错）。
    ///
    /// 与容器包的轮转同构，只是分组键换成库 + schema：一台机器上常常有十几个库，
    /// 按服务器分组会让一个库的 nightly 备份把别的库的包挤掉。
    pub(super) fn prune_bundles(
        &self,
        record: &BackupRecord,
        keep: usize,
        limit: usize,
    ) -> Result<Vec<(String, u64)>> {
        if keep == 0 || record.server_id.is_empty() || record.database.is_empty() {
            return Ok(Vec::new());
        }
        let bundle_dir = self.store.db_bundle_dir();
        let mut records = self.store.load_backups()?;
        let mut stale: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.id != record.id
                    && item.server_id == record.server_id
                    && item.database == record.database
                    && item.schema == record.schema
                    && !item.bundle_path.is_empty()
            })
            .map(|(index, _)| index)
            .collect();
        if stale.len() < keep {
            return Ok(Vec::new());
        }
        // started_at 是 "%Y-%m-%d %H:%M:%S"，字典序即时间序；本次任务不在候选里，
        // 所以窗口按 keep - 1 算，刚落地的这个包永远不会被自己挤掉。
        stale.sort_by(|a, b| records[*b].started_at.cmp(&records[*a].started_at));
        let mut touched: Vec<(BackupRecord, PathBuf, String, u64)> = Vec::new();
        for index in stale.into_iter().skip(keep - 1) {
            let path = PathBuf::from(&records[index].bundle_path);
            if path.parent().map(|parent| parent != bundle_dir).unwrap_or(true) {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size = records[index].dump_size;
            records[index].bundle_path = String::new();
            touched.push((records[index].clone(), path, name, size));
        }
        if touched.is_empty() {
            return Ok(Vec::new());
        }
        // 先把记录里的路径抹空，再删文件：反过来做的话，删完文件到写回记录之间崩了会留下一条指向
        // 不存在文件的 bundle_path，界面按它判断「有没有包可恢复」，点了才报错。
        // 现在最坏是文件晚一步删除，变成孤儿包，界面上的孤儿统计能看见。
        for (record, _, _, _) in &touched {
            self.store.upsert_backup(record, limit)?;
        }
        let mut removed = Vec::new();
        for (_, path, name, size) in touched {
            if !path.is_file() {
                continue;
            }
            // 删不掉（被别的进程占着）就留下这个文件：记录里的路径已经抹空，轮转不会再回头看它，
            // 但它会出现在设置页的「未认领的备份包」里，由用户手动清。
            if std::fs::remove_file(&path).is_err() {
                continue;
            }
            removed.push((name, size));
        }
        Ok(removed)
    }

    async fn run_script(
        &self,
        client: &SshClient,
        server: &ServerConfig,
        connect_timeout: u64,
        remote: &str,
        script: &str,
        timeout: u64,
        logger: &mut TaskLogger<BackupEvent>,
        record_id: &str,
    ) -> Result<u64> {
        self.put_script(client, remote, script).await?;

        let mut dump_size: u64 = 0;
        let command = format!("bash {}", shell_quote(remote));
        let result = client
            .exec_stream(&command, timeout, &mut |kind, line| {
                let line = line.trim_end().to_string();
                if let Some(rest) = line.strip_prefix("###STAGE:") {
                    if let Some((index, text)) = rest.split_once(':') {
                        let text = text.trim();
                        if let Ok(stage) = index.trim().parse::<u8>() {
                            logger.info(text);
                            logger.progress(stage_percent(stage), text);
                        }
                    }
                } else if let Some(value) = line.strip_prefix("###SIZE:") {
                    if let Ok(size) = value.trim().parse::<u64>() {
                        dump_size = size;
                        logger.info(format!("压缩包大小: {}", human_size(size)));
                    }
                } else if line == "###DONE" {
                    logger.progress(100, "完成");
                } else if !line.is_empty() {
                    match kind {
                        OutputKind::Stdout => logger.info(&line),
                        OutputKind::Stderr => logger.warn(&line),
                    }
                }
            })
            .await;

        // 无论成功失败都删除远端脚本（脚本自身 trap 也会删，双保险）。
        let _ = client
            .exec_capture(&format!("rm -f {}", shell_quote(remote)), 30)
            .await;

        match result {
            Ok(0) => Ok(dump_size),
            Ok(code) => Err(CoreError::backup(format!(
                "备份脚本执行失败（退出码 {code}），请查看上方日志"
            ))),
            Err(err) => {
                // 超时/连接中断时远端脚本可能仍在执行破坏性的清空/导入，
                // 尽力终止其进程组，避免用户重试时并发写同一 schema。
                // kill_script 会删除 pidfile，这里一并删除脚本与导出文件。
                let pidfile = format!("/tmp/deploycode-backup-{record_id}.pid");
                let dump = remote_dump_path(record_id);
                let kill = format!(
                    "{}; rm -f {} {}",
                    crate::engine::kill_script(&pidfile),
                    shell_quote(&dump),
                    shell_quote(remote)
                );
                // 本地超时那条路上这条会话还活着；半开/断线那条路上它已经死了，
                // 此时原来那句注释承诺的「重新连上」才是唯一出路 —— 所以第一条不通就另起一条，
                // 而不是拿着注定失败的 exec 结果只留一行 warn。
                let terminated = match client.exec_capture(&kill, 15).await {
                    Ok(_) => true,
                    Err(_) => match SshClient::connect(server, connect_timeout).await {
                        Ok(retry) => {
                            let sent = retry.exec_capture(&kill, 15).await.is_ok();
                            retry.disconnect().await;
                            sent
                        }
                        Err(_) => false,
                    },
                };
                if terminated {
                    logger.warn("已尝试终止远端备份脚本，请确认服务器上没有残留进程");
                } else {
                    logger.warn("无法终止远端备份脚本，请在服务器上检查残留进程");
                }
                Err(CoreError::backup(format!("{err}（已尝试终止远端脚本）")))
            }
        }
    }

    /// 上传脚本到服务器（上传前先以 0600 创建，上传后 0700）。
    pub(super) async fn put_script(&self, client: &SshClient, remote: &str, script: &str) -> Result<()> {
        // 从远程路径提取文件名（如 /tmp/backup.sh → backup.sh），失败时默认用 "backup.sh"
        let local = self
            .store
            .prepare_temp_file(remote.rsplit('/').next().unwrap_or("backup.sh"))?;
        // 本地脚本含数据库密码：权限收紧为 0600，并由守卫保证任何路径下都会删除。
        let _guard = TempScriptGuard(local.clone());
        std::fs::write(&local, script).map_err(|e| CoreError::io_path(&local, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| CoreError::io_path(&local, e))?;
        }

        // 脚本含数据库与目标库密码：先以 0600 创建远端文件，避免 SFTP 默认权限窗口内被其他用户读取。
        let quoted = shell_quote(remote);
        let _ = client
            .exec_capture(&format!("umask 077; : > {quoted} && chmod 600 {quoted}"), 30)
            .await;

        let uploaded = client.upload_file(&local, remote, &mut |_, _| {}).await;
        if let Err(err) = uploaded {
            let _ = client
                .exec_capture(&format!("rm -f {quoted}"), 15)
                .await;
            return Err(err);
        }

        let (code, output) = match client
            .exec_capture(&format!("chmod 700 {quoted}"), 30)
            .await
        {
            Ok(value) => value,
            Err(err) => {
                // 权限设置命令本身失败时也要删除含密码的远端脚本，避免残留。
                let _ = client
                    .exec_capture(&format!("rm -f {quoted}"), 15)
                    .await;
                return Err(err);
            }
        };
        if code != 0 {
            let _ = client
                .exec_capture(&format!("rm -f {quoted}"), 15)
                .await;
            return Err(CoreError::backup(format!(
                "设置脚本权限失败: {}",
                output.trim()
            )));
        }
        Ok(())
    }
}

pub(super) fn clean_lines(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn stage_percent(stage: u8) -> u8 {
    match stage {
        1 => 5,
        2 => 60,
        3 => 70,
        4 => 80,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::backup::testutil::record;
    use crate::models::DeployStatus;

    /// 导出包轮转：同一个（服务器 + 库 + schema）只留最近 keep 个，多出来的删文件并把记录里的
    /// `bundle_path` 抹空。本次任务自己不在候选里，所以窗口按 keep - 1 算。
    #[test]
    fn prune_bundles_keeps_the_newest_and_blanks_the_rest() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-db-prune-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::new(&dir));
        let engine = BackupEngine::new(store.clone());
        let bundles = store.db_bundle_dir();
        std::fs::create_dir_all(&bundles).unwrap();
        // 故意放在备份目录之外：轮转不许删用户自己挪走的包。
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let stashed = |dir: &Path, name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, b"gz").unwrap();
            path
        };
        let record_of = |id: &str, database: &str, started_at: &str, path: &Path| {
            let mut item = record();
            item.id = id.to_string();
            item.database = database.to_string();
            item.status = DeployStatus::Success;
            item.started_at = started_at.to_string();
            item.bundle_path = path.to_string_lossy().into_owned();
            item
        };

        let oldest = stashed(&bundles, "oldest.sql.gz");
        let middle = stashed(&bundles, "middle.sql.gz");
        let moved = stashed(&elsewhere, "moved.sql.gz");
        let other_db = stashed(&bundles, "other-db.sql.gz");
        store.upsert_backup(&record_of("r1", "app", "2026-01-01 03:00:00", &oldest), 50).unwrap();
        store.upsert_backup(&record_of("r2", "app", "2026-01-02 03:00:00", &middle), 50).unwrap();
        store.upsert_backup(&record_of("r3", "app", "2026-01-03 03:00:00", &moved), 50).unwrap();
        store.upsert_backup(&record_of("r4", "blog", "2026-01-04 03:00:00", &other_db), 50).unwrap();

        let current_file = stashed(&bundles, "current.sql.gz");
        let current = record_of("r5", "app", "2026-01-05 03:00:00", &current_file);
        let removed = engine.prune_bundles(&current, 2, 50).unwrap();

        let names: Vec<&str> = removed.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["middle.sql.gz", "oldest.sql.gz"]);
        assert!(!oldest.exists() && !middle.exists());
        // 留下的那个是「最近的候选 + 本次」，别的库完全不受影响。
        assert!(moved.exists() && other_db.exists() && current_file.exists());

        let records = store.load_backups().unwrap();
        let path_of = |id: &str| {
            records
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.bundle_path.clone())
                .unwrap_or_default()
        };
        assert!(path_of("r1").is_empty() && path_of("r2").is_empty());
        assert!(path_of("r3").ends_with("moved.sql.gz"), "目录外的包不该被抹空");
        assert!(path_of("r4").ends_with("other-db.sql.gz"));
    }
}
