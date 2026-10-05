//! `deploy-code-cli agent ...`：控制机的命令行入口。
//!
//! 与 GUI 共用 `deploy_core::agent::AgentControl`，所以两边看到的是同一份状态、
//! 走的是同一条下发与发起路径。JSON 输出不含凭据：状态与记录都是摘要字段。

use std::sync::Arc;

use deploy_core::agent::{AgentControl, AgentPruneReport, AgentStatus};
use deploy_core::models::{BackupEvent, ContainerEvent, DeployStatus, RunLocation};
use deploy_core::{format_duration, human_size, CoreError, Result, Store};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

use super::{claim_task_lock, open_store, print_json};
use super::container::lookup_server;

pub(super) async fn agent_command(cli: &Cli, command: &AgentCommand) -> Result<()> {
    let store = Arc::new(open_store(cli)?);
    let control = AgentControl::new(store.clone());
    match command {
        AgentCommand::Use { server } => {
            let id = lookup_server(&store, server)?.id;
            // 与 GUI 那个下拉框（src-tauri commands/app.rs::save_settings）同一个口径：旧控制机读的
            // 是它自己那份 config.json，不会自己停，而里面有全部源机的明文口令。换指向时把确实交接过
            // 的那台记成待收回，否则它到点照跑、界面那一栏却是空的，「收回」按钮只指向新机。
            let previous = store
                .load_config()?
                .settings
                .agent_server_id
                .trim()
                .to_string();
            let handed_off = store.load_agent_sync().is_for(&previous);
            store.mutate_config(|config| {
                config.settings.agent_server_id = id.clone();
                Ok(())
            })?;
            if !previous.is_empty() && previous != id && handed_off {
                store.remember_orphan_agent(&previous)?;
            }
            if !id.is_empty() {
                store.forget_orphan_agent(&id)?;
            }
            if cli.json {
                return print_json(&id);
            }
            output::success(format!("已把 {server} 设为控制机"));
            if !previous.is_empty() && previous != id && handed_off {
                output::error("原来那台控制机还挂着 agent，请收回：`agent uninstall --server <旧机器>`");
            }
            output::dim("下一步：`agent install` 上传并起服务，再 `agent sync` 下发配置");
            Ok(())
        }
        AgentCommand::Status { server } => {
            let status = control.status(server.as_deref()).await?;
            if cli.json {
                return print_json(&status);
            }
            print_status(&status);
            Ok(())
        }
        AgentCommand::Install { server } => {
            // 带 --server 就是「换成这台」：走 promote，装完顺手收回原来那台，
            // 与 GUI 的 install_agent 同一条路径（旧机器上的 agent 不会自己停）。
            let status = match server.as_deref() {
                Some(key) => {
                    let id = lookup_server(&store, key)?.id;
                    control.promote(&id).await?
                }
                None => control.install(None).await?,
            };
            if cli.json {
                return print_json(&status);
            }
            output::success(format!(
                "agent 已安装并启动（版本 {}，协议 {}）",
                status.version, status.proto
            ));
            print_status(&status);
            Ok(())
        }
        AgentCommand::Uninstall { server } => {
            let message = control.uninstall(server.as_deref()).await?;
            if cli.json {
                return print_json(&message);
            }
            output::success("已停用并删除 agent 服务");
            output::dim(format!("（{message} —— 备份包与记录留在控制机上）"));
            Ok(())
        }
        AgentCommand::Sync { server } => {
            let report = control.sync(server.as_deref()).await?;
            if cli.json {
                return print_json(&report);
            }
            output::success(format!(
                "已下发：服务器 {} 台、数据库备份配置 {} 条、容器备份配置 {} 条",
                report.servers, report.backup_configs, report.container_configs
            ));
            for warning in &report.warnings {
                output::dim(format!("提示: {warning}"));
            }
            print_status(&report.status);
            Ok(())
        }
        AgentCommand::Records { kind, limit } => match kind.as_str() {
            "backup" => {
                let list = control.backup_records(*limit, None).await?;
                if cli.json {
                    return print_json(&list);
                }
                if list.is_empty() {
                    output::dim("控制机上还没有数据库备份记录");
                    return Ok(());
                }
                println!(
                    "{:<9} {:<19} {:<14} {:<12} {:<10} {:<6} {}",
                    "ID", "时间", "服务器", "数据库", "导出包", "状态", "耗时"
                );
                for record in list {
                    println!(
                        "{:<9} {:<19} {:<14} {:<12} {:<10} {:<6} {}",
                        short_id(&record.id),
                        record.started_at,
                        trim(&record.server_name, 12),
                        trim(&record.database, 10),
                        human_size(record.dump_size),
                        status_label(record.status),
                        format_duration(record.duration_ms)
                    );
                }
                Ok(())
            }
            "container" => {
                let list = control.container_records(*limit, None).await?;
                if cli.json {
                    return print_json(&list);
                }
                if list.is_empty() {
                    output::dim("控制机上还没有容器备份记录");
                    return Ok(());
                }
                println!(
                    "{:<9} {:<19} {:<14} {:<12} {:<10} {:<6} {}",
                    "ID", "时间", "项目", "来源", "包", "状态", "耗时"
                );
                for record in list {
                    println!(
                        "{:<9} {:<19} {:<14} {:<12} {:<10} {:<6} {}",
                        short_id(&record.id),
                        record.started_at,
                        trim(&record.project, 12),
                        trim(&record.server_name, 10),
                        human_size(record.bundle_size),
                        status_label(record.status),
                        format_duration(record.duration_ms)
                    );
                }
                Ok(())
            }
            other => Err(CoreError::config(format!(
                "--kind 只接受 backup 或 container，收到: {other}"
            ))),
        },
        AgentCommand::Logs { lines } => {
            let logs = control.logs(*lines, None).await?;
            if cli.json {
                return print_json(&logs);
            }
            for line in logs {
                output::info(line);
            }
            Ok(())
        }
        AgentCommand::Prune { server } => {
            // 先列后删：命令行也看得见删了什么。两步之间孤儿不可能重新被记录认领
            // （记录只认自己生成的包），而删除那一步 agent 还会再核一遍并拿任务锁。
            let orphans = control.prune_list(server.as_deref()).await?;
            if !cli.json {
                for item in &orphans {
                    output::info(format!(
                        "{:<10} {:<10} {}",
                        item.kind,
                        human_size(item.size_bytes),
                        item.file_name
                    ));
                }
            }
            let report = if orphans.is_empty() {
                AgentPruneReport {
                    deleted: 0,
                    freed_bytes: 0,
                }
            } else {
                control.prune(server.as_deref()).await?
            };
            if cli.json {
                return print_json(&report);
            }
            if report.deleted == 0 {
                output::dim("控制机上没有未认领的备份包");
            } else {
                output::success(format!(
                    "已删除 {} 个未认领备份包，释放 {}",
                    report.deleted,
                    human_size(report.freed_bytes)
                ));
            }
            Ok(())
        }
        AgentCommand::Location { config, at, kind } => {
            let location = match at.trim().to_ascii_lowercase().as_str() {
                "remote" => RunLocation::Remote,
                "local" => RunLocation::Local,
                other => {
                    return Err(CoreError::config(format!(
                        "执行位只接受 remote 或 local，收到: {other}"
                    )))
                }
            };
            let saved = store.mutate_config(|app| match kind.as_str() {
                "backup" => {
                    let item = Store::find_backup_config(app, config)?.clone();
                    for entry in app.backup_configs.iter_mut() {
                        if entry.id == item.id {
                            entry.run_location = location;
                        }
                    }
                    Ok((item.id, item.name))
                }
                "container" => {
                    let item = Store::find_container_config(app, config)?.clone();
                    for entry in app.container_configs.iter_mut() {
                        if entry.id == item.id {
                            entry.run_location = location;
                        }
                    }
                    Ok((item.id, item.name))
                }
                other => Err(CoreError::config(format!(
                    "--kind 只接受 backup 或 container，收到: {other}"
                ))),
            })?;
            if cli.json {
                return print_json(&serde_json::json!({
                    "id": saved.0,
                    "name": saved.1,
                    "runLocation": location,
                }));
            }
            output::success(format!(
                "已把「{}」的执行位改成{}",
                saved.1,
                match location {
                    RunLocation::Remote => "控制机（定时与包都在控制机）",
                    RunLocation::Local => "本机",
                }
            ));
            output::dim("改完记得 `agent sync` 下发，否则控制机那边还是旧的一份");
            Ok(())
        }
        AgentCommand::Run { kind, config } => match kind.as_str() {
            "backup" => {
                // 名额与本机 / GUI 用同一把锁：命令行发起的远端任务期间，本机也别再开一条。
                let _lock = claim_task_lock(&store, "backup")?;
                let (sender, mut receiver) = mpsc::unbounded_channel::<BackupEvent>();
                let json = cli.json;
                let printer = tokio::spawn(async move {
                    while let Some(event) = receiver.recv().await {
                        print_backup_event(event, json);
                    }
                });
                let result = control
                    .run_backup_config(config.trim(), &mut |event| {
                        let _ = sender.send(event);
                    }, None)
                    .await;
                drop(sender);
                let _ = printer.await;
                result?;
                Ok(())
            }
            "container" => {
                let _lock = claim_task_lock(&store, "container")?;
                let (sender, mut receiver) = mpsc::unbounded_channel::<ContainerEvent>();
                let json = cli.json;
                let printer = tokio::spawn(async move {
                    while let Some(event) = receiver.recv().await {
                        print_container_event(event, json);
                    }
                });
                let result = control
                    .run_container_config(config.trim(), &mut |event| {
                        let _ = sender.send(event);
                    }, None)
                    .await;
                drop(sender);
                let _ = printer.await;
                result?;
                Ok(())
            }
            other => Err(CoreError::config(format!(
                "--kind 只接受 backup 或 container，收到: {other}"
            ))),
        },
        AgentCommand::Restore {
            record,
            server,
            dir,
            no_start,
        } => {
            let target = lookup_server(&store, server)?.id;
            let _lock = claim_task_lock(&store, "container")?;
            let (sender, mut receiver) = mpsc::unbounded_channel::<ContainerEvent>();
            let json = cli.json;
            let printer = tokio::spawn(async move {
                while let Some(event) = receiver.recv().await {
                    print_container_event(event, json);
                }
            });
            let result = control
                .restore_container_bundle(
                    record.trim(),
                    &target,
                    dir.as_deref().unwrap_or(""),
                    !*no_start,
                    &mut |event| {
                        let _ = sender.send(event);
                    },
                    None,
                )
                .await;
            drop(sender);
            let _ = printer.await;
            result?;
            Ok(())
        }
    }
}

