use std::sync::Arc;

use deploy_core::models::{
    BackupConfig, BackupTarget, DeployEvent, DeployRecord, DeployRequest, LogLevel, ServerConfig,
    SshAuth,
};
use deploy_core::{
    backup::mask_database_url, format_duration, human_size, CoreError, DeployEngine, Result, Store,
};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

mod backup;
mod branch;
mod container;
mod deploy;
mod pages;
mod repo;
mod server;

pub async fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Repo(command) => repo::repo_command(cli, command),
        Command::Branch(command) => branch::branch_command(cli, command),
        Command::Server(command) => server::server_command(cli, command).await,
        Command::Deploy(args) => deploy::deploy_command(cli, args).await,
        Command::Release(command) => deploy::release_command(cli, command).await,
        Command::History(command) => deploy::history_command(cli, command).await,
        Command::Backup(command) => backup::backup_command(cli, command).await,
        Command::Container(command) => container::container_command(cli, command).await,
        Command::Pages(command) => pages::pages_command(cli, command).await,
        Command::Where => {
            let store = open_store(cli)?;
            output::info(store.base_dir().display().to_string());
            Ok(())
        }
    }
}

pub fn open_store(cli: &Cli) -> Result<Store> {
    let store = match &cli.data_dir {
        Some(dir) => Store::new(dir),
        None => Store::default_store()?,
    };
    // 旧版每仓库一份的 repo.pages 是 skip_serializing 字段（只读不写），任何一次写配置
    // 都会把它冲掉；命令行入口先迁移成 pages_configs 列表，与 GUI 启动时同一套规则。
    let _ = store.migrate_pages_configs();
    Ok(store)
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    output::info(serde_json::to_string_pretty(value)?);
    Ok(())
}

/// JSON 输出中用于替换密码的占位符。
const MASKED_SECRET: &str = "***";

/// 复制一份服务器配置用于 JSON 输出，隐藏 SSH 密码 / 私钥口令 / 数据库密码与连接串密码。
fn redact_server(server: &ServerConfig) -> ServerConfig {
    let mut copy = server.clone();
    copy.auth = match &server.auth {
        SshAuth::Password { .. } => SshAuth::Password {
            password: MASKED_SECRET.to_string(),
        },
        SshAuth::PrivateKey { key_path, passphrase } => SshAuth::PrivateKey {
            key_path: key_path.clone(),
            passphrase: passphrase.as_ref().map(|_| MASKED_SECRET.to_string()),
        },
    };
    if let Some(source) = copy.db_backup.as_mut() {
        source.password = MASKED_SECRET.to_string();
    }
    if let Some(url) = copy.supabase_url.as_deref() {
        copy.supabase_url = Some(mask_database_url(url));
    }
    copy
}

/// 复制一份备份目标用于 JSON 输出，隐藏连接串中的密码。
fn redact_target(target: &BackupTarget) -> BackupTarget {
    BackupTarget {
        id: target.id.clone(),
        name: target.name.clone(),
        url: mask_database_url(&target.url),
    }
}

/// 复制一份备份配置用于 JSON 输出，隐藏数据库密码与目标连接串中的密码。
fn redact_backup_config(saved: &BackupConfig) -> BackupConfig {
    let mut copy = saved.clone();
    copy.source.password = MASKED_SECRET.to_string();
    if let Some(url) = copy.supabase_url.as_deref() {
        copy.supabase_url = Some(mask_database_url(url));
    }
    copy
}

/// 抢占跨进程任务锁；已被 GUI / 其他命令行进程持有时返回统一错误。
fn claim_task_lock(store: &Store, name: &str) -> Result<deploy_core::store::TaskLock> {
    store.try_task_lock(name)?.ok_or_else(|| {
        CoreError::busy("已有任务正在进行（GUI 或其他命令行进程），请稍后再试")
    })
}

/// 把部署事件转发到终端；`--json` 时每个事件一行，便于脚本消费。
/// 返回的任务在事件发送端全部 drop 之后才结束。
fn spawn_deploy_printer(
    mut receiver: mpsc::UnboundedReceiver<DeployEvent>,
    json: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            if json {
                // --json：每个事件一行 JSON，便于脚本消费。
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
                continue;
            }
            match event {
                DeployEvent::Log { level, message } => output::deploy_log(level, &message),
                DeployEvent::Progress { message, .. } => output::progress(&message),
                // 批次中止那台没有自己的记录，不打印就等于「看起来全成功」。
                DeployEvent::BatchAborted {
                    succeeded,
                    total,
                    reason,
                } => output::deploy_log(
                    LogLevel::Error,
                    &format!("已停止：{succeeded}/{total} 台成功，剩余机器未部署（{reason}）"),
                ),
                _ => {}
            }
        }
    })
}

