use std::sync::Arc;

use deploy_core::models::{
    DeployEvent, DeployRecord, DeployRequest, DeployStatus, RepoConfig, SshAuth,
};
use deploy_core::{repo_info, CoreError, DeployEngine, Git, Result, Store};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

pub async fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Repo(command) => repo_command(cli, command),
        Command::Branch(command) => branch_command(cli, command),
        Command::Server(command) => server_command(cli, command).await,
        Command::Deploy(args) => deploy_command(cli, args).await,
        Command::History(command) => history_command(cli, command).await,
        Command::Where => {
            let store = open_store(cli)?;
            output::info(store.base_dir().display().to_string());
            Ok(())
        }
    }
}

fn open_store(cli: &Cli) -> Result<Store> {
    match &cli.data_dir {
        Some(dir) => Ok(Store::new(dir)),
        None => Store::default_store(),
    }
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    output::info(serde_json::to_string_pretty(value)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// repo
// ---------------------------------------------------------------------------

fn repo_command(cli: &Cli, command: &RepoCommand) -> Result<()> {
    let store = open_store(cli)?;
    match command {
        RepoCommand::Add(args) => {
            let mut config = store.load_config()?;
            Git::open(&args.path)?;

            let path = deploy_core::process::canonicalize_path(&args.path);
            let name = args
                .name
                .clone()
                .or_else(|| {
                    path.file_name()
                        .map(|value| value.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "repo".to_string());

            if config.repos.iter().any(|repo| repo.name == name) {
                return Err(CoreError::config(format!("仓库名称已存在: {name}")));
            }
            let normalized = path.to_string_lossy().replace('\\', "/");
            if config.repos.iter().any(|repo| {
                repo.path.replace('\\', "/").trim_end_matches('/') == normalized.trim_end_matches('/')
            }) {
                return Err(CoreError::config(format!("仓库已添加: {}", path.display())));
            }

            let default_server_id = match &args.server {
                Some(key) => Some(Store::find_server(&config, key)?.id.clone()),
                None => None,
            };

            let mut repo = RepoConfig::new(name, path.to_string_lossy().into_owned());
            repo.default_server_id = default_server_id;
            repo.default_target_dir = args.dir.clone().unwrap_or_default();
            config.repos.push(repo.clone());
            store.save_config(&config)?;

            if cli.json {
                return print_json(&repo_info(&repo));
            }
            output::success(format!("已添加仓库 {} ({})", repo.name, repo.path));
            Ok(())
        }
        RepoCommand::List => {
            let config = store.load_config()?;
            let infos: Vec<_> = config.repos.iter().map(repo_info).collect();
            if cli.json {
                return print_json(&infos);
            }
            if infos.is_empty() {
                output::dim("暂无仓库，使用 `repo add <路径>` 添加");
                return Ok(());
            }
            for info in infos {
                let state = if info.is_repo {
                    format!("{} · {} 个变动", info.current_branch, info.change_count)
                } else {
                    "路径不可用".to_string()
                };
                println!("{:<20} {:<28} {}", info.name, state, info.path);
            }
            Ok(())
        }
        RepoCommand::Remove { repo } => {
            let mut config = store.load_config()?;
            let target = Store::find_repo(&config, repo)?.id.clone();
            config.repos.retain(|item| item.id != target);
            store.save_config(&config)?;
            output::success(format!("已移除仓库 {repo}"));
            Ok(())
        }
        RepoCommand::Info { repo } => {
            let config = store.load_config()?;
            let repo = Store::find_repo(&config, repo)?.clone();
            let info = repo_info(&repo);
            if cli.json {
                return print_json(&info);
            }
            println!("名称      : {}", info.name);
            println!("路径      : {}", info.path);
            println!("当前分支  : {}", info.current_branch);
            println!(
                "远端      : {}",
                info.remote.as_deref().unwrap_or("未配置")
            );
            println!("变动文件  : {}", info.change_count);
            println!(
                "默认服务器: {}",
                info.default_server_id.as_deref().unwrap_or("-")
            );
            println!(
                "默认目录  : {}",
                if info.default_target_dir.is_empty() {
                    "-"
                } else {
                    &info.default_target_dir
                }
            );
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// branch
// ---------------------------------------------------------------------------

fn branch_command(cli: &Cli, command: &BranchCommand) -> Result<()> {
    let store = open_store(cli)?;
    let config = store.load_config()?;
    match command {
        BranchCommand::List { repo, all } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let branches = git.branches(*all)?;
            if cli.json {
                return print_json(&branches);
            }
            for branch in branches {
                let marker = if branch.is_current { "*" } else { " " };
                let mut tags = Vec::new();
                if branch.is_remote {
                    tags.push("远程");
                }
                if let Some(upstream) = &branch.upstream {
                    if !branch.is_remote {
                        tags.push(upstream.as_str());
                    }
                }
                let suffix = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", tags.join(" · "))
                };
                println!(
                    "{marker} {:<36} {:<17} {}{suffix}",
                    branch.name, branch.last_commit_date, branch.last_commit_subject
                );
            }
            Ok(())
        }
        BranchCommand::Switch { repo, branch } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.checkout(branch)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Create {
            repo,
            name,
            from,
            no_checkout,
        } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.create_branch(name, from.as_deref(), !no_checkout)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Delete {
            repo,
            branch,
            force,
        } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            if git.current_branch().unwrap_or_default() == *branch {
                return Err(CoreError::git("不能删除当前所在分支"));
            }
            let message = git.delete_branch(branch, *force)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Log { repo, limit } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let commits = git.log(*limit)?;
            if cli.json {
                return print_json(&commits);
            }
            for commit in commits {
                println!(
                    "{:<10} {:<17} {:<14} {}",
                    commit.short, commit.date, commit.author, commit.subject
                );
            }
            Ok(())
        }
        BranchCommand::Status { repo } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let status = git.status()?;
            if cli.json {
                return print_json(&status);
            }
            let upstream = status
                .upstream
                .as_deref()
                .map(|value| format!(" -> {value}"))
                .unwrap_or_default();
            println!("分支: {}{upstream}", status.branch);
            if status.ahead > 0 || status.behind > 0 {
                println!("领先 {} / 落后 {}", status.ahead, status.behind);
            }
            if status.changes.is_empty() {
                output::dim("工作区干净");
            } else {
                for change in status.changes {
                    println!("  {:<8} {}", change.status, change.path);
                }
            }
            Ok(())
        }
        BranchCommand::Commit { repo, message } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let result = git.commit_all(message)?;
            output::success(result);
            Ok(())
        }
        BranchCommand::Reset { repo, rev, yes } => {
            if !yes {
                return Err(CoreError::git(
                    "该操作会丢弃未提交的工作区改动，确认请加 --yes",
                ));
            }
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.reset_hard(rev)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Fetch { repo } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.fetch()?;
            output::info(message);
            Ok(())
        }
        BranchCommand::Pull { repo } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.pull()?;
            output::info(message);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// server
// ---------------------------------------------------------------------------

async fn server_command(cli: &Cli, command: &ServerCommand) -> Result<()> {
    let store = open_store(cli)?;
    match command {
        ServerCommand::Add(args) => {
            let mut config = store.load_config()?;
            let auth = match (&args.password, &args.key) {
                (Some(_), Some(_)) => {
                    return Err(CoreError::config("--password 与 --key 只能提供一个"))
                }
                (Some(password), None) => SshAuth::Password {
                    password: password.clone(),
                },
                (None, Some(key)) => SshAuth::PrivateKey {
                    key_path: key.to_string_lossy().into_owned(),
                    passphrase: args.passphrase.clone(),
                },
                (None, None) => {
                    return Err(CoreError::config("请提供 --password 或 --key 进行认证"))
                }
            };

            let mut server = match config.servers.iter().find(|item| item.name == args.name) {
                Some(existing) => existing.clone(),
                None => deploy_core::models::ServerConfig::new(
                    args.name.clone(),
                    args.host.clone(),
                    args.user.clone(),
                    auth.clone(),
                ),
            };
            server.host = args.host.clone();
            server.port = args.port;
            server.username = args.user.clone();
            server.auth = auth;
            if let Some(dir) = &args.dir {
                server.default_target_dir = dir.clone();
            }

            let is_new = !config.servers.iter().any(|item| item.id == server.id);
            Store::upsert_server(&mut config, server.clone())?;
            store.save_config(&config)?;

            if cli.json {
                return print_json(&server);
            }
            output::success(format!(
                "{}服务器 {} ({}@{})",
                if is_new { "已添加" } else { "已更新" },
                server.name,
                server.username,
                server.host
            ));
            Ok(())
        }
        ServerCommand::List => {
            let config = store.load_config()?;
            if cli.json {
                return print_json(&config.servers);
            }
            if config.servers.is_empty() {
                output::dim("暂无服务器，使用 `server add` 添加");
                return Ok(());
            }
            for server in &config.servers {
                let auth = match &server.auth {
                    SshAuth::Password { .. } => "密码",
                    SshAuth::PrivateKey { .. } => "私钥",
                };
                println!(
                    "{:<16} {}@{}:{:<5} {:<4} {}",
                    server.name,
                    server.username,
                    server.host,
                    server.port,
                    auth,
                    server.default_target_dir
                );
            }
            Ok(())
        }
        ServerCommand::Remove { server } => {
            let mut config = store.load_config()?;
            let id = Store::find_server(&config, server)?.id.clone();
            config.servers.retain(|item| item.id != id);
            store.save_config(&config)?;
            output::success(format!("已删除服务器 {server}"));
            Ok(())
        }
        ServerCommand::Test { server } => {
            let config = store.load_config()?;
            let server = Store::find_server(&config, server)?.clone();
            let engine = DeployEngine::new(Arc::new(store));
            output::info(format!("正在连接 {} ...", server.name));
            let message = engine.test_server(&server).await?;
            output::success(message);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// deploy / history
// ---------------------------------------------------------------------------

async fn deploy_command(cli: &Cli, args: &DeployArgs) -> Result<()> {
    let store = Arc::new(open_store(cli)?);
    let config = store.load_config()?;
    let repo = Store::find_repo(&config, &args.repo)?.clone();

    let git = Git::open(&repo.path)?;
    let rev = match &args.rev {
        Some(rev) => rev.clone(),
        None => git.current_branch()?,
    };

    let server = if let Some(key) = &args.server {
        Store::find_server(&config, key)?.clone()
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
        run_scripts: !args.no_scripts && config.settings.run_scripts,
        script_dir: args
            .script_dir
            .clone()
            .unwrap_or_else(|| config.settings.script_dir.clone()),
        script: args.script.clone(),
    };

    let success = run_deploy(&store, request).await?;
    if !success {
        std::process::exit(2);
    }
    Ok(())
}

async fn history_command(cli: &Cli, command: &HistoryCommand) -> Result<()> {
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
                let rev = format!("{} {}", record.branch, record.commit_short);
                println!(
                    "{:<9} {:<19} {:<16} {:<20} {:<12} {:<6} {}",
                    &record.id[..8.min(record.id.len())],
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
            println!(
                "版本      : {} [{}] {}",
                record.commit_short, record.rev, record.commit_subject
            );
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
                rev: record.commit.clone(),
                server_id: record.server_id.clone(),
                target_dir: record.target_dir.clone(),
                run_scripts: record.run_scripts,
                script_dir: record.script_dir.clone(),
                script: record.script.clone(),
            };
            let success = run_deploy(&Arc::new(store), request).await?;
            if !success {
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

/// 执行部署并把事件实时打印到终端。
async fn run_deploy(store: &Arc<Store>, request: DeployRequest) -> Result<bool> {
    let engine = DeployEngine::new(store.clone());
    let record = engine.prepare(&request)?;
    output::info(format!("部署记录: {}", record.id));

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let printer = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            match event {
                DeployEvent::Log { level, message } => output::deploy_log(level, &message),
                DeployEvent::Progress { message, .. } => output::progress(&message),
                _ => {}
            }
        }
    });

    let final_record = engine.run(record, request, Some(sender)).await;
    let _ = printer.await;
    output::progress_done();

    Ok(final_record.status != DeployStatus::Failed)
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
