use std::sync::Arc;

use deploy_core::models::{DeployRequest, DeployStatus};
use deploy_core::{CoreError, Git, Result, Store};

use crate::cli::*;
use crate::output;

use super::{
    claim_task_lock, find_record, format_duration, open_store, print_json, run_deploy, short_id,
    truncate,
};

pub(super) async fn deploy_command(cli: &Cli, args: &DeployArgs) -> Result<()> {
    let store = Arc::new(open_store(cli)?);
    let config = store.load_config()?;

    // 已保存的部署配置作为默认值来源；命令行显式参数优先，保证脚本化调用不被配置覆盖。
    let saved = match args
        .config
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(key) => Some(Store::find_deploy_config(&config, key)?.clone()),
        None => None,
    };
    let repo_key = args
        .repo
        .clone()
        // 显式空串（脚本变量展开）按未提供处理，回退到配置里的仓库。
        .filter(|value| !value.trim().is_empty())
        .or_else(|| saved.as_ref().map(|item| item.repo_id.clone()))
        .ok_or_else(|| CoreError::config("请指定仓库，或使用 --config 指定已保存的部署配置"))?;
    let repo = Store::find_repo(&config, &repo_key)?.clone();

    let git = Git::open(&repo.path)?;
    // 显式 --rev 优先；配置里的 rev 空值表示「当前工作区」，不能当成未提供而回退到当前分支。
    let rev = match &args.rev {
        Some(rev) => rev.clone(),
        None => match &saved {
            Some(item) => item.rev.clone(),
            None => {
                // 空仓库（没有任何提交）没有可部署的分支：与 GUI 一致，直接打包当前工作区。
                if git.resolve("HEAD").is_err() {
                    String::new()
                } else {
                    git.current_branch()?
                }
            }
        },
    };

    let server = if let Some(key) = args
        .server
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Store::find_server(&config, key)?.clone()
    } else if let Some(key) = saved.as_ref().map(|item| item.server_id.clone()) {
        Store::find_server(&config, &key)?.clone()
    } else if let Some(default_id) = &repo.default_server_id {
        Store::find_server(&config, default_id)
            .map(|server| server.clone())
            .map_err(|_| CoreError::config("仓库配置的默认服务器不存在，请用 --server 指定"))?
    } else {
        return Err(CoreError::config("请使用 --server 指定部署服务器"));
    };

    let target_dir = args
        .dir
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            saved
                .as_ref()
                .map(|item| item.target_dir.clone())
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| Some(repo.default_target_dir.clone()).filter(|value| !value.trim().is_empty()))
        .or_else(|| {
            Some(server.default_target_dir.clone()).filter(|value| !value.trim().is_empty())
        })
        .ok_or_else(|| CoreError::config("请使用 --dir 指定部署目录"))?;

    let request = DeployRequest {
        repo_id: repo.id.clone(),
        rev,
        server_id: server.id.clone(),
        target_dir,
        // 显式 --script 表示用户明确要执行脚本，即使全局设置或配置关闭了脚本执行。
        run_scripts: if args.no_scripts {
            false
        } else {
            !args.script.is_empty()
                || saved
                    .as_ref()
                    .map(|item| item.run_scripts)
                    .unwrap_or(config.settings.run_scripts)
        },
        script_dir: args
            .script_dir
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                saved
                    .as_ref()
                    .map(|item| item.script_dir.clone())
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| config.settings.script_dir.clone()),
        scripts: if args.script.is_empty() {
            saved.as_ref().map(|item| item.scripts.clone()).unwrap_or_default()
        } else {
            args.script.clone()
        },
        upload_env: if args.no_env {
            false
        } else {
            saved.as_ref().map(|item| item.upload_env).unwrap_or(true)
        },
    };

    let record = run_deploy(&store, request, cli.json).await?;
    if record.status == DeployStatus::Failed {
        std::process::exit(2);
    }
    Ok(())
}

