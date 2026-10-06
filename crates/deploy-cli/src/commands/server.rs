use std::sync::Arc;

use deploy_core::models::SshAuth;
use deploy_core::{CoreError, DeployEngine, Result, Store};

use crate::cli::*;
use crate::output;

use super::{open_store, print_json, redact_server};

pub(super) async fn server_command(cli: &Cli, command: &ServerCommand) -> Result<()> {
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
                let removed: Vec<String> = config
                    .backup_configs
                    .iter()
                    .filter(|saved| saved.server_id == id)
                    .map(|saved| saved.id.clone())
                    .collect();
                config.backup_configs.retain(|saved| saved.server_id != id);
                // 与 GUI 一致：从各部署配置的目标列表里摘掉这一台，没有剩余目标的配置整条删除。
                Store::detach_server_from_deploy_configs(config, &id);
                // 容器备份配置同样收敛：来源被删的整条删除，只当过迁移目标的降级为纯备份。
                Store::detach_server_from_container_configs(config, &id);
                // 定时备份若引用被删配置，清空引用，避免每天到点报错。
                if config
                    .settings
                    .scheduled_backup_config_id
                    .as_ref()
                    .is_some_and(|config_id| removed.contains(config_id))
                {
                    config.settings.scheduled_backup_config_id = None;
                }
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
