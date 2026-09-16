use std::sync::Arc;

use deploy_core::models::{DeployStatus, PagesEvent, PagesRequest};
use deploy_core::{CoreError, PagesEngine, Result, Store};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

use super::{claim_task_lock, open_store, print_json, short_id, truncate};

pub(super) async fn pages_command(cli: &Cli, command: &PagesCommand) -> Result<()> {
    let store = Arc::new(open_store(cli)?);
    match command {
        PagesCommand::Config {
            repo,
            provider,
            project,
            build,
            output,
            branch,
            publish_branch,
        } => {
            let (repo_name, pages) = store.mutate_config(|config| {
                let repo_id = Store::find_repo(config, repo)?.id.clone();
                let repo_name = config
                    .repos
                    .iter()
                    .find(|item| item.id == repo_id)
                    .map(|item| item.name.clone())
                    .unwrap_or_default();
                let repo_mut = config
                    .repos
                    .iter_mut()
                    .find(|item| item.id == repo_id)
                    .expect("repo exists");

                let mut pages = repo_mut.pages.clone().unwrap_or_default();
                if let Some(value) = provider {
                    pages.provider = value.trim().to_lowercase();
                }
                if let Some(value) = project {
                    pages.project_name = value.trim().to_string();
                }
                if let Some(value) = build {
                    pages.build_command = value.trim().to_string();
                }
                if let Some(value) = output {
                    pages.output_dir = value.trim().to_string();
                }
                if let Some(value) = branch {
                    pages.branch = value.trim().to_string();
                }
                if let Some(value) = publish_branch {
                    pages.publish_branch = value.trim().to_string();
                }
                let pages = pages.normalize();
                if pages.provider != "cloudflare" && pages.provider != "github" {
                    return Err(CoreError::config(
                        "不支持的 Pages 平台（可选 cloudflare / github）",
                    ));
                }
                repo_mut.pages = if pages.provider == "github" || !pages.project_name.is_empty() {
                    Some(pages.clone())
                } else {
                    None
                };
                Ok((repo_name, pages))
            })?;

            if cli.json {
                return print_json(&pages);
            }
            output::success(format!("已更新 {repo_name} 的 Pages 配置"));
            Ok(())
        }
        PagesCommand::Run {
            repo,
            provider,
            project,
            build,
            output,
            branch,
            publish_branch,
            skip_build,
        } => {
            // 与 GUI / 其他命令行进程互斥，避免并发执行破坏性操作。
            let _lock = claim_task_lock(&store, "pages")?;
            let config = store.load_config()?;
            let repo = Store::find_repo(&config, repo)?.clone();
            let engine = PagesEngine::new(store.clone());
            let request = PagesRequest {
                repo_id: repo.id.clone(),
                provider: provider.clone(),
                project_name: project.clone(),
                build_command: build.clone(),
                output_dir: output.clone(),
                branch: branch.clone(),
                publish_branch: publish_branch.clone(),
                skip_build: *skip_build,
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
                output::info(format!("Pages 部署记录: {}", prepared.record.id));
                output::info(format!(
                    "仓库: {} -> 项目 {}（{}）",
                    prepared.record.repo_name, prepared.record.project_name, prepared.record.branch
                ));
                if !skip_build && !prepared.config.build_command.trim().is_empty() {
                    output::info(format!("构建命令: {}", prepared.config.build_command));
                }
            }

            let json = cli.json;
            let skip_build = *skip_build;
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let printer = tokio::spawn(async move {
                while let Some(event) = receiver.recv().await {
                    if json {
                        if let Ok(line) = serde_json::to_string(&event) {
                            println!("{line}");
                        }
                        continue;
                    }
                    if let PagesEvent::Log { level, message } = event {
                        output::deploy_log(level, &message);
                    }
                }
            });

            let record = tokio::task::spawn_blocking(move || {
                engine.run(
                    prepared.record,
                    prepared.config,
                    prepared.repo_path,
                    prepared.token,
                    prepared.account_id,
                    skip_build,
                    Some(sender),
                )
            })
            .await
            .map_err(|e| CoreError::Process(format!("Pages 部署任务异常: {e}")))?;
            let _ = printer.await;

            if json {
                println!("{}", serde_json::to_string(&record)?);
            }

            if record.status == DeployStatus::Failed {
                std::process::exit(2);
            }
            Ok(())
        }
        PagesCommand::List { repo, limit } => {
            let mut records = store.load_pages_records()?;
            records.reverse();
            if let Some(key) = repo {
                let repo_id = Store::find_repo(&store.load_config()?, key)?.id.clone();
                records.retain(|record| record.repo_id == repo_id);
            }
            records.truncate(*limit);

            if cli.json {
                return print_json(&records);
            }
            if records.is_empty() {
                output::dim("暂无 Pages 部署记录");
                return Ok(());
            }
            println!(
                "{:<9} {:<19} {:<12} {:<18} {:<10} {:<6} {}",
                "ID", "时间", "仓库", "项目", "分支", "状态", "地址"
            );
            for record in records {
                let status = match record.status {
                    DeployStatus::Success => "成功",
                    DeployStatus::Failed => "失败",
                    DeployStatus::Running => "进行中",
                };
                println!(
                    "{:<9} {:<19} {:<12} {:<18} {:<10} {:<6} {}",
                    short_id(&record.id),
                    record.started_at,
                    truncate(&record.repo_name, 10),
                    truncate(&record.project_name, 16),
                    truncate(&record.branch, 8),
                    status,
                    record.url.as_deref().unwrap_or("-"),
                );
            }
            Ok(())
        }
        PagesCommand::Show { record } => {
            let record = store.find_pages_record(record)?;
            if cli.json {
                return print_json(&record);
            }
            println!("记录 ID   : {}", record.id);
            println!("仓库      : {} ({})", record.repo_name, record.repo_id);
            println!("项目      : {}（分支 {}）", record.project_name, record.branch);
            if !record.commit_short.is_empty() {
                println!("提交      : {}", record.commit_short);
            }
            println!(
                "状态      : {}",
                match record.status {
                    DeployStatus::Success => "成功",
                    DeployStatus::Failed => "失败",
                    DeployStatus::Running => "进行中",
                }
            );
            if let Some(url) = &record.url {
                println!("地址      : {url}");
            }
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
        PagesCommand::Clear { yes } => {
            if !yes {
                return Err(CoreError::config("清空记录不可恢复，确认请加 --yes"));
            }
            store.clear_pages_records()?;
            output::success("已清空 Pages 部署记录");
            Ok(())
        }
        PagesCommand::Test { repo } => {
            let config = store.load_config()?;
            let repo = Store::find_repo(&config, repo)?.clone();
            if !cli.json {
                output::info(format!("正在检查 {} 的 Pages 环境 ...", repo.name));
            }
            let repo_id = repo.id.clone();
            let engine = PagesEngine::new(store);
            let message = tokio::task::spawn_blocking(move || engine.test(&repo_id, None))
                .await
                .map_err(|e| CoreError::Process(format!("Pages 检查异常: {e}")))??;
            if cli.json {
                return print_json(&serde_json::json!({ "message": message }));
            }
            output::success("Pages 环境连通");
            output::info(message);
            Ok(())
        }
    }
}