fn print_status(status: &AgentStatus) {
    output::info(format!(
        "控制机 {} · agent {} · 协议 {}",
        status.data_dir, status.version, status.proto
    ));
    output::info(format!(
        "本地时间 {} ({}) · 磁盘余量 {} / 共 {}（水位线 {}）",
        status.local_time,
        status.timezone,
        human_size(status.free_bytes),
        human_size(status.total_bytes),
        human_size(status.floor_bytes)
    ));
    output::info(format!(
        "已留备份包 {} 个，共 {}",
        status.bundle_count,
        human_size(status.bundle_bytes)
    ));
    output::info(format!(
        "配置：服务器 {} 台 · 数据库备份 {} 条 · 容器备份 {} 条",
        status.servers, status.backup_configs, status.container_configs
    ));
    let backup = if status.backup_enabled {
        format!(
            "{} 跑「{}」（上次触发 {}）",
            status.backup_time,
            if status.backup_config_name.is_empty() {
                "(未选配置)"
            } else {
                status.backup_config_name.as_str()
            },
            if status.backup_last_run.is_empty() {
                "从未"
            } else {
                status.backup_last_run.as_str()
            }
        )
    } else {
        "未开启".to_string()
    };
    output::info(format!("定时数据库备份: {backup}"));
    let container = if status.container_enabled {
        format!(
            "{} 排 {} 项（上次触发 {}）",
            status.container_time,
            status.container_queue,
            if status.container_last_run.is_empty() {
                "从未"
            } else {
                status.container_last_run.as_str()
            }
        )
    } else {
        "未开启".to_string()
    };
    output::info(format!("定时容器备份: {container}"));
    if status.free_bytes < status.floor_bytes {
        output::error("磁盘已低于水位线，控制机会拒绝新的备份任务，先清理备份包");
    }
}

