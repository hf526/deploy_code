//! 数据库备份：把服务器上的 PostgreSQL schema 导出并全量恢复到远端 PostgreSQL。
//!
//! 导出与导入都在目标服务器上完成，本机默认只收一个大小数字：
//! 1. 通过 `pg_dump`（docker exec 或本机命令）导出为压缩的 SQL 文件；
//! 2. 校验压缩文件完整性；
//! 3. 清空并重建目标 schema（默认 public，保留默认角色授权）；
//! 4. `gunzip | psql` 全量导入目标数据库。
//!
//! `settings.db_bundle_keep` 非 0 时多一步：脚本导出后不删远端文件，由本机下载留档
//! （`<数据目录>/backups/`），下载完再把远端那份删掉 —— 源服务器仍然不留副本，
//! 所有备份包集中在执行机上，轮转和水位只需管一处。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::container::{safe_component, short_id};
use crate::disk;
use crate::error::{CoreError, Result};
use crate::models::{
    now_string, AppConfig, BackupEvent, BackupRecord, BackupRequest, BackupTarget, DbBackupSource,
    DeployStatus, ServerConfig, Settings,
};
use crate::process::shell_quote;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::{format_duration, human_size};

/// 本地临时脚本的清理守卫：无论正常返回还是 panic 都会删除文件。
struct TempScriptGuard(PathBuf);

impl Drop for TempScriptGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 一次备份的执行结果。
pub struct BackupOutcome {
    pub dump_size: u64,
    /// 导出包在本机的路径；未开启留存时为空串。
    pub bundle_path: String,
}

/// 备份事件发送端（GUI 转发为 Tauri 事件，CLI 直接打印）。
pub type BackupEventSender = UnboundedSender<BackupEvent>;

/// 已校验并解析好的备份任务。
pub struct PreparedBackup {
    pub record: BackupRecord,
    pub source: DbBackupSource,
    /// 实际使用的目标连接串（含密码，仅用于执行，不写日志）。
    pub target: String,
}

/// 数据库备份引擎。
pub struct BackupEngine {
    store: Arc<Store>,
}

