use deploy_core::models::RepoConfig;
use deploy_core::{repo_info, CoreError, Git, Result, Store};

use crate::cli::*;
use crate::output;

use super::{open_store, print_json};

pub(super) fn repo_command(cli: &Cli, command: &RepoCommand) -> Result<()> {
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
                // 与 GUI 一致：仓库已移除，指向它的部署配置不可用（部署记录保留作历史）。
                config.deploy_configs.retain(|saved| saved.repo_id != target);
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
