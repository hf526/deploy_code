//! 数据库备份：把服务器上的 PostgreSQL schema 导出并全量恢复到远端 PostgreSQL。
//!
//! 流程全部在目标服务器上完成（不上传数据库数据到本机）：
//! 1. 通过 `pg_dump`（docker exec 或本机命令）导出为压缩的 SQL 文件；
//! 2. 校验压缩文件完整性；
//! 3. 清空并重建目标 schema（默认 public，保留默认角色授权）；
//! 4. `gunzip | psql` 全量导入目标数据库。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::models::{
    now_string, BackupEvent, BackupRecord, BackupRequest, BackupTarget, DbBackupSource,
    DeployStatus, LogLevel, ServerConfig, Settings,
};
use crate::process::shell_quote;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;

/// 单条备份记录最多保留的日志行数。
const MAX_LOG_LINES: usize = 8000;

/// 本地临时脚本的清理守卫：无论正常返回还是 panic 都会删除文件。
struct TempScriptGuard(PathBuf);

impl Drop for TempScriptGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
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
        let server = Store::find_server(&config, &req.server_id)?.clone();

        let mut source = normalize_source(
            server
                .db_backup
                .clone()
                .ok_or_else(|| CoreError::config("请先为该服务器配置数据库备份来源"))?,
        )?;
        if let Some(database) = non_empty(&req.database) {
            source.database = database.to_string();
        }
        if let Some(schema) = non_empty(&req.schema) {
            source.schema = schema.to_string();
        }
        validate_source(&source)?;

        let target = resolve_target(
            &config.backup_targets,
            &server,
            &config.settings,
            &req.target_id,
            &req.supabase_url,
        )?;

        let record = BackupRecord {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            database: source.database.clone(),
            schema: source.schema.clone(),
            target_name: target.name.clone(),
            target: mask_database_url(&target.url),
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            dump_size: 0,
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };
        self.store
            .upsert_backup(&record, config.settings.backup_history_limit)?;
        Ok(PreparedBackup {
            record,
            source,
            target: target.url,
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
        let mut logger = BackupLogger::new(events);

        let _ = logger.send(BackupEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始备份 {} · {} @ {}",
            record.server_name, record.database, record.target
        ));

        let result = self.execute(&record, &source, &target, &mut logger).await;

        match result {
            Ok(size) => {
                record.dump_size = size;
                record.status = DeployStatus::Success;
                logger.success(format!(
                    "备份成功（{}，耗时 {}）",
                    human_size(size),
                    format_duration(started.elapsed().as_millis() as u64)
                ));
            }
            Err(err) => {
                record.status = DeployStatus::Failed;
                record.error = Some(err.to_string());
                logger.error(format!("备份失败: {err}"));
            }
        }

        record.log = join_log_lines(&logger);
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        let limit = self
            .store
            .load_config()
            .map(|config| config.settings.backup_history_limit)
            .unwrap_or(200);
        if let Err(err) = self.store.upsert_backup(&record, limit) {
            logger.error(format!("保存备份记录失败: {err}"));
            record.log = join_log_lines(&logger);
        }

        if let Some(sender) = logger.events.take() {
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
        logger: &mut BackupLogger,
    ) -> Result<u64> {
        let config = self.store.load_config()?;
        let settings = config.settings.clone();
        let server = Store::find_server(&config, &record.server_id)?.clone();

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

        let script = build_backup_script(record, source, &target_url, target_password.as_deref());
        let remote = format!("/tmp/deploycode-backup-{}.sh", record.id);
        let outcome = self
            .run_script(&client, &remote, &script, timeout, logger, &record.id)
            .await;
        client.disconnect().await;
        outcome
    }

    async fn run_script(
        &self,
        client: &SshClient,
        remote: &str,
        script: &str,
        timeout: u64,
        logger: &mut BackupLogger,
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
                let dump = format!("/tmp/deploycode-backup-{record_id}.sql.gz");
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

        let (code, output) = client
            .exec_capture(&format!("chmod 700 {quoted}"), 30)
            .await?;
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
        let server = Store::find_server(&config, &req.server_id)?.clone();

        let mut source = normalize_source(
            server
                .db_backup
                .clone()
                .ok_or_else(|| CoreError::config("请先为该服务器配置数据库备份来源"))?,
        )?;
        if let Some(database) = non_empty(&req.database) {
            source.database = database.to_string();
        }
        if let Some(schema) = non_empty(&req.schema) {
            source.schema = schema.to_string();
        }
        validate_source(&source)?;
        let target = resolve_target(
            &config.backup_targets,
            &server,
            &config.settings,
            &req.target_id,
            &req.supabase_url,
        )?;
        let (target_url, target_password) = split_database_url(&target.url)?;

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
    if source.schema.contains('"') || source.schema.contains('\0') {
        return Err(CoreError::config("schema 名称包含非法字符"));
    }
    Ok(())
}

/// 已解析的备份目标。
struct ResolvedTarget {
    name: String,
    url: String,
}

/// 解析备份目标，优先级：
/// 接口/命令行直接连接串 > 指定目标 > 服务器绑定目标 > 服务器自定义连接串 >
/// 全局默认目标 > 旧版全局连接串。
fn resolve_target(
    targets: &[BackupTarget],
    server: &ServerConfig,
    settings: &Settings,
    target_id: &Option<String>,
    override_url: &Option<String>,
) -> Result<ResolvedTarget> {
    if let Some(url) = non_empty(override_url) {
        return Ok(ResolvedTarget {
            name: "自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(target_id) {
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

/// 隐藏连接串中的密码（userinfo 与查询参数），用于日志与记录展示。
pub fn mask_database_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..scheme_end];
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let remainder = &rest[authority_end..];

    // 以最后一个 '@' 作为 userinfo 与 host 的分界，避免未编码的 '@' 出现在密码里时把密码带出来。
    let authority = match authority.rfind('@') {
        Some(at) => {
            let creds = &authority[..at];
            let host = &authority[at + 1..];
            let user = creds.split(':').next().unwrap_or(creds);
            if creds.contains(':') {
                format!("{user}:***@{host}")
            } else {
                format!("{user}@{host}")
            }
        }
        None => authority.to_string(),
    };

    format!("{scheme}://{authority}{}", mask_query_password(remainder))
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
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
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

cleanup() { rm -f "$OUT" "$PID_FILE" "$0"; }
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
gunzip -c "$OUT" | run_psql -v ON_ERROR_STOP=1 -q

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

fn build_backup_script(
    record: &BackupRecord,
    source: &DbBackupSource,
    target_url: &str,
    target_password: Option<&str>,
) -> String {
    render_template(
        BACKUP_TEMPLATE,
        &[
            ("__ID__", record.id.clone()),
            ("__TARGET_URL__", shell_quote(target_url)),
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
fn render_template(template: &str, values: &[(&str, String)]) -> String {
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

// ---------------------------------------------------------------------------
// 日志
// ---------------------------------------------------------------------------

struct BackupLogger {
    lines: VecDeque<String>,
    events: Option<BackupEventSender>,
}

impl BackupLogger {
    fn new(events: Option<BackupEventSender>) -> Self {
        Self {
            lines: VecDeque::new(),
            events,
        }
    }

    fn send(&self, event: BackupEvent) -> Option<()> {
        self.events.as_ref().map(|sender| {
            let _ = sender.send(event);
        })
    }

    fn line(&mut self, level: LogLevel, message: impl Into<String>) {
        let text = format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            message.into()
        );
        if self.lines.len() >= MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(text.clone());
        self.send(BackupEvent::Log {
            level,
            message: text,
        });
    }

    fn info(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Info, message);
    }

    fn success(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Success, message);
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Warn, message);
    }

    fn error(&mut self, message: impl Into<String>) {
        self.line(LogLevel::Error, message);
    }

    fn progress(&self, percent: u8, message: &str) {
        self.send(BackupEvent::Progress {
            percent,
            message: message.to_string(),
        });
    }
}

fn join_log_lines(logger: &BackupLogger) -> String {
    logger.lines.iter().cloned().collect::<Vec<_>>().join("\n")
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.2} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:.0}s", ms / 60_000, (ms % 60_000) as f64 / 1000.0)
    }
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
    fn template_values_with_reset_sql_placeholder_are_not_rescanned() {
        let mut src = source();
        src.database = "x__RESET_SQL__y$(touch /tmp/pwned)".to_string();
        src.schema = "s__RESET_SQL__$(id)".to_string();
        let script =
            build_backup_script(&record(), &src, "postgresql://u@h:5432/db", Some("secret"));
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
        assert!(resolve_target(&[], &server, &settings, &None, &None).is_err());

        let targets = vec![
            BackupTarget::new("Aiven".to_string(), "postgresql://aiven@host/db".to_string()),
            BackupTarget::new("Neon".to_string(), "postgresql://neon@host/db".to_string()),
        ];

        // 全局旧连接串作为兜底。
        settings.supabase_url = "postgresql://legacy@host/db".to_string();
        let resolved = resolve_target(&targets, &server, &settings, &None, &None).unwrap();
        assert_eq!(resolved.url, "postgresql://legacy@host/db");

        // 全局默认目标优先于旧连接串。
        settings.default_backup_target_id = Some(targets[0].id.clone());
        let resolved = resolve_target(&targets, &server, &settings, &None, &None).unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 服务器绑定目标优先于全局默认。
        server.backup_target_id = Some(targets[1].id.clone());
        let resolved = resolve_target(&targets, &server, &settings, &None, &None).unwrap();
        assert_eq!(resolved.name, "Neon");

        // 指定目标（可用名称匹配）优先于服务器绑定。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &None,
        )
        .unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 直接连接串优先级最高。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &Some("postgresql://raw@host/db".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.url, "postgresql://raw@host/db");

        // 不存在的目标报错。
        assert!(matches!(
            resolve_target(&targets, &server, &settings, &Some("missing".to_string()), &None),
            Err(CoreError::Config(_))
        ));

        // 非 postgres 连接串报错。
        assert!(matches!(
            resolve_target(
                &targets,
                &server,
                &settings,
                &Some("mysql://x".to_string()),
                &None
            ),
            Err(CoreError::Config(_))
        ));
    }
}