pub(super) async fn release_command(cli: &Cli, command: &ReleaseCommand) -> Result<()> {
    let store = open_store(cli)?;
    let config = store.load_config()?;
    match command {
        ReleaseCommand::List { server, dir } => {
            let server = Store::find_server(&config, server)?.clone();
            let releases =
                deploy_core::release::list_releases(&server, dir, config.settings.connect_timeout_secs)
                    .await?;
            if cli.json {
                return print_json(&releases);
            }
            if releases.is_empty() {
                output::dim("暂无历史版本（该目录还没有 releases/ 版本或未开启原子发布）");
                return Ok(());
            }
            println!("{:<44} {:<22} 当前", "版本", "时间");
            for item in releases {
                println!(
                    "{:<44} {:<22} {}",
                    item.name,
                    item.modified,
                    if item.current { "✓" } else { "" }
                );
            }
            Ok(())
        }
        ReleaseCommand::Switch { server, dir, release } => {
            let _lock = claim_task_lock(&store, "deploy")?;
            let server = Store::find_server(&config, server)?.clone();
            let message =
                deploy_core::release::switch_release(&server, dir, release, config.settings.connect_timeout_secs)
                    .await?;
            output::success(message);
            Ok(())
        }
    }
}

pub(super) async fn history_command(cli: &Cli, command: &HistoryCommand) -> Result<()> {
    let store = open_store(cli)?;
    match command {
        HistoryCommand::List { repo, limit } => {
            let mut records = store.load_history()?;
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
                output::dim("暂无部署记录");
                return Ok(());
            }
            println!(
                "{:<9} {:<19} {:<16} {:<20} {:<12} {:<6} {}",
                "ID", "时间", "仓库", "版本", "服务器", "状态", "耗时"
            );
            for record in records {
                let status = match record.status {
                    DeployStatus::Success => "成功",
                    DeployStatus::Failed => "失败",
                    DeployStatus::Running => "进行中",
                };
                let rev = if record.worktree {
                    format!("{} 工作区", record.branch)
                } else {
                    format!("{} {}", record.branch, record.commit_short)
                };
                println!(
                    "{:<9} {:<19} {:<16} {:<20} {:<12} {:<6} {}",
                    short_id(&record.id),
                    record.started_at,
                    truncate(&record.repo_name, 14),
                    truncate(&rev, 18),
                    truncate(&record.server_name, 10),
                    status,
                    format_duration(record.duration_ms),
                );
            }
            Ok(())
        }
        HistoryCommand::Show { record } => {
            let record = find_record(&store, record)?;
            if cli.json {
                return print_json(&record);
            }
            println!("记录 ID   : {}", record.id);
            println!("仓库      : {} ({})", record.repo_name, record.repo_id);
            if record.worktree {
                let base = if record.commit_short.is_empty() {
                    String::new()
                } else {
                    format!("（HEAD {} {}）", record.commit_short, record.commit_subject)
                };
                println!("版本      : 当前工作区（含未提交改动）{base}");
            } else {
                println!(
                    "版本      : {} [{}] {}",
                    record.commit_short, record.rev, record.commit_subject
                );
            }
            println!("提交      : {}", record.commit);
            println!(
                "服务器    : {} -> {}",
                record.server_name, record.target_dir
            );
            println!(
                "状态      : {}",
                match record.status {
                    DeployStatus::Success => "成功",
                    DeployStatus::Failed => "失败",
                    DeployStatus::Running => "进行中",
                }
            );
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
        HistoryCommand::Redeploy { record } => {
            let record = find_record(&store, record)?;
            let request = DeployRequest {
                repo_id: record.repo_id.clone(),
                // 工作区部署没有固定提交，重新部署时同样打包当前工作区。
                rev: if record.worktree {
                    String::new()
                } else {
                    record.commit.clone()
                },
                server_id: record.server_id.clone(),
                target_dir: record.target_dir.clone(),
                run_scripts: record.run_scripts,
                script_dir: record.script_dir.clone(),
                scripts: record.scripts.clone(),
                upload_env: true,
            };
            let record = run_deploy(&Arc::new(store), request, cli.json).await?;
            if record.status == DeployStatus::Failed {
                std::process::exit(2);
            }
            Ok(())
        }
        HistoryCommand::Clear { yes } => {
            if !yes {
                return Err(CoreError::config("清空记录不可恢复，确认请加 --yes"));
            }
            store.clear_history()?;
            output::success("已清空部署记录");
            Ok(())
        }
    }
}
