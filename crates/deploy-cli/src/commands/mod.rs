use std::sync::Arc;

use deploy_core::models::{
    BackupConfig, BackupTarget, DeployEvent, DeployRecord, DeployRequest, ServerConfig, SshAuth,
};
use deploy_core::{backup::mask_database_url, CoreError, DeployEngine, Result, Store};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

mod backup;
mod branch;
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
        Command::Pages(command) => pages::pages_command(cli, command).await,
        Command::Where => {
            let store = open_store(cli)?;
            output::info(store.base_dir().display().to_string());
            Ok(())
        }
    }
}

pub fn open_store(cli: &Cli) -> Result<Store> {
    match &cli.data_dir {
        Some(dir) => Ok(Store::new(dir)),
        None => Store::default_store(),
    }
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

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let printer = tokio::spawn(async move {
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
                _ => {}
            }
        }
    });

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

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:.0}s", ms / 60_000, (ms % 60_000) as f64 / 1000.0)
    }
}

/// 取记录 ID 前 8 个字符用于列表展示（按字符而非字节，避免多字节 panic）。
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
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