/// 串行部署到多台服务器：与 GUI 走同一个 `DeployEngine::run_targets`，
/// 所以「一台失败即停止、剩余不部署」和批次日志在两边完全一致。整批共用一把任务锁。
async fn run_deploy_batch(
    store: &Arc<Store>,
    base: DeployRequest,
    server_ids: Vec<String>,
    json: bool,
) -> Result<Vec<DeployRecord>> {
    let _lock = claim_task_lock(store, "deploy")?;
    let engine = DeployEngine::new(store.clone());
    let (sender, receiver) = mpsc::unbounded_channel();
    let printer = spawn_deploy_printer(receiver, json);
    let batch = engine
        .run_targets(&base, &server_ids, Some(sender), |_| {})
        .await;
    let _ = printer.await;

    if json {
        for record in &batch.records {
            println!("{}", serde_json::to_string(record)?);
        }
    } else {
        output::progress_done();
    }
    // 首台连准备都没通过时不会有记录、也不会有一行「失败」输出，
    // 只返回空列表会让脚本调用方误读成成功（退出码 0），错误必须原样带回。
    if let Some(err) = batch.blocked {
        return Err(err);
    }
    // 跑起来过又被后面某台的准备失败掐断：第一台的记录是 success，只看记录就会 0 退出，
    // 而 GUI 那边同一批是红色失败。批次没发完就得是非零退出。
    if let Some(reason) = batch.aborted {
        return Err(CoreError::deploy(format!(
            "批量部署未全部发出：{reason}"
        )));
    }
    Ok(batch.records)
}

/// 执行部署并把事件实时打印到终端。
async fn run_deploy(store: &Arc<Store>, request: DeployRequest, json: bool) -> Result<DeployRecord> {
    // 与 GUI / 其他命令行进程互斥，避免并发执行破坏性操作。
    let _lock = claim_task_lock(store, "deploy")?;
    let engine = DeployEngine::new(store.clone());
    let record = engine.prepare(&request)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "type": "prepared",
                "recordId": record.id,
            }))?
        );
    } else {
        output::info(format!("部署记录: {}", record.id));
    }

    let (sender, receiver) = mpsc::unbounded_channel();
    let printer = spawn_deploy_printer(receiver, json);

    let final_record = engine.run(record, request, Some(sender)).await;
    let _ = printer.await;

    if json {
        println!("{}", serde_json::to_string(&final_record)?);
    } else {
        output::progress_done();
    }

    Ok(final_record)
}

fn find_record(store: &Store, key: &str) -> Result<DeployRecord> {
    let records = store.load_history()?;
    if let Some(record) = records.iter().find(|record| record.id == key) {
        return Ok(record.clone());
    }
    let matches: Vec<_> = records
        .iter()
        .filter(|record| record.id.starts_with(key))
        .collect();
    match matches.len() {
        0 => Err(CoreError::not_found(format!("部署记录不存在: {key}"))),
        1 => Ok(matches[0].clone()),
        _ => Err(CoreError::config("记录 ID 不唯一，请输入完整 ID")),
    }
}

fn truncate(value: &str, width: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= width {
        value.to_string()
    } else {
        let mut result: String = chars[..width.saturating_sub(1)].iter().collect();
        result.push('…');
        result
    }
}

