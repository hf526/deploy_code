use std::sync::Arc;

use deploy_core::models::{
    BackupConfig, BackupEvent, BackupRequest, BackupTarget, DbBackupSource, DeployEvent,
    DeployRecord, DeployRequest, DeployStatus, PagesEvent, PagesRequest, RepoConfig, ServerConfig,
    SshAuth,
};
use deploy_core::{
    backup::mask_database_url, repo_info, BackupEngine, CoreError, DeployEngine, Git, PagesEngine,
    Result, Store,
};
use tokio::sync::mpsc;

use crate::cli::*;
use crate::output;

pub async fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Repo(command) => repo_command(cli, command),
        Command::Branch(command) => branch_command(cli, command),
        Command::Server(command) => server_command(cli, command).await,
        Command::Deploy(args) => deploy_command(cli, args).await,
        Command::Release(command) => release_command(cli, command).await,
        Command::History(command) => history_command(cli, command).await,
        Command::Backup(command) => backup_command(cli, command).await,
        Command::Pages(command) => pages_command(cli, command).await,
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
        CoreError::config("已有任务正在进行（GUI 或其他命令行进程），请稍后再试")
    })
}

// ---------------------------------------------------------------------------
// repo
// ---------------------------------------------------------------------------

fn repo_command(cli: &Cli, command: &RepoCommand) -> Result<()> {
    let store = open_store(cli)?;
    match command {
        RepoCommand::Add(args) => {
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
            let normalized = path.to_string_lossy().replace('\\', "/");

            let repo = store.mutate_config(|config| {
                if config.repos.iter().any(|repo| repo.name == name) {
                    return Err(CoreError::config(format!("仓库名称已存在: {name}")));
                }
                if config.repos.iter().any(|repo| {
                    repo.path.replace('\\', "/").trim_end_matches('/')
                        == normalized.trim_end_matches('/')
                }) {
                    return Err(CoreError::config(format!("仓库已添加: {}", path.display())));
                }

                let default_server_id = match &args.server {
                    Some(key) => Some(Store::find_server(config, key)?.id.clone()),
                    None => None,
                };

                let mut repo = RepoConfig::new(name.clone(), path.to_string_lossy().into_owned());
                repo.default_server_id = default_server_id;
                repo.default_target_dir = args.dir.clone().unwrap_or_default();
                config.repos.push(repo.clone());
                Ok(repo)
            })?;

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
                let state = if !info.path_exists {
                    "路径不可用".to_string()
                } else if info.is_repo {
                    format!("{} · {} 个变动", info.current_branch, info.change_count)
                } else {
                    "非 Git 仓库".to_string()
                };
                println!("{:<20} {:<28} {}", info.name, state, info.path);
            }
            Ok(())
        }
        RepoCommand::Remove { repo } => {
            store.mutate_config(|config| {
                let target = Store::find_repo(config, repo)?.id.clone();
                config.repos.retain(|item| item.id != target);
                Ok(())
            })?;
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
        BranchCommand::Commit {
            repo,
            message,
            allow_sensitive,
        } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let result = git.commit_all(message, *allow_sensitive)?;
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
            let auth = match (&args.password, &args.key) {
                (Some(_), Some(_)) => {
                    return Err(CoreError::config("--password 与 --key 只能提供一个"))
                }
                (Some(password), None) => Some(SshAuth::Password {
                    password: password.clone(),
                }),
                (None, Some(key)) => Some(SshAuth::PrivateKey {
                    key_path: key.to_string_lossy().into_owned(),
                    passphrase: args.passphrase.clone(),
                }),
                // 更新已有服务器时允许只改非认证字段，保留原凭据。
                (None, None) => None,
            };

            let (server, is_new) = store.mutate_config(|config| {
                let existing = config
                    .servers
                    .iter()
                    .find(|item| item.name == args.name)
                    .cloned();
                if existing.is_none() && auth.is_none() {
                    return Err(CoreError::config(
                        "新增服务器请提供 --password 或 --key 进行认证",
                    ));
                }
                let mut server = match existing {
                    Some(existing) => existing,
                    None => deploy_core::models::ServerConfig::new(
                        args.name.clone(),
                        args.host.clone(),
                        args.user.clone(),
                        auth.clone().unwrap_or(SshAuth::Password {
                            password: String::new(),
                        }),
                    ),
                };
                server.host = args.host.clone();
                if let Some(port) = args.port {
                    if port == 0 {
                        return Err(CoreError::config("SSH 端口必须在 1-65535 之间"));
                    }
                    server.port = port;
                }
                server.username = args.user.clone();
                if let Some(auth) = &auth {
                    server.auth = auth.clone();
                }
                if let Some(dir) = &args.dir {
                    server.default_target_dir = dir.clone();
                }

                let has_db_args = args.db_mode.is_some()
                    || args.db_container.is_some()
                    || args.db_name.is_some()
                    || args.db_user.is_some()
                    || args.db_password.is_some()
                    || args.db_schema.is_some();
                if has_db_args {
                    let mut source = server.db_backup.clone().unwrap_or_default();
                    if let Some(mode) = &args.db_mode {
                        source.mode = mode.clone();
                    }
                    if let Some(container) = &args.db_container {
                        source.container = container.clone();
                    }
                    if let Some(name) = &args.db_name {
                        source.database = name.clone();
                    }
                    if let Some(user) = &args.db_user {
                        source.username = user.clone();
                    }
                    if let Some(password) = &args.db_password {
                        source.password = password.clone();
                    }
                    if let Some(schema) = &args.db_schema {
                        source.schema = schema.clone();
                    }
                    server.db_backup = Some(source);
                }
                if let Some(url) = &args.supabase_url {
                    server.supabase_url = Some(url.clone());
                }
                if let Some(key) = &args.backup_target {
                    let target = config
                        .backup_targets
                        .iter()
                        .find(|target| target.id == *key || target.name == *key)
                        .ok_or_else(|| CoreError::not_found(format!("备份目标不存在: {key}")))?;
                    server.backup_target_id = Some(target.id.clone());
                }

                let is_new = !config.servers.iter().any(|item| item.id == server.id);
                Store::upsert_server(config, server.clone())?;
                Ok((server, is_new))
            })?;

            if cli.json {
                return print_json(&redact_server(&server));
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
                let servers: Vec<_> = config.servers.iter().map(redact_server).collect();
                return print_json(&servers);
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
            store.mutate_config(|config| {
                let id = Store::find_server(config, server)?.id.clone();
                config.servers.retain(|item| item.id != id);
                // 服务器已删除，其备份配置不再可用（备份记录保留作历史）。
                config.backup_configs.retain(|saved| saved.server_id != id);
                // 清理仓库上的悬空默认服务器引用。
                for repo in &mut config.repos {
                    if repo.default_server_id.as_deref() == Some(id.as_str()) {
                        repo.default_server_id = None;
                    }
                }
                Ok(())
            })?;
            output::success(format!("已删除服务器 {server}"));
            Ok(())
        }
        ServerCommand::Test { server } => {
            let config = store.load_config()?;
            let server = Store::find_server(&config, server)?.clone();
            let engine = DeployEngine::new(Arc::new(store));
            if cli.json {
                let message = engine.test_server(&server).await?;
                return print_json(&serde_json::json!({ "message": message }));
            }
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
        None => {
            // 空仓库（没有任何提交）没有可部署的分支：与 GUI 一致，直接打包当前工作区。
            if git.resolve("HEAD").is_err() {
                String::new()
            } else {
                git.current_branch()?
            }
        }
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
        scripts: args.script.clone(),
        upload_env: !args.no_env,
    };

    let record = run_deploy(&store, request, cli.json).await?;
    if record.status == DeployStatus::Failed {
        std::process::exit(2);
    }
    Ok(())
}

async fn release_command(cli: &Cli, command: &ReleaseCommand) -> Result<()> {
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

// ---------------------------------------------------------------------------
// backup
// ---------------------------------------------------------------------------

/// 解析 `backup run/test` 的目标服务器：`--config` 优先，其次才是显式服务器参数。
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

async fn backup_command(cli: &Cli, command: &BackupCommand) -> Result<()> {
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
                    let supabase_url = args
                        .supabase_url
                        .clone()
                        .or_else(|| existing.as_ref().and_then(|item| item.supabase_url.clone()));

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
                    let before = config.backup_configs.len();
                    config
                        .backup_configs
                        .retain(|item| item.id != *key && item.name != *key);
                    if config.backup_configs.len() == before {
                        return Err(CoreError::not_found(format!("备份配置不存在: {key}")));
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

// ---------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------

async fn pages_command(cli: &Cli, command: &PagesCommand) -> Result<()> {
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
                if pages.provider.is_empty() {
                    pages.provider = "cloudflare".to_string();
                }
                if pages.provider != "cloudflare" && pages.provider != "github" {
                    return Err(CoreError::config(
                        "不支持的 Pages 平台（可选 cloudflare / github）",
                    ));
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
                if pages.output_dir.is_empty() {
                    pages.output_dir = "dist".to_string();
                }
                if pages.branch.is_empty() {
                    pages.branch = "main".to_string();
                }
                if pages.publish_branch.is_empty() {
                    pages.publish_branch = "gh-pages".to_string();
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
