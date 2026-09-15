use std::sync::Arc;

use deploy_core::models::{
    BackupConfig, BackupEvent, BackupRequest, BackupTarget, DbBackupSource, DeployStatus,
};
use deploy_core::{backup::mask_database_url, BackupEngine, CoreError, Result, Store};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

use super::{
    claim_task_lock, format_duration, human_size, open_store, print_json, redact_backup_config,
    redact_target, short_id, truncate,
};

fn resolve_backup_server(
    config: &deploy_core::models::AppConfig,
    server: Option<&str>,
    backup_config: Option<&str>,
) -> Result<String> {
    if let Some(key) = backup_config {
        return Ok(Store::find_backup_config(config, key)?.server_id.clone());
    }
    match server {
        Some(key) => Ok(Store::find_server(config, key)?.id.clone()),
        None => Err(CoreError::config(
            "请指定服务器，或使用 --config <名称/ID> 选择已保存的备份配置",
        )),
    }
}

/// 与 core 保持一致的来源字段规范化。
fn normalize_source(source: &mut DbBackupSource) {
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
}

pub(super) async fn backup_command(cli: &Cli, command: &BackupCommand) -> Result<()> {
    let store = Arc::new(open_store(cli)?);
    // 旧版「服务器单份来源配置」升级为备份配置列表（已迁移则直接返回）。
    let _ = store.migrate_backup_configs();
    match command {
        BackupCommand::Run {
            server,
            config: config_key,
            target,
            supabase_url,
            database,
            schema,
        } => {
            // 与 GUI / 其他命令行进程互斥，避免并发执行破坏性操作。
            let _lock = claim_task_lock(&store, "backup")?;
            let loaded = store.load_config()?;
            let server_id =
                resolve_backup_server(&loaded, server.as_deref(), config_key.as_deref())?;
            let engine = BackupEngine::new(store.clone());
            let request = BackupRequest {
                server_id,
                backup_config_id: config_key.clone(),
                source: None,
                target_id: target.clone(),
                supabase_url: supabase_url.clone(),
                database: database.clone(),
                schema: schema.clone(),
            };
            let prepared = engine.prepare(&request)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "type": "prepared",
                        "recordId": prepared.record.id,
                    }))?
                );
            } else {
                output::info(format!("备份记录: {}", prepared.record.id));
                output::info(format!(
                    "来源: {} / {}",
                    prepared.record.server_name, prepared.record.database
                ));
                output::info(format!(
                    "目标: {} ({})",
                    prepared.record.target_name, prepared.record.target
                ));
            }

            let json = cli.json;
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let printer = tokio::spawn(async move {
                while let Some(event) = receiver.recv().await {
                    if json {
                        if let Ok(line) = serde_json::to_string(&event) {
                            println!("{line}");
                        }
                        continue;
                    }
                    match event {
                        BackupEvent::Log { level, message } => output::deploy_log(level, &message),
                        BackupEvent::Progress { message, .. } => output::progress(&message),
                        _ => {}
                    }
                }
            });

            let record = engine
                .run(prepared.record, prepared.source, prepared.target, Some(sender))
                .await;
            let _ = printer.await;
            if json {
                println!("{}", serde_json::to_string(&record)?);
            } else {
                output::progress_done();
            }

            if record.status == DeployStatus::Failed {
                std::process::exit(2);
            }
            Ok(())
        }
        BackupCommand::Config(command) => match command {
            BackupConfigCommand::Add(args) => {
                let saved = store.mutate_config(|config| {
                    let server = Store::find_server(config, &args.server)?.clone();
                    // 同名（或同 id）视为更新，保留其余未覆盖字段。
                    let existing = config
                        .backup_configs
                        .iter()
                        .find(|item| item.name == args.name || item.id == args.name)
                        .cloned();
                    let mut source = existing
                        .as_ref()
                        .map(|item| item.source.clone())
                        .or_else(|| server.db_backup.clone())
                        .unwrap_or_default();
                    if let Some(mode) = &args.mode {
                        source.mode = mode.clone();
                    }
                    if let Some(container) = &args.container {
                        source.container = container.clone();
                    }
                    if let Some(database) = &args.database {
                        source.database = database.clone();
                    }
                    if let Some(username) = &args.username {
                        source.username = username.clone();
                    }
                    if let Some(password) = &args.password {
                        source.password = password.clone();
                    }
                    if let Some(schema) = &args.schema {
                        source.schema = schema.clone();
                    }
                    normalize_source(&mut source);
                    if source.database.is_empty() {
                        return Err(CoreError::config("数据库名不能为空"));
                    }
                    if source.username.is_empty() {
                        return Err(CoreError::config("数据库用户名不能为空"));
                    }
                    if source.mode == "docker" && source.container.is_empty() {
                        return Err(CoreError::config("docker 模式需要填写容器名"));
                    }

                    let target_id = match &args.target {
                        Some(key) => Some(
                            config
                                .backup_targets
                                .iter()
                                .find(|item| item.id == *key || item.name == *key)
                                .map(|item| item.id.clone())
                                .ok_or_else(|| {
                                    CoreError::not_found(format!("备份目标不存在: {key}"))
                                })?,
                        ),
                        None => existing.as_ref().and_then(|item| item.target_id.clone()),
                    };
                    if let Some(url) = &args.supabase_url {
                        if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                            return Err(CoreError::config(
                                "连接串必须以 postgres:// 或 postgresql:// 开头",
                            ));
                        }
                    }
                    // 自定义连接串在解析时优先于目标：显式指定其中一个时清掉另一个，
                    // 否则旧字段会压过新指定的目标，且无法从命令行纠正。
                    let (target_id, supabase_url) = if args.supabase_url.is_some() {
                        (None, args.supabase_url.clone())
                    } else if args.target.is_some() {
                        (target_id, None)
                    } else {
                        (
                            target_id,
                            existing.as_ref().and_then(|item| item.supabase_url.clone()),
                        )
                    };

                    let item = BackupConfig {
                        id: existing
                            .as_ref()
                            .map(|item| item.id.clone())
                            .unwrap_or_else(deploy_core::models::new_id),
                        name: args.name.trim().to_string(),
                        server_id: server.id.clone(),
                        source,
                        target_id,
                        supabase_url,
                    };
                    if item.name.is_empty() {
                        return Err(CoreError::config("配置名称不能为空"));
                    }
                    if config
                        .backup_configs
                        .iter()
                        .any(|other| other.id != item.id && other.name == item.name)
                    {
                        return Err(CoreError::config(format!(
                            "备份配置名称已存在: {}",
                            item.name
                        )));
                    }
                    match config.backup_configs.iter_mut().find(|other| other.id == item.id) {
                        Some(slot) => *slot = item.clone(),
                        None => config.backup_configs.push(item.clone()),
                    }
                    Ok(item)
                })?;

                if cli.json {
                    return print_json(&redact_backup_config(&saved));
                }
                output::success(format!(
                    "已保存备份配置 {}（{} · {}）",
                    saved.name, args.server, saved.source.database
                ));
                Ok(())
            }
            BackupConfigCommand::List => {
                let config = store.load_config()?;
                if cli.json {
                    let list: Vec<_> =
                        config.backup_configs.iter().map(redact_backup_config).collect();
                    return print_json(&list);
                }
                if config.backup_configs.is_empty() {
                    output::dim(
                        "暂无备份配置，使用 `backup config add <名称> --server <服务器> --database <库名> --username <用户>` 添加",
                    );
                    return Ok(());
                }
                println!(
                    "{:<16} {:<14} {:<12} {:<7} {}",
                    "名称", "服务器", "数据库", "方式", "目标"
                );
                for item in &config.backup_configs {
                    let server = config
                        .servers
                        .iter()
                        .find(|server| server.id == item.server_id)
                        .map(|server| server.name.as_str())
                        .unwrap_or("(服务器已删除)");
                    let target = item
                        .target_id
                        .as_deref()
                        .and_then(|id| {
                            config
                                .backup_targets
                                .iter()
                                .find(|target| target.id == id)
                        })
                        .map(|target| target.name.clone())
                        .or_else(|| item.supabase_url.as_deref().map(mask_database_url))
                        .unwrap_or_else(|| "默认目标".to_string());
                    println!(
                        "{:<16} {:<14} {:<12} {:<7} {}",
                        item.name, server, item.source.database, item.source.mode, target
                    );
                }
                Ok(())
            }
            BackupConfigCommand::Remove { config: key } => {
                store.mutate_config(|config| {
                    let removed: Vec<String> = config
                        .backup_configs
                        .iter()
                        .filter(|item| item.id == *key || item.name == *key)
                        .map(|item| item.id.clone())
                        .collect();
                    if removed.is_empty() {
                        return Err(CoreError::not_found(format!("备份配置不存在: {key}")));
                    }
                    config.backup_configs.retain(|item| !removed.contains(&item.id));
                    // 被删除的配置若正被定时备份使用，清空引用，避免每天到点报错。
                    if config
                        .settings
                        .scheduled_backup_config_id
                        .as_ref()
                        .is_some_and(|id| removed.contains(id))
                    {
                        config.settings.scheduled_backup_config_id = None;
                    }
                    Ok(())
                })?;
                output::success(format!("已删除备份配置 {key}"));
                Ok(())
            }
        },
        BackupCommand::Target(command) => match command {
            TargetCommand::Add { name, url } => {
                let target = store.mutate_config(|config| {
                    if config.backup_targets.iter().any(|target| target.name == *name) {
                        return Err(CoreError::config(format!("备份目标名称已存在: {name}")));
                    }
                    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                        return Err(CoreError::config(
                            "连接串必须以 postgres:// 或 postgresql:// 开头",
                        ));
                    }
                    let target = BackupTarget::new(name.clone(), url.clone());
                    config.backup_targets.push(target.clone());
                    Ok(target)
                })?;
                if cli.json {
                    return print_json(&redact_target(&target));
                }
                output::success(format!(
                    "已添加备份目标 {} ({})",
                    target.name,
                    mask_database_url(&target.url)
                ));
                Ok(())
            }
            TargetCommand::List => {
                let config = store.load_config()?;
                if cli.json {
                    let targets: Vec<_> = config.backup_targets.iter().map(redact_target).collect();
                    return print_json(&targets);
                }
                if config.backup_targets.is_empty() {
                    output::dim("暂无备份目标，使用 `backup target add <名称> <连接串>` 添加");
                    return Ok(());
                }
                let default_id = config.settings.default_backup_target_id.as_deref();
                for target in &config.backup_targets {
                    let marker = if default_id == Some(target.id.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    println!(
                        "{marker} {:<16} {}",
                        target.name,
                        mask_database_url(&target.url)
                    );
                }
                Ok(())
            }
            TargetCommand::Remove { target } => {
                store.mutate_config(|config| {
                    let id = config
                        .backup_targets
                        .iter()
                        .find(|item| item.id == *target || item.name == *target)
                        .map(|item| item.id.clone())
                        .ok_or_else(|| {
                            CoreError::not_found(format!("备份目标不存在: {target}"))
                        })?;
                    config.backup_targets.retain(|item| item.id != id);
                    if config.settings.default_backup_target_id.as_deref() == Some(id.as_str()) {
                        config.settings.default_backup_target_id = None;
                    }
                    for server in config.servers.iter_mut() {
                        if server.backup_target_id.as_deref() == Some(id.as_str()) {
                            server.backup_target_id = None;
                        }
                    }
                    for saved in config.backup_configs.iter_mut() {
                        if saved.target_id.as_deref() == Some(id.as_str()) {
                            saved.target_id = None;
                        }
                    }
                    Ok(())
                })?;
                output::success(format!("已删除备份目标 {target}"));
                Ok(())
            }
        },
        BackupCommand::List { server, limit } => {
            let mut records = store.load_backups()?;
            records.reverse();
            if let Some(key) = server {
                let server_id = Store::find_server(&store.load_config()?, key)?.id.clone();
                records.retain(|record| record.server_id == server_id);
            }
            records.truncate(*limit);

            if cli.json {
                return print_json(&records);
            }
            if records.is_empty() {
                output::dim("暂无备份记录");
                return Ok(());
            }
            println!(
                "{:<9} {:<19} {:<14} {:<12} {:<12} {:<10} {:<6} {}",
                "ID", "时间", "服务器", "数据库", "目标", "大小", "状态", "耗时"
            );
            for record in records {
                let status = match record.status {
                    DeployStatus::Success => "成功",
                    DeployStatus::Failed => "失败",
                    DeployStatus::Running => "进行中",
                };
                println!(
                    "{:<9} {:<19} {:<14} {:<12} {:<12} {:<10} {:<6} {}",
                    short_id(&record.id),
                    record.started_at,
                    truncate(&record.server_name, 12),
                    truncate(&record.database, 10),
                    truncate(&record.target_name, 10),
                    human_size(record.dump_size),
                    status,
                    format_duration(record.duration_ms),
                );
            }
            Ok(())
        }
        BackupCommand::Show { record } => {
            let record = store.find_backup(record)?;
            if cli.json {
                return print_json(&record);
            }
            println!("记录 ID   : {}", record.id);
            println!("服务器    : {} ({})", record.server_name, record.server_id);
            println!("数据库    : {} · schema {}", record.database, record.schema);
            println!("目标      : {} ({})", record.target_name, record.target);
            println!(
                "状态      : {}",
                match record.status {
                    DeployStatus::Success => "成功",
                    DeployStatus::Failed => "失败",
                    DeployStatus::Running => "进行中",
                }
            );
            println!("压缩包    : {}", human_size(record.dump_size));
            println!("开始时间  : {}", record.started_at);
            if let Some(finished) = &record.finished_at {
                println!("结束时间  : {finished}");
            }
            if let Some(err) = &record.error {
                println!("错误      : {err}");
            }
            println!("--- 日志 ---");
            output::info(&record.log);
            Ok(())
        }
        BackupCommand::Clear { yes } => {
            if !yes {
                return Err(CoreError::config("清空记录不可恢复，确认请加 --yes"));
            }
            store.clear_backups()?;
            output::success("已清空备份记录");
            Ok(())
        }
        BackupCommand::Test { server, config: config_key } => {
            let loaded = store.load_config()?;
            let server_id =
                resolve_backup_server(&loaded, server.as_deref(), config_key.as_deref())?;
            if let Ok(server) = Store::find_server(&loaded, &server_id) {
                if !cli.json {
                    output::info(format!("正在检查 {} 的备份环境 ...", server.name));
                }
            }
            let request = BackupRequest {
                server_id,
                backup_config_id: config_key.clone(),
                source: None,
                target_id: None,
                supabase_url: None,
                database: None,
                schema: None,
            };
            let message = BackupEngine::new(store).test(&request).await?;
            if cli.json {
                return print_json(&serde_json::json!({ "message": message }));
            }
            output::success("备份环境连通");
            output::info(message);
            Ok(())
        }
    }
}
