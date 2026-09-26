use std::sync::Arc;

use deploy_core::models::{
    ContainerEvent, ContainerRecord, ContainerRequest, ContainerRestoreRequest, ContainerTarget,
    DeployStatus,
};
use deploy_core::{ContainerEngine, CoreError, Result, Store};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

use super::{claim_task_lock, human_size, open_store, print_json, short_id};

pub(super) async fn container_command(cli: &Cli, command: &ContainerCommand) -> Result<()> {
    let store = Arc::new(open_store(cli)?);
    let engine = ContainerEngine::new(store.clone());
    match command {
        ContainerCommand::Dir => {
            let dir = engine.bundle_dir();
            std::fs::create_dir_all(&dir).map_err(|e| CoreError::io_path(&dir, e))?;
            if cli.json {
                print_json(&dir.to_string_lossy().into_owned())
            } else {
                output::info(dir.display().to_string());
                Ok(())
            }
        }
        ContainerCommand::Ls { server } => {
            let target = lookup_server(&store, server)?;
            let stacks = engine.list_stacks(&target).await?;
            if cli.json {
                return print_json(&stacks);
            }
            if stacks.is_empty() {
                output::info("该服务器上没有 docker compose 管理的项目");
                return Ok(());
            }
            for stack in stacks {
                output::info(format!(
                    "{} · {}/{} 服务 · {}{}",
                    stack.name,
                    stack.running,
                    stack.services,
                    stack.working_dir,
                    if stack.status.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", stack.status)
                    }
                ));
            }
            Ok(())
        }
        ContainerCommand::Inspect { server, project } => {
            let target = lookup_server(&store, server)?;
            let detail = engine.inspect_stack(&target, project).await?;
            if cli.json {
                return print_json(&detail);
            }
            output::info(format!(
                "项目 {} · 目录 {} · compose 命令: {}",
                detail.stack.name, detail.stack.working_dir, detail.compose_command
            ));
            for file in &detail.stack.files {
                output::dim(format!("配置 {file}"));
            }
            for env in &detail.env_files {
                output::dim(format!("env  {env}"));
            }
            for service in &detail.services {
                output::info(format!(
                    "  服务 {} · {} · {}{}",
                    service.name,
                    service.image,
                    service.state,
                    if service.ports.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", service.ports)
                    }
                ));
            }
            for volume in &detail.volumes {
                output::info(format!(
                    "  卷 {} · {}{}",
                    volume.name,
                    human_size(volume.size_bytes),
                    if volume.readable { "" } else { " · 需容器内导出" }
                ));
            }
            output::info(format!(
                "预计体积 {}（目录 {} · 卷 {} · 镜像 {}）",
                human_size(detail.project_bytes + detail.volume_bytes + detail.image_bytes),
                human_size(detail.project_bytes),
                human_size(detail.volume_bytes),
                human_size(detail.image_bytes)
            ));
            for warning in &detail.warnings {
                output::dim(format!("提示: {warning}"));
            }
            Ok(())
        }
        ContainerCommand::Backup(args) => {
            let request = ContainerRequest {
                server_id: lookup_server(&store, &args.server)?.id,
                project: args.project.clone(),
                pause_source: args.pause,
                include_volumes: !args.no_volumes,
                include_images: !args.no_images,
                target: None,
            };
            run_container(cli, &store, &engine, request).await
        }
        ContainerCommand::Migrate(args) => {
            let source = lookup_server(&store, &args.server)?;
            let target = lookup_server(&store, &args.to)?;
            let request = ContainerRequest {
                server_id: source.id,
                project: args.project.clone(),
                pause_source: args.pause,
                include_volumes: !args.no_volumes,
                include_images: !args.no_images,
                target: Some(ContainerTarget {
                    server_id: target.id,
                    target_dir: args.dir.clone().unwrap_or_default(),
                    start_services: !args.no_start,
                }),
            };
            run_container(cli, &store, &engine, request).await
        }
        ContainerCommand::Restore(args) => {
            let target = lookup_server(&store, &args.to)?;
            let request = ContainerRestoreRequest {
                bundle_path: args.bundle.clone(),
                target: ContainerTarget {
                    server_id: target.id,
                    target_dir: args.dir.clone().unwrap_or_default(),
                    start_services: !args.no_start,
                },
            };
            // 与 GUI / 其他命令行进程互斥：同一时间只能跑一个容器任务。
            let _lock = claim_task_lock(&store, "container")?;
            let (record, job) = engine.prepare_restore(&request)?;
            finish(cli, &engine, record, job).await
        }
        ContainerCommand::List { limit } => {
            let mut records = store.load_containers()?;
            records.reverse();
            records.truncate(*limit);
            if cli.json {
                return print_json(&records);
            }
            if records.is_empty() {
                output::info("暂无容器备份 / 迁移记录");
                return Ok(());
            }
            for record in records {
                output::info(format!(
                    "{} · {} · {}{} · {} · {}",
                    short_id(&record.id),
                    record.project,
                    record_label(&record),
                    record.started_at,
                    status_label(&record.status),
                    human_size(record.bundle_size)
                ));
            }
            Ok(())
        }
        ContainerCommand::Show { record } => {
            let found = find_record(&store, record)?;
            if cli.json {
                return print_json(&found);
            }
            output::info(format!(
                "{} · {} · {} · {}",
                found.id,
                record_label(&found),
                found.project,
                status_label(&found.status)
            ));
            output::info(format!("开始: {} · 备份包: {}", found.started_at, found.bundle_path));
            if !found.target_dir.is_empty() {
                output::info(format!("目标: {} -> {}", found.target_server_name, found.target_dir));
            }
            if let Some(error) = &found.error {
                output::error(error.clone());
            }
            output::info(found.log.clone());
            Ok(())
        }
        ContainerCommand::Clear { yes } => {
            if !yes {
                return Err(CoreError::config("清空容器记录需要 --yes 确认"));
            }
            store.clear_container_records()?;
            output::success("已清空容器记录（本机备份包文件未删除）");
            Ok(())
        }
    }
}