impl BackupEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    /// 校验参数、解析来源与目标，并生成一条「运行中」的备份记录（写入历史）。
    pub fn prepare(&self, req: &BackupRequest) -> Result<PreparedBackup> {
        let config = self.store.load_config()?;
        let resolved = resolve_backup(&config, req)?;

        let record = BackupRecord {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: resolved.server.id.clone(),
            server_name: resolved.server.name.clone(),
            database: resolved.source.database.clone(),
            schema: resolved.source.schema.clone(),
            target_name: resolved.target.name.clone(),
            target: mask_database_url(&resolved.target.url),
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            dump_size: 0,
            bundle_path: String::new(),
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };
        self.store
            .upsert_backup(&record, config.settings.backup_history_limit)?;
        Ok(PreparedBackup {
            record,
            source: resolved.source,
            target: resolved.target.url,
        })
    }

    /// 执行备份流程。无论成功失败都会返回带有最终状态的记录。
    pub async fn run(
        &self,
        mut record: BackupRecord,
        source: DbBackupSource,
        target: String,
        events: Option<BackupEventSender>,
    ) -> BackupRecord {
        let started = Instant::now();
        let mut logger = TaskLogger::new(
            events,
            |level, message| BackupEvent::Log { level, message },
            Some(|percent, message| BackupEvent::Progress { percent, message }),
        );

        logger.send(BackupEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始备份 {} · {} @ {}",
            record.server_name, record.database, record.target
        ));

        let result = self.execute(&record, &source, &target, &mut logger).await;

        match result {
            Ok(outcome) => {
                record.dump_size = outcome.dump_size;
                record.bundle_path = outcome.bundle_path.clone();
                record.status = DeployStatus::Success;
                logger.success(format!(
                    "备份成功（{}，耗时 {}）",
                    human_size(outcome.dump_size),
                    format_duration(started.elapsed().as_millis() as u64)
                ));
                if !outcome.bundle_path.is_empty() {
                    logger.info(format!("导出包已留存: {}", outcome.bundle_path));
                }
            }
            Err(err) => {
                record.status = DeployStatus::Failed;
                record.error = Some(err.to_string());
                logger.error(format!("备份失败: {err}"));
            }
        }

        let settings = self
            .store
            .load_config()
            .map(|config| config.settings)
            .unwrap_or_default();
        let limit = settings.backup_history_limit;

        // 只有成功收尾才轮转：失败那次的半截包不该占住窗口，更不该顺手把上一晚的好包挤掉。
        if record.status == DeployStatus::Success {
            match self.prune_bundles(&record, settings.db_bundle_keep, limit) {
                Ok(removed) => {
                    for (name, _) in &removed {
                        logger.info(format!("清理旧导出包 {name}"));
                    }
                    if !removed.is_empty() {
                        let freed: u64 = removed.iter().map(|(_, size)| size).sum();
                        logger.success(format!(
                            "本轮清理 {} 个旧导出包，释放 {}",
                            removed.len(),
                            human_size(freed)
                        ));
                    }
                }
                Err(err) => logger.warn(format!("导出包轮转失败: {err}")),
            }
        }

        record.log = logger.joined();
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        if let Err(err) = self.store.upsert_backup(&record, limit) {
            logger.error(format!("保存备份记录失败: {err}"));
            record.log = logger.joined();
        }

        if let Some(sender) = logger.take_events() {
            let _ = sender.send(BackupEvent::Finished {
                record: record.clone(),
            });
        }
        record
    }

    async fn execute(
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
            let size = self
                .run_script(
                    &client,
                    &remote_script,
                    &script,
                    timeout,
                    logger,
                    &record.id,
                )
                .await?;
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
    fn prune_bundles(
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
        let mut removed = Vec::new();
        let mut touched: Vec<BackupRecord> = Vec::new();
        for index in stale.into_iter().skip(keep - 1) {
            let path = PathBuf::from(&records[index].bundle_path);
            if path.parent().map(|parent| parent != bundle_dir).unwrap_or(true) {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.is_file() {
                // 删不掉（被别的进程占着）就原样留着这条，下次任务收尾时再试。
                if std::fs::remove_file(&path).is_err() {
                    continue;
                }
                removed.push((name, records[index].dump_size));
            }
            records[index].bundle_path = String::new();
            touched.push(records[index].clone());
        }
        for item in touched {
            self.store.upsert_backup(&item, limit)?;
        }
        Ok(removed)
    }

    async fn run_script(
        &self,
        client: &SshClient,
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
                // 尽力重新连上并终止其进程组，避免用户重试时并发写同一 schema。
                // kill_script 会删除 pidfile，这里一并删除脚本与导出文件。
                let pidfile = format!("/tmp/deploycode-backup-{record_id}.pid");
                let dump = remote_dump_path(record_id);
                let kill = format!(
                    "{}; rm -f {} {}",
                    crate::engine::kill_script(&pidfile),
                    shell_quote(&dump),
                    shell_quote(remote)
                );
                match client.exec_capture(&kill, 15).await {
                    Ok(_) => logger.warn("已尝试终止远端备份脚本，请确认服务器上没有残留进程"),
                    Err(_) => logger.warn("无法终止远端备份脚本，请在服务器上检查残留进程"),
                }
                Err(CoreError::backup(format!("{err}（已尝试终止远端脚本）")))
            }
        }
    }

    /// 上传脚本到服务器（上传前先以 0600 创建，上传后 0700）。
    async fn put_script(&self, client: &SshClient, remote: &str, script: &str) -> Result<()> {
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

    /// 检查服务器侧备份环境：pg_dump 可用性与 Supabase 连通性。
    /// 与真实备份使用同一套来源/目标解析优先级。
    pub async fn test(&self, req: &BackupRequest) -> Result<String> {
        let config = self.store.load_config()?;
        let resolved = resolve_backup(&config, req)?;
        let server = resolved.server;
        let source = resolved.source;
        let (target_url, target_password) = split_database_url(&resolved.target.url)?;

        let settings = &config.settings;
        let client = SshClient::connect(&server, settings.connect_timeout_secs).await?;
        let script = build_test_script(&source, &target_url, target_password.as_deref());
        let remote = format!("/tmp/deploycode-backup-test-{}.sh", uuid::Uuid::new_v4());

        let result = async {
            self.put_script(&client, &remote, &script).await?;
            let run = client
                .exec_capture(&format!("bash {}", shell_quote(&remote)), 120)
                .await;
            let _ = client
                .exec_capture(&format!("rm -f {}", shell_quote(&remote)), 30)
                .await;
            let (code, output) = run?;
            if code != 0 {
                return Err(CoreError::backup(format!(
                    "环境检查失败: {}",
                    clean_lines(&output)
                )));
            }
            Ok(clean_lines(&output))
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 应用退出时清理仍在执行的远端备份脚本（重新连接并终止进程，删除脚本与导出文件）。
    pub async fn cleanup_remote_script(
        &self,
        server: &ServerConfig,
        record_id: &str,
    ) -> Result<()> {
        let settings = self.store.load_config()?.settings;
        let pidfile = format!("/tmp/deploycode-backup-{record_id}.pid");
        let script = format!("/tmp/deploycode-backup-{record_id}.sh");
        let dump = format!("/tmp/deploycode-backup-{record_id}.sql.gz");
        let command = format!(
            "{}; rm -f {} {}",
            crate::engine::kill_script(&pidfile),
            shell_quote(&script),
            shell_quote(&dump)
        );
        let client = SshClient::connect(server, settings.connect_timeout_secs).await?;
        let result = client.exec_capture(&command, 15).await;
        client.disconnect().await;
        result.map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// 校验与解析
// ---------------------------------------------------------------------------

impl DbBackupSource {
    fn is_docker(&self) -> bool {
        self.mode.eq_ignore_ascii_case("docker")
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalize_source(mut source: DbBackupSource) -> Result<DbBackupSource> {
    source.mode = source.mode.trim().to_lowercase();
    if source.mode.is_empty() {
        source.mode = "docker".to_string();
    }
    source.container = source.container.trim().to_string();
    source.database = source.database.trim().to_string();
    source.username = source.username.trim().to_string();
    source.password = source.password.trim().to_string();
    source.schema = source.schema.trim().to_string();
    if source.schema.is_empty() {
        source.schema = "public".to_string();
    }
    Ok(source)
}

fn validate_source(source: &DbBackupSource) -> Result<()> {
    if source.mode != "docker" && source.mode != "system" {
        return Err(CoreError::config(format!(
            "不支持的采集方式: {}（可选 docker / system）",
            source.mode
        )));
    }
    if source.is_docker() && source.container.is_empty() {
        return Err(CoreError::config("docker 模式需要填写容器名"));
    }
    if source.database.is_empty() {
        return Err(CoreError::config("数据库名不能为空"));
    }
    if source.username.is_empty() {
        return Err(CoreError::config("数据库用户名不能为空"));
    }
    if source.schema.contains('"') || source.schema.chars().any(char::is_control) {
        return Err(CoreError::config(
            "schema 名称包含非法字符（不能含双引号或换行等控制字符）",
        ));
    }
    // reset SQL 使用 $do$ 作为 dollar-quote 标签，schema 中出现同名标签会截断 DO 块。
    if source.schema.contains("$do$") {
        return Err(CoreError::config("schema 名称不能包含 $do$"));
    }
    Ok(())
}

/// 已解析的备份任务来源（服务器 + 来源 + 目标）。
struct ResolvedBackup {
    server: ServerConfig,
    source: DbBackupSource,
    target: ResolvedTarget,
}

/// 解析一次备份请求：
/// - 指定了 `backup_config_id` 时，服务器 / 来源 / 目标取自保存的配置；
/// - 否则沿用服务器上的旧版单份来源配置；
/// - `source` 字段可直接覆盖来源（测试未保存的表单），`database` / `schema` 再覆盖对应字段。
fn resolve_backup(config: &AppConfig, req: &BackupRequest) -> Result<ResolvedBackup> {
    let (server, source, config_target_id, config_url) =
        match non_empty(&req.backup_config_id) {
            Some(key) => {
                let saved = Store::find_backup_config(config, key)?;
                (
                    Store::find_server(config, &saved.server_id)?.clone(),
                    req.source.clone().unwrap_or_else(|| saved.source.clone()),
                    saved.target_id.clone(),
                    saved.supabase_url.clone(),
                )
            }
            None => {
                let server = Store::find_server(config, &req.server_id)?.clone();
                let source = req
                    .source
                    .clone()
                    .or_else(|| server.db_backup.clone())
                    .ok_or_else(|| CoreError::config("请先为该服务器配置数据库备份来源"))?;
                (server, source, None, None)
            }
        };

    let mut source = normalize_source(source)?;
    if let Some(database) = non_empty(&req.database) {
        source.database = database.to_string();
    }
    if let Some(schema) = non_empty(&req.schema) {
        source.schema = schema.to_string();
    }
    validate_source(&source)?;

    // 请求参数（连接串 / 目标）> 配置自身的覆盖目标 > 服务器 / 全局默认。
    // 必须按来源区分：请求里显式指定的目标不能被配置里保存的连接串压过，反之亦然。
    let target = resolve_target(
        &config.backup_targets,
        &server,
        &config.settings,
        &req.target_id,
        &req.supabase_url,
        &config_target_id,
        &config_url,
    )?;

    Ok(ResolvedBackup {
        server,
        source,
        target,
    })
}

/// 已解析的备份目标。
struct ResolvedTarget {
    name: String,
    url: String,
}

/// 解析备份目标，优先级：
/// 请求直接连接串 > 请求指定目标 > 配置连接串 > 配置指定目标 >
/// 服务器绑定目标 > 服务器自定义连接串 > 全局默认目标 > 旧版全局连接串。
fn resolve_target(
    targets: &[BackupTarget],
    server: &ServerConfig,
    settings: &Settings,
    request_target_id: &Option<String>,
    request_url: &Option<String>,
    config_target_id: &Option<String>,
    config_url: &Option<String>,
) -> Result<ResolvedTarget> {
    if let Some(url) = non_empty(request_url) {
        return Ok(ResolvedTarget {
            name: "自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(request_target_id) {
        return find_target(targets, key);
    }
    if let Some(url) = non_empty(config_url) {
        return Ok(ResolvedTarget {
            name: "自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(config_target_id) {
        return find_target(targets, key);
    }
    if let Some(key) = non_empty(&server.backup_target_id) {
        return find_target(targets, key);
    }
    if let Some(url) = non_empty(&server.supabase_url) {
        return Ok(ResolvedTarget {
            name: "服务器自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(&settings.default_backup_target_id) {
        return find_target(targets, key);
    }
    let legacy = settings.supabase_url.trim();
    if !legacy.is_empty() {
        return Ok(ResolvedTarget {
            name: "旧版连接串".to_string(),
            url: validate_target_url(legacy)?,
        });
    }
    Err(CoreError::config(
        "请先在设置页配置备份目标（数据库备份 → 备份目标）",
    ))
}

fn find_target(targets: &[BackupTarget], key: &str) -> Result<ResolvedTarget> {
    let target = targets
        .iter()
        .find(|target| target.id == key || target.name == key)
        .ok_or_else(|| CoreError::config(format!("备份目标不存在: {key}")))?;
    Ok(ResolvedTarget {
        name: target.name.clone(),
        url: validate_target_url(&target.url)?,
    })
}

fn validate_target_url(url: &str) -> Result<String> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Ok(url.to_string())
    } else {
        Err(CoreError::config(
            "备份目标连接串必须以 postgres:// 或 postgresql:// 开头",
        ))
    }
}

/// 定位连接串 authority 的结束位置，以及 userinfo 分界 `@` 的位置（如果有）。
///
/// 常规情况：authority 截止到第一个 `/`、`?`、`#`。
/// 密码里可能出现未编码的 `/`（如 `user:p/ss@host`），此时第一个 `/` 之前没有 `@`，
/// 但查询 / 片段之前存在 `@`。
///
/// `assume_late_at_is_userinfo` 决定这种歧义输入的取舍：
/// - `true`（脱敏展示）：一律当作 userinfo，宁可多遮蔽也不能漏出密码明文；
///   代价是 `host:5432/db@name` 会被显示成 `host:***@name`。
/// - `false`（连接串解析）：只有第一个 `/` 前的部分不像 `host[:port]` 时才当作 userinfo，
///   且取第一个 `@`（避免密码含 `/` 且路径也含 `@` 时把路径的 `@` 当成分界）。
///   歧义输入保持原样交给 psql，宁可报错也不要连到错误的主机 / 库。
fn locate_authority(rest: &str, assume_late_at_is_userinfo: bool) -> (usize, Option<usize>) {
    let first_sep = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host_end_after = |at: usize| {
        rest[at + 1..]
            .find(['/', '?', '#'])
            .map(|offset| at + 1 + offset)
            .unwrap_or(rest.len())
    };

    if let Some(at) = rest[..first_sep].rfind('@') {
        return (host_end_after(at), Some(at));
    }

    let query_limit = rest.find(['?', '#']).unwrap_or(rest.len());
    let late = if assume_late_at_is_userinfo {
        rest[..query_limit].rfind('@')
    } else {
        rest[first_sep..query_limit]
            .find('@')
            .map(|offset| first_sep + offset)
    };
    match late {
        Some(at) if assume_late_at_is_userinfo || !looks_like_host_port(&rest[..first_sep]) => {
            (host_end_after(at), Some(at))
        }
        _ => (first_sep, None),
    }
}

/// 判断 authority 片段是否像 `host[:port]`（含 IPv6）：端口必须是纯数字。
fn looks_like_host_port(prefix: &str) -> bool {
    if let Some(rest) = prefix.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let tail = &rest[end + 1..];
        return tail.is_empty()
            || tail
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    }
    match prefix.split_once(':') {
        Some((_, port)) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
        None => true,
    }
}

/// 隐藏连接串中的密码（userinfo 与查询参数），用于日志与记录展示。
pub fn mask_database_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..scheme_end];
    let rest = &url[scheme_end + 3..];

    // 脱敏按「宁可多遮蔽」处理：不能因为无法区分而漏出密码明文。
    let (authority_end, at) = locate_authority(rest, true);
    let authority = match at {
        Some(at) => {
            let creds = &rest[..at];
            let user = creds.split(':').next().unwrap_or(creds);
            let host = &rest[at + 1..authority_end];
            if creds.contains(':') {
                format!("{user}:***@{host}")
            } else {
                format!("{user}@{host}")
            }
        }
        None => rest[..authority_end].to_string(),
    };

    format!(
        "{scheme}://{authority}{}",
        mask_query_password(&rest[authority_end..])
    )
}

/// 把查询串中的 `password=...` 值替换为 `***`，保留其他参数。
fn mask_query_password(remainder: &str) -> String {
    let Some(question) = remainder.find('?') else {
        return remainder.to_string();
    };
    let hash = remainder.find('#').unwrap_or(remainder.len());
    if question > hash {
        return remainder.to_string();
    }
    let prefix = &remainder[..question];
    let query = &remainder[question + 1..hash];
    let fragment = &remainder[hash..];
    let masked = query
        .split('&')
        .map(|param| {
            let key = param.split('=').next().unwrap_or(param);
            if key.eq_ignore_ascii_case("password") && param.contains('=') {
                format!("{key}=***")
            } else {
                param.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{prefix}?{masked}{fragment}")
}

/// 从连接串中拆分出「去掉密码的连接串」与「密码」。
///
/// 密码可能来自 userinfo（`postgres://user:pass@host/db`）或查询参数
/// （`...?password=pass`）。psql 通过 PGPASSWORD 读取原始密码，
/// 因此去掉密码的连接串可安全地放进命令行/进程环境，不再暴露给 `ps`。
pub fn split_database_url(url: &str) -> Result<(String, Option<String>)> {
    let scheme_end = url
        .find("://")
        .ok_or_else(|| CoreError::config("连接串格式无效"))?;
    let scheme = &url[..scheme_end];
    if scheme != "postgres" && scheme != "postgresql" {
        return Err(CoreError::config(
            "备份目标连接串必须以 postgres:// 或 postgresql:// 开头",
        ));
    }
    let rest = &url[scheme_end + 3..];
    // 歧义输入（如密码含未编码 '/'）保守处理：拿不准就不剥离密码，让 psql 报错而不是连错库。
    let (authority_end, _) = locate_authority(rest, false);
    let authority = &rest[..authority_end];
    let remainder = &rest[authority_end..];

    // 查询参数里的 password 先取出并从连接串移除；userinfo 中的密码优先级更高。
    let (remainder, query_password) = strip_query_password(remainder);

    let mut password = query_password;
    let authority = match authority.rfind('@') {
        Some(at) => {
            let userinfo = &authority[..at];
            let host = &authority[at + 1..];
            match userinfo.find(':') {
                Some(colon) => {
                    password = Some(percent_decode(&userinfo[colon + 1..]));
                    format!("{}@{host}", &userinfo[..colon])
                }
                None => authority.to_string(),
            }
        }
        None => authority.to_string(),
    };

    Ok((format!("{scheme}://{authority}{remainder}"), password))
}

/// 移除查询串中的 `password` 参数，返回新的 remainder 与解码后的密码。
fn strip_query_password(remainder: &str) -> (String, Option<String>) {
    let Some(question) = remainder.find('?') else {
        return (remainder.to_string(), None);
    };
    let hash = remainder.find('#').unwrap_or(remainder.len());
    if question > hash {
        return (remainder.to_string(), None);
    }
    let prefix = &remainder[..question];
    let query = &remainder[question + 1..hash];
    let fragment = &remainder[hash..];

    let mut password = None;
    let mut kept: Vec<&str> = Vec::new();
    for param in query.split('&') {
        let key = param.split('=').next().unwrap_or(param);
        if key.eq_ignore_ascii_case("password") {
            if password.is_none() {
                password = param
                    .split_once('=')
                    .map(|(_, value)| percent_decode(value));
            }
        } else {
            kept.push(param);
        }
    }

    let mut rebuilt = prefix.to_string();
    if !kept.is_empty() {
        rebuilt.push('?');
        rebuilt.push_str(&kept.join("&"));
    }
    rebuilt.push_str(fragment);
    (rebuilt, password)
}

/// 百分号解码（`%XX` 十六进制）；非 UTF-8 字节按 lossy 处理。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn clean_lines(output: &str) -> String {
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

// ---------------------------------------------------------------------------
// 远端脚本
// ---------------------------------------------------------------------------

/// 备份脚本模板：来源导出 -> 校验 -> 清空目标 schema -> 导入。
const BACKUP_TEMPLATE: &str = r##"#!/usr/bin/env bash
set -euo pipefail
umask 077

OUT="/tmp/deploycode-backup-__ID__.sql.gz"
PID_FILE="/tmp/deploycode-backup-__ID__.pid"
export TARGET_URL=__TARGET_URL__
export TARGET_PASSWORD=__TARGET_PASSWORD__
MODE=__MODE__
CONTAINER=__CONTAINER__
DB_NAME=__DB_NAME__
DB_USER=__DB_USER__
DB_PASSWORD=__DB_PASSWORD__
SCHEMA=__SCHEMA__
KEEP=__KEEP__

# KEEP=1 时把导出留给调用方下载（下载完由调用方删），否则照旧一走了之。
cleanup() {
  if [ "$KEEP" = "1" ]; then
    rm -f "$PID_FILE" "$0"
  else
    rm -f "$OUT" "$PID_FILE" "$0"
  fi
}
trap cleanup EXIT INT TERM
echo $$ > "$PID_FILE"

# Supabase 等托管服务在公网访问，需要 psql 客户端：优先用服务器本机的，退回数据库容器自带的。
if command -v psql >/dev/null 2>&1; then
  USE_DOCKER_PSQL=0
elif [ "$MODE" = "docker" ] && docker exec "$CONTAINER" sh -c 'command -v psql >/dev/null 2>&1'; then
  USE_DOCKER_PSQL=1
else
  echo '目标端缺少 psql 客户端：请安装 postgresql-client，或使用 docker 模式（PG 镜像自带 psql）' >&2
  exit 1
fi

run_psql() {
  if [ "$USE_DOCKER_PSQL" = "1" ]; then
    docker exec -i -e TARGET_URL -e PGPASSWORD "$CONTAINER" sh -c 'psql "$TARGET_URL" "$@"' psql "$@"
  else
    psql "$TARGET_URL" "$@"
  fi
}

echo '###STAGE:1:正在导出数据库' "$DB_NAME" '...'
export PGPASSWORD="$DB_PASSWORD"
if [ "$MODE" = "docker" ]; then
  docker exec -e PGPASSWORD "$CONTAINER" pg_dump -U "$DB_USER" -d "$DB_NAME" -n "$SCHEMA" --no-owner --no-acl
else
  pg_dump -U "$DB_USER" -d "$DB_NAME" -n "$SCHEMA" --no-owner --no-acl
fi | gzip > "$OUT"
unset PGPASSWORD

export PGPASSWORD="$TARGET_PASSWORD"
SIZE=$(stat -c%s "$OUT" 2>/dev/null || wc -c < "$OUT")
printf '###SIZE:%s\n' "$SIZE"
echo '###STAGE:2:正在校验备份文件 ...'
gzip -t "$OUT"

echo '###STAGE:3:正在清空目标 schema ...'
run_psql -v ON_ERROR_STOP=1 -q <<'DEPLOYCODE_SQL'
__RESET_SQL__
DEPLOYCODE_SQL

echo '###STAGE:4:正在导入到目标数据库 ...'
# pg_dump -n 在较新版本（PG15+，或 public 的属主/注释被改过）会输出 CREATE SCHEMA public;
# 目标 schema 已由上面的重置步骤创建并授权，直接导入会因 "schema already exists" 中断（ON_ERROR_STOP）。
# 这里只过滤「紧跟 Schema TOC 注释的那条 CREATE SCHEMA」，避免误删数据里同名的文本行；
# COPY 数据段原样放行，避免数据里恰好出现形似 TOC 注释的行时被误删。
gunzip -c "$OUT" | awk '
  in_copy && /^\\\.$/ { in_copy = 0; print; next }
  in_copy { print; next }
  /^COPY .* FROM stdin;$/ { in_copy = 1; print; next }
  /^-- Name: / { schema_toc = ($0 ~ /; Type: SCHEMA;/) }
  schema_toc && /^CREATE SCHEMA / { schema_toc = 0; next }
  schema_toc && /^[^-[:space:]]/ { schema_toc = 0 }
  { print }
' | run_psql -v ON_ERROR_STOP=1 -q

echo '###DONE'
"##;

/// 环境检查脚本：pg_dump 版本 + Supabase 连接测试。
const TEST_TEMPLATE: &str = r##"#!/usr/bin/env bash
set -euo pipefail

export TARGET_URL=__TARGET_URL__
export TARGET_PASSWORD=__TARGET_PASSWORD__
export PGPASSWORD="$TARGET_PASSWORD"
MODE=__MODE__
CONTAINER=__CONTAINER__
trap 'rm -f "$0"' EXIT INT TERM

echo -n "pg_dump: "
if [ "$MODE" = "docker" ]; then
  docker exec "$CONTAINER" pg_dump --version
else
  pg_dump --version
fi

echo -n "目标数据库: "
if command -v psql >/dev/null 2>&1; then
  psql "$TARGET_URL" -Atc 'select current_database() || chr(64) || current_user'
elif docker exec "$CONTAINER" sh -c 'command -v psql >/dev/null 2>&1'; then
  docker exec -i -e TARGET_URL -e PGPASSWORD "$CONTAINER" sh -c 'psql "$TARGET_URL" -Atc "select current_database() || chr(64) || current_user"'
else
  echo '缺少 psql 客户端' >&2
  exit 1
fi
"##;

/// 远端导出文件路径。脚本模板里是字面量，这里拼一份给下载与善后用 —— 两处必须同规则。
fn remote_dump_path(record_id: &str) -> String {
    format!("/tmp/deploycode-backup-{record_id}.sql.gz")
}

fn build_backup_script(
    record: &BackupRecord,
    source: &DbBackupSource,
    target_url: &str,
    target_password: Option<&str>,
    keep: bool,
) -> String {
    render_template(
        BACKUP_TEMPLATE,
        &[
            ("__ID__", record.id.clone()),
            ("__KEEP__", if keep { "1" } else { "0" }.to_string()),
            ("__TARGET_URL__", shell_quote(target_url)),
            // 目标密码可选：配置中未设置时为空字符串（不影响执行）
            (
                "__TARGET_PASSWORD__",
                shell_quote(target_password.unwrap_or("")),
            ),
            ("__MODE__", shell_quote(&source.mode)),
            ("__CONTAINER__", shell_quote(&source.container)),
            ("__DB_NAME__", shell_quote(&source.database)),
            ("__DB_USER__", shell_quote(&source.username)),
            ("__DB_PASSWORD__", shell_quote(&source.password)),
            ("__SCHEMA__", shell_quote(&source.schema)),
            ("__RESET_SQL__", reset_schema_sql(&source.schema)),
        ],
    )
}

fn build_test_script(
    source: &DbBackupSource,
    target_url: &str,
    target_password: Option<&str>,
) -> String {
    render_template(
        TEST_TEMPLATE,
        &[
            ("__TARGET_URL__", shell_quote(target_url)),
            // 目标密码可选：配置中未设置时为空字符串（不影响执行）
            (
                "__TARGET_PASSWORD__",
                shell_quote(target_password.unwrap_or("")),
            ),
            ("__MODE__", shell_quote(&source.mode)),
            ("__CONTAINER__", shell_quote(&source.container)),
        ],
    )
}

/// 单遍渲染模板占位符：替换结果不再被扫描，避免值中的 `__KEY__` 触发二次替换。
pub(crate) fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("__") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("__") else {
            output.push_str(&rest[start..]);
            return output;
        };
        // 占位符整体（含两侧下划线）与 values 中的键匹配。
        let placeholder = &rest[start..start + 2 + end + 2];
        match values.iter().find(|(name, _)| *name == placeholder) {
            Some((_, value)) => output.push_str(value),
            None => output.push_str(placeholder),
        }
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    output
}

/// 生成「清空并重建 schema」SQL；重建后恢复 Supabase 默认角色授权。
fn reset_schema_sql(schema: &str) -> String {
    let ident = sql_ident(schema);
    let base = format!(
        "DROP SCHEMA IF EXISTS {ident} CASCADE;\n\
         CREATE SCHEMA {ident};\n\
         GRANT ALL ON SCHEMA {ident} TO PUBLIC;"
    );
    // 标识符内嵌到 EXECUTE 的字符串字面量时，只需转义单引号（双引号是标识符本身的定界符）。
    let literal = ident.replace('\'', "''");
    format!(
        "{base}\n\
         DO $do$\n\
         BEGIN\n\
         \x20 IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN\n\
         \x20   EXECUTE 'GRANT USAGE ON SCHEMA {literal} TO anon, authenticated, service_role';\n\
         \x20   EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA {literal} GRANT ALL ON TABLES TO anon, authenticated, service_role';\n\
         \x20   EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA {literal} GRANT ALL ON SEQUENCES TO anon, authenticated, service_role';\n\
         \x20   EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA {literal} GRANT ALL ON FUNCTIONS TO anon, authenticated, service_role';\n\
         \x20 END IF;\n\
         END\n\
         $do$;"
    )
}

fn sql_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DbBackupSource {
        DbBackupSource {
            mode: "docker".to_string(),
            container: "postgres".to_string(),
            database: "app".to_string(),
            username: "postgres".to_string(),
            password: "p'ass".to_string(),
            schema: "public".to_string(),
        }
    }

    fn record() -> BackupRecord {
        BackupRecord {
            id: "abc".to_string(),
            server_id: "s1".to_string(),
            server_name: "prod".to_string(),
            database: "app".to_string(),
            schema: "public".to_string(),
            target_name: "Supabase".to_string(),
            target: "postgresql://postgres:***@db.example.com:5432/postgres".to_string(),
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            dump_size: 0,
            bundle_path: String::new(),
            started_at: String::new(),
            finished_at: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn mask_database_url_hides_password() {
        assert_eq!(
            mask_database_url("postgresql://postgres.abc:secret@aws-0.pooler.supabase.com:5432/postgres"),
            "postgresql://postgres.abc:***@aws-0.pooler.supabase.com:5432/postgres"
        );
        assert_eq!(
            mask_database_url("postgresql://postgres:p@ss@host:5432/db"),
            "postgresql://postgres:***@host:5432/db"
        );
        assert_eq!(
            mask_database_url("postgresql://postgres@host:5432/db"),
            "postgresql://postgres@host:5432/db"
        );
        // 查询参数里的密码同样要隐藏。
        assert_eq!(
            mask_database_url("postgresql://u@host:5432/db?password=secret&sslmode=require"),
            "postgresql://u@host:5432/db?password=***&sslmode=require"
        );
        assert_eq!(
            mask_database_url("postgresql://u@host:5432/db?a=1&Password=secret#frag"),
            "postgresql://u@host:5432/db?a=1&Password=***#frag"
        );
        // 密码含未编码 '/' 时也必须遮蔽（不能按第一个 '/' 截断 authority）。
        assert_eq!(
            mask_database_url("postgresql://user:p/ss@host:5432/db"),
            "postgresql://user:***@host:5432/db"
        );
        // 密码前段是纯数字时同样必须遮蔽（宁可把路径里的 '@' 一起遮掉，也不能漏明文）。
        assert_eq!(
            mask_database_url("postgresql://admin:888888/password@host/db"),
            "postgresql://admin:***@host/db"
        );
        assert_eq!(
            mask_database_url("postgresql://host:5432/db@name"),
            "postgresql://host:***@name"
        );
        // 查询参数里的 '@' 不应被当作 host 分界。
        assert_eq!(
            mask_database_url("postgresql://host/db?user=a@b&password=s@cret"),
            "postgresql://host/db?user=a@b&password=***"
        );
        assert_eq!(mask_database_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn split_database_url_extracts_password() {
        let (url, password) = split_database_url("postgresql://u:secret@h:5432/db").unwrap();
        assert_eq!(url, "postgresql://u@h:5432/db");
        assert_eq!(password.as_deref(), Some("secret"));
        assert!(!url.contains("secret"));

        // 密码中的特殊字符先做百分号解码，交给 PGPASSWORD 的是原始值。
        let (url, password) = split_database_url("postgres://u:p%40ss%3Aword@h/db").unwrap();
        assert_eq!(url, "postgres://u@h/db");
        assert_eq!(password.as_deref(), Some("p@ss:word"));

        // 密码含未编码 '/' 时不能按第一个 '/' 截断（与 mask_database_url 行为一致）。
        let (url, password) = split_database_url("postgresql://user:p/ss@host:5432/db").unwrap();
        assert_eq!(url, "postgresql://user@host:5432/db");
        assert_eq!(password.as_deref(), Some("p/ss"));

        // 路径里的 '@'（前缀是 host:port）不能被当成 userinfo。
        let (url, password) = split_database_url("postgresql://host:5432/db@name").unwrap();
        assert_eq!(url, "postgresql://host:5432/db@name");
        assert!(password.is_none());

        // 密码含 '/' 且路径也含 '@'：取第一个 '@' 做分界，不能把路径的 '@' 当分界。
        let (url, password) =
            split_database_url("postgresql://user:p/ss@host:5432/db@name").unwrap();
        assert_eq!(url, "postgresql://user@host:5432/db@name");
        assert_eq!(password.as_deref(), Some("p/ss"));

        // 无法区分「密码含 '/'」与「host:port + 路径」时保守处理：不剥离密码，交给 psql 报错。
        let (url, password) = split_database_url("postgresql://admin:888888/password@host/db").unwrap();
        assert_eq!(url, "postgresql://admin:888888/password@host/db");
        assert!(password.is_none());

        // 查询参数中的 password 也会被提取并从连接串移除。
        let (url, password) =
            split_database_url("postgresql://u@h:5432/db?password=se%20cret&sslmode=require")
                .unwrap();
        assert_eq!(url, "postgresql://u@h:5432/db?sslmode=require");
        assert_eq!(password.as_deref(), Some("se cret"));

        // 唯一的参数被移除后不残留 '?'。
        let (url, password) = split_database_url("postgresql://u@h/db?Password=secret").unwrap();
        assert_eq!(url, "postgresql://u@h/db");
        assert_eq!(password.as_deref(), Some("secret"));

        // 中间参数被移除后其余参数与 fragment 正常拼接。
        let (url, _) = split_database_url("postgresql://u@h/db?a=1&password=x&b=2#frag").unwrap();
        assert_eq!(url, "postgresql://u@h/db?a=1&b=2#frag");

        // userinfo 中的密码优先于查询参数。
        let (url, password) =
            split_database_url("postgresql://u:first@h/db?password=second").unwrap();
        assert_eq!(url, "postgresql://u@h/db");
        assert_eq!(password.as_deref(), Some("first"));

        // 没有密码时保持原样。
        let (url, password) = split_database_url("postgresql://u@h:5432/db").unwrap();
        assert_eq!(url, "postgresql://u@h:5432/db");
        assert!(password.is_none());

        assert!(split_database_url("mysql://u:p@h/db").is_err());
        assert!(split_database_url("not-a-url").is_err());
    }

    #[test]
    fn validate_source_rejects_bad_mode_and_empty_fields() {
        let mut bad = source();
        bad.mode = "k8s".to_string();
        assert!(validate_source(&bad).is_err());

        let mut bad = source();
        bad.container = "  ".to_string();
        assert!(validate_source(&normalize_source(bad).unwrap()).is_err());

        let mut bad = source();
        bad.database = String::new();
        assert!(validate_source(&bad).is_err());
    }

    #[test]
    fn script_quotes_credentials_and_keeps_stage_markers() {
        let script = build_backup_script(
            &record(),
            &source(),
            "postgresql://u@h:5432/db",
            Some("secret"),
            false,
        );
        assert!(script.contains("###STAGE:1"));
        assert!(script.contains("###STAGE:4"));
        assert!(script.contains("###DONE"));
        assert!(
            script.contains("DB_PASSWORD='p'\\''ass'"),
            "password not quoted: {script}"
        );
        assert!(script.contains(
            "pg_dump -U \"$DB_USER\" -d \"$DB_NAME\" -n \"$SCHEMA\" --no-owner --no-acl"
        ));
        assert!(script.contains("docker exec -e PGPASSWORD"));
        // 查询数据库后必须先撤下来源库密码，再为导入阶段设置目标库密码。
        assert!(script.contains("unset PGPASSWORD"));
        assert!(script.contains("export PGPASSWORD=\"$TARGET_PASSWORD\""));
        assert!(script.contains("docker exec -i -e TARGET_URL -e PGPASSWORD"));
        // dump 文件位于 /tmp，需要收紧权限。
        assert!(script.contains("umask 077"));
        // 目标密码不再出现在连接串里，只通过环境变量传递。
        let target_line = script
            .lines()
            .find(|line| line.starts_with("export TARGET_URL="))
            .unwrap();
        assert!(target_line.contains("u@h:5432/db"));
        assert!(
            !target_line.contains("secret"),
            "password leaked into url: {target_line}"
        );
        assert!(script.contains("export TARGET_PASSWORD=secret"));
        // 数据库名不得插入单引号字符串内部（防止引号错位/命令注入）。
        assert!(!script.contains("__DB_NAME__"));
        assert!(script.contains("echo '###STAGE:1:正在导出数据库' \"$DB_NAME\" '...'"));
        // 超时后可通过 pidfile 终止远端脚本。
        assert!(script.contains("PID_FILE=\"/tmp/deploycode-backup-abc.pid\""));
        assert!(script.contains("echo $$ > \"$PID_FILE\""));
    }

    #[test]
    fn retention_flag_decides_whether_the_dump_survives_the_script() {
        let url = "postgresql://u@h:5432/db";
        let keep = build_backup_script(&record(), &source(), url, None, true);
        assert!(keep.contains("KEEP=1"));
        // 留存模式下 cleanup 放过 $OUT，本机才有东西可下载；pidfile 与脚本仍旧照删。
        assert!(keep.contains("if [ \"$KEEP\" = \"1\" ]"));
        assert!(keep.contains("rm -f \"$PID_FILE\" \"$0\""));

        let discard = build_backup_script(&record(), &source(), url, None, false);
        assert!(discard.contains("KEEP=0"));
        assert!(discard.contains("rm -f \"$OUT\" \"$PID_FILE\" \"$0\""));
    }

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

    #[test]
    fn template_values_with_reset_sql_placeholder_are_not_rescanned() {
        let mut src = source();
        src.database = "x__RESET_SQL__y$(touch /tmp/pwned)".to_string();
        src.schema = "s__RESET_SQL__$(id)".to_string();
        let script = build_backup_script(
            &record(),
            &src,
            "postgresql://u@h:5432/db",
            Some("secret"),
            false,
        );
        // 值整体留在单引号内，内部的 __RESET_SQL__ 不会被二次替换成裸 SQL。
        assert!(
            script.contains("DB_NAME='x__RESET_SQL__y$(touch /tmp/pwned)'"),
            "database value was rescanned: {script}"
        );
        assert!(script.contains("SCHEMA='s__RESET_SQL__$(id)'"));
        // 重置 SQL 只出现一次（仅来自模板自身的 __RESET_SQL__ 占位符）。
        assert_eq!(script.matches("DROP SCHEMA IF EXISTS").count(), 1);
    }

    #[test]
    fn reset_sql_drops_and_recreates_schema_with_grants() {
        let sql = reset_schema_sql("public");
        assert!(sql.contains("DROP SCHEMA IF EXISTS \"public\" CASCADE;"));
        assert!(sql.contains("CREATE SCHEMA \"public\";"));
        assert!(sql.contains("anon, authenticated, service_role"));
        assert!(sql.contains("$do$"));
        // EXECUTE 字符串里的标识符必须是 "public"，不能是 '"public"'（会语法错误）。
        assert!(sql.contains("EXECUTE 'GRANT USAGE ON SCHEMA \"public\" TO"));
        assert!(!sql.contains("'\"public\"'"));
        // schema 名含单引号时字面量转义，不破坏 SQL。
        let tricky = reset_schema_sql("it's");
        assert!(tricky.contains("EXECUTE 'GRANT USAGE ON SCHEMA \"it''s\" TO"));
    }

    #[test]
    fn import_filters_dump_created_schema_statement() {
        let script = build_backup_script(
            &record(),
            &source(),
            "postgresql://u@h:5432/db",
            Some("secret"),
            false,
        );
        // 导入时必须过滤 pg_dump 自带的 CREATE SCHEMA，否则目标 schema 已存在会中断导入。
        assert!(
            script.contains("gunzip -c \"$OUT\" | awk"),
            "import pipeline missing schema filter: {script}"
        );
        assert!(script.contains("schema_toc"));
        // COPY 数据段必须原样放行，避免数据行恰好形似 TOC 注释时被误删。
        assert!(script.contains("in_copy"));
        // 过滤后仍以 ON_ERROR_STOP 严格导入。
        assert!(script.contains("| run_psql -v ON_ERROR_STOP=1 -q"));
    }

    #[test]
    fn resolve_target_prefers_override_then_targets_then_legacy() {
        let mut server = ServerConfig::new(
            "s".to_string(),
            "h".to_string(),
            "u".to_string(),
            crate::models::SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let mut settings = Settings::default();
        assert!(resolve_target(&[], &server, &settings, &None, &None, &None, &None).is_err());

        let targets = vec![
            BackupTarget::new("Aiven".to_string(), "postgresql://aiven@host/db".to_string()),
            BackupTarget::new("Neon".to_string(), "postgresql://neon@host/db".to_string()),
        ];

        // 全局旧连接串作为兜底。
        settings.supabase_url = "postgresql://legacy@host/db".to_string();
        let resolved =
            resolve_target(&targets, &server, &settings, &None, &None, &None, &None).unwrap();
        assert_eq!(resolved.url, "postgresql://legacy@host/db");

        // 全局默认目标优先于旧连接串。
        settings.default_backup_target_id = Some(targets[0].id.clone());
        let resolved =
            resolve_target(&targets, &server, &settings, &None, &None, &None, &None).unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 服务器绑定目标优先于全局默认。
        server.backup_target_id = Some(targets[1].id.clone());
        let resolved =
            resolve_target(&targets, &server, &settings, &None, &None, &None, &None).unwrap();
        assert_eq!(resolved.name, "Neon");

        // 请求指定目标（可用名称匹配）优先于服务器绑定。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &None,
            &None,
            &None,
        )
        .unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 请求指定目标优先于配置自身的连接串（否则会被配置覆盖到错误的库）。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &None,
            &None,
            &Some("postgresql://config@host/db".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 请求直接连接串优先级最高。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &Some("postgresql://raw@host/db".to_string()),
            &None,
            &None,
        )
        .unwrap();
        assert_eq!(resolved.url, "postgresql://raw@host/db");

        // 无请求参数时，配置连接串优先于配置目标。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &None,
            &None,
            &Some(targets[1].id.clone()),
            &Some("postgresql://config@host/db".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.url, "postgresql://config@host/db");

        // 不存在的目标报错。
        assert!(matches!(
            resolve_target(
                &targets,
                &server,
                &settings,
                &Some("missing".to_string()),
                &None,
                &None,
                &None
            ),
            Err(CoreError::Config(_))
        ));

        // 非 postgres 连接串报错。
        assert!(matches!(
            resolve_target(
                &targets,
                &server,
                &settings,
                &Some("mysql://x".to_string()),
                &None,
                &None,
                &None
            ),
            Err(CoreError::Config(_))
        ));
    }

    #[test]
    fn resolve_backup_uses_saved_config_then_request_overrides() {
        use crate::models::{BackupConfig, SshAuth};

        let server = ServerConfig::new(
            "prod".to_string(),
            "h".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let mut config = AppConfig {
            servers: vec![server.clone()],
            ..AppConfig::default()
        };
        let targets = vec![BackupTarget::new(
            "Aiven".to_string(),
            "postgresql://aiven@host/db".to_string(),
        )];
        config.backup_targets = targets.clone();
        let mut saved = BackupConfig::new(
            "生产库".to_string(),
            server.id.clone(),
            DbBackupSource {
                mode: "system".to_string(),
                container: String::new(),
                database: "app".to_string(),
                username: "postgres".to_string(),
                password: "p".to_string(),
                schema: "app".to_string(),
            },
        );
        saved.target_id = Some(config.backup_targets[0].id.clone());
        config.backup_configs.push(saved.clone());

        let request = BackupRequest {
            // 使用配置时允许 server_id 留空（CLI 可只给 --config）。
            server_id: String::new(),
            backup_config_id: Some(saved.id.clone()),
            source: None,
            target_id: None,
            supabase_url: None,
            database: Some("override".to_string()),
            schema: None,
        };
        let resolved = resolve_backup(&config, &request).unwrap();
        assert_eq!(resolved.server.id, server.id);
        assert_eq!(resolved.source.database, "override");
        assert_eq!(resolved.source.mode, "system");
        assert_eq!(resolved.target.name, "Aiven");

        // 也可以在配置基础上直接覆盖来源（测试未保存的表单）。
        let request = BackupRequest {
            backup_config_id: Some(saved.id.clone()),
            source: Some(source()),
            database: None,
            schema: None,
            ..request
        };
        let resolved = resolve_backup(&config, &request).unwrap();
        assert_eq!(resolved.source.mode, "docker");
        assert_eq!(resolved.source.container, "postgres");
        assert_eq!(resolved.source.database, "app");

        // 配置里保存了自定义连接串时，请求显式指定的目标仍必须生效。
        let mut with_url = config.clone();
        with_url.backup_configs[0].supabase_url = Some("postgresql://saved@host/db".to_string());
        with_url.backup_targets = vec![
            BackupTarget::new("Aiven".to_string(), "postgresql://aiven@host/db".to_string()),
            BackupTarget::new("Neon".to_string(), "postgresql://neon@host/db".to_string()),
        ];
        let neon = with_url.backup_targets[1].id.clone();
        let request = BackupRequest {
            server_id: String::new(),
            backup_config_id: Some(saved.id.clone()),
            source: None,
            target_id: Some(neon),
            supabase_url: None,
            database: None,
            schema: None,
        };
        let resolved = resolve_backup(&with_url, &request).unwrap();
        assert_eq!(resolved.target.name, "Neon");

        // 未指定配置且服务器没有来源时报错。
        let empty = AppConfig {
            servers: vec![server],
            ..AppConfig::default()
        };
        let request = BackupRequest {
            server_id: empty.servers[0].id.clone(),
            backup_config_id: None,
            source: None,
            target_id: None,
            supabase_url: None,
            database: None,
            schema: None,
        };
        assert!(matches!(
            resolve_backup(&empty, &request),
            Err(CoreError::Config(_))
        ));

        // 不存在的配置报错。
        let request = BackupRequest {
            backup_config_id: Some("missing".to_string()),
            ..request
        };
        assert!(matches!(
            resolve_backup(&config, &request),
            Err(CoreError::NotFound(_))
        ));
    }
}