fn print_backup_event(event: BackupEvent, json: bool) {
    if json {
        if let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
        return;
    }
    match event {
        BackupEvent::Log { level, message } => output::deploy_log(level, &message),
        BackupEvent::Progress { message, .. } => output::progress(&message),
        BackupEvent::Finished { record } => {
            output::progress_done();
            match record.status {
                DeployStatus::Failed => output::error(format!(
                    "备份失败: {}",
                    record.error.unwrap_or_else(|| "远端没有给出原因".to_string())
                )),
                other => output::success(format!(
                    "备份结束（{}，{}，包 {}）",
                    status_label(other),
                    format_duration(record.duration_ms),
                    if record.bundle_path.is_empty() {
                        "未留存"
                    } else {
                        record.bundle_path.as_str()
                    }
                )),
            }
        }
        BackupEvent::Started { record_id } => {
            output::dim(format!("控制机记录: {}", short_id(&record_id)))
        }
    }
}

fn print_container_event(event: ContainerEvent, json: bool) {
    if json {
        if let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
        return;
    }
    match event {
        ContainerEvent::Log { level, message } => output::deploy_log(level, &message),
        ContainerEvent::Progress { message, .. } => output::progress(&message),
        ContainerEvent::Finished { record } => {
            output::progress_done();
            match record.status {
                DeployStatus::Failed => output::error(format!(
                    "任务失败: {}",
                    record.error.unwrap_or_else(|| "远端没有给出原因".to_string())
                )),
                other => output::success(format!(
                    "任务结束（{}，{}，包 {}）",
                    status_label(other),
                    format_duration(record.duration_ms),
                    human_size(record.bundle_size)
                )),
            }
        }
        ContainerEvent::Started { record_id } => {
            output::dim(format!("控制机记录: {}", short_id(&record_id)))
        }
    }
}

fn status_label(status: DeployStatus) -> &'static str {
    match status {
        DeployStatus::Success => "成功",
        DeployStatus::Failed => "失败",
        DeployStatus::Running => "进行中",
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn trim(value: &str, width: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= width {
        return value.to_string();
    }
    chars.iter().take(width.saturating_sub(1)).collect::<String>() + "…"
}