fn lookup_server(store: &Store, key: &str) -> Result<deploy_core::ServerConfig> {
    let config = store.load_config()?;
    Ok(Store::find_server(&config, key)?.clone())
}

fn find_record(store: &Store, key: &str) -> Result<ContainerRecord> {
    let records = store.load_containers()?;
    records
        .into_iter()
        .find(|record| record.id == key || record.id.starts_with(key))
        .ok_or_else(|| CoreError::not_found(format!("容器记录不存在: {key}")))
}

fn record_label(record: &ContainerRecord) -> String {
    let kind = match record.kind {
        deploy_core::ContainerRecordKind::Backup => "备份",
        deploy_core::ContainerRecordKind::Migrate => "迁移",
        deploy_core::ContainerRecordKind::Restore => "恢复",
    };
    if record.target_server_name.is_empty() {
        format!("{kind} {} -> 本机", record.server_name)
    } else if record.server_name.is_empty() {
        format!("{kind} 本机 -> {}", record.target_server_name)
    } else {
        format!("{kind} {} -> {}", record.server_name, record.target_server_name)
    }
}

fn status_label(status: &DeployStatus) -> &'static str {
    match status {
        DeployStatus::Success => "成功",
        DeployStatus::Failed => "失败",
        DeployStatus::Running => "进行中",
    }
}

/// 快照 / 迁移：与 GUI 走同一条引擎路径，先抢任务锁再 prepare。
async fn run_container(
    cli: &Cli,
    store: &Arc<Store>,
    engine: &ContainerEngine,
    request: ContainerRequest,
) -> Result<()> {
    let _lock = claim_task_lock(store, "container")?;
    let (record, job) = engine.prepare(&request)?;
    finish(cli, engine, record, job).await
}

async fn finish(
    cli: &Cli,
    engine: &ContainerEngine,
    record: ContainerRecord,
    job: deploy_core::ContainerJob,
) -> Result<()> {
    let json = cli.json;
    let (sender, mut receiver) = mpsc::unbounded_channel::<ContainerEvent>();
    let printer = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            if json {
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
                continue;
            }
            match event {
                ContainerEvent::Log { level, message } => output::deploy_log(level, &message),
                ContainerEvent::Progress { message, .. } => output::progress(&message),
                _ => {}
            }
        }
    });

    let record = engine.run(record, job, Some(sender)).await;
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