/// 取记录 ID 前 8 个字符用于列表展示（按字符而非字节，避免多字节 panic）。
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::models::DbBackupSource;

    // 这几个是「明文凭据不许出 stdout」那条红线的哨兵：断言它们一个字节都不出现在输出里。
    const SSH_PASSWORD: &str = "ssh-pass-9f2a";
    const KEY_PASSPHRASE: &str = "key-pass-4c71";
    const DB_PASSWORD: &str = "db-pass-7b3e";
    const DSN_PASSWORD: &str = "dsn-pass-1d80";

    fn db_source() -> DbBackupSource {
        DbBackupSource {
            database: "app".to_string(),
            username: "postgres".to_string(),
            password: DB_PASSWORD.to_string(),
            ..DbBackupSource::default()
        }
    }

    fn server() -> ServerConfig {
        ServerConfig {
            id: "s1".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SshAuth::Password {
                password: SSH_PASSWORD.to_string(),
            },
            default_target_dir: "/opt/app".to_string(),
            db_backup: Some(db_source()),
            backup_target_id: Some("t1".to_string()),
            supabase_url: Some(format!("postgresql://u:{DSN_PASSWORD}@h:5432/db")),
            tunnels: Vec::new(),
            created_at: "2026-01-01 00:00:00".to_string(),
        }
    }

    fn json<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[test]
    fn redact_server_hides_every_credential_but_keeps_topology() {
        let text = json(&redact_server(&server()));

        for secret in [SSH_PASSWORD, DB_PASSWORD, DSN_PASSWORD] {
            assert!(!text.contains(secret), "凭据泄漏到 JSON 输出：{secret}\n{text}");
        }
        // 脱敏只该动凭据：脚本靠这些字段认服务器，一起抹掉这条输出就没用了。
        for kept in ["prod", "10.0.0.1", "root", "/opt/app", "t1", "app"] {
            assert!(text.contains(kept), "脱敏把非凭据字段也吃掉了：{kept}\n{text}");
        }
        // 连接串遮成密码位为 *** 但仍可读，而不是整条抹掉。
        assert!(text.contains("postgresql://u:***@h:5432/db"), "{text}");
    }

    #[test]
    fn redact_server_masks_private_key_passphrase_and_keeps_key_path() {
        let mut server = server();
        server.auth = SshAuth::PrivateKey {
            key_path: "/home/me/.ssh/id_ed25519".to_string(),
            passphrase: Some(KEY_PASSPHRASE.to_string()),
        };
        let text = json(&redact_server(&server));
        assert!(!text.contains(KEY_PASSPHRASE), "{text}");
        // key_path 本身不是凭据，界面与脚本要靠它指认用的是哪把钥匙。
        assert!(text.contains("/home/me/.ssh/id_ed25519"), "{text}");
        assert!(
            text.contains(&format!("\"passphrase\":\"{MASKED_SECRET}\"")),
            "{text}"
        );

        // 没设口令的私钥不能凭空多出一个 ***，否则用户会以为自己配了口令。
        server.auth = SshAuth::PrivateKey {
            key_path: "/home/me/.ssh/id_ed25519".to_string(),
            passphrase: None,
        };
        let text = json(&redact_server(&server));
        assert!(text.contains("\"passphrase\":null"), "{text}");
    }

    #[test]
    fn redact_target_hides_connection_string_password() {
        let target = BackupTarget {
            id: "t1".to_string(),
            name: "Aiven".to_string(),
            url: format!("postgresql://avnadmin:{DSN_PASSWORD}@db:5432/defaultdb"),
        };
        let text = json(&redact_target(&target));

        assert!(!text.contains(DSN_PASSWORD), "{text}");
        assert!(text.contains("Aiven"), "{text}");
        assert!(text.contains("db:5432/defaultdb"), "{text}");
    }

    #[test]
    fn redact_backup_config_hides_source_password_and_direct_url() {
        let config = BackupConfig {
            id: "c1".to_string(),
            name: "夜间备份".to_string(),
            server_id: "s1".to_string(),
            source: db_source(),
            target_id: Some("t1".to_string()),
            supabase_url: Some(format!("postgresql://u:{DSN_PASSWORD}@h:5432/db")),
        };
        let text = json(&redact_backup_config(&config));

        for secret in [DB_PASSWORD, DSN_PASSWORD] {
            assert!(!text.contains(secret), "凭据泄漏到 JSON 输出：{secret}\n{text}");
        }
        for kept in ["夜间备份", "postgres", "app", "t1"] {
            assert!(text.contains(kept), "脱敏把非凭据字段也吃掉了：{kept}\n{text}");
        }
    }

    #[test]
    fn passwordless_connection_string_is_not_mangled() {
        // 反向保护：没有密码的连接串不能被遮成 ***，否则用户看不出自己配的是哪个库。
        let target = BackupTarget {
            id: "t2".to_string(),
            name: "本地".to_string(),
            url: "postgresql://postgres@localhost:5432/app".to_string(),
        };
        let text = json(&redact_target(&target));
        assert!(text.contains("postgresql://postgres@localhost:5432/app"), "{text}");
    }
}
