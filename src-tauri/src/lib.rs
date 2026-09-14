mod commands;
mod state;
mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deploy_core::{BackupEngine, DeployEngine, Store};
use state::AppState;
use tauri::{Manager, RunEvent, WindowEvent};

/// 退出清理只执行一次。
static EXIT_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);
/// 退出清理是否已完成：完成后才允许 handle.exit(0) 真正退出。
static EXIT_CLEANUP_DONE: AtomicBool = AtomicBool::new(false);

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let store = Store::default_store()?;
            // 上次异常退出遗留的 Running 记录在此收敛为失败（有其他进程正在跑任务时跳过）。
            let _ = store.reconcile_interrupted();
            // 旧版「服务器单份 db_backup」升级为备份配置列表。
            let _ = store.migrate_backup_configs();
            // 托盘文案先按已保存偏好初始化，前端启动后会按解析出的界面语言再同步一次。
            let language = store
                .load_config()
                .map(|config| config.settings.language)
                .unwrap_or_default();
            app.manage(AppState::new(Arc::new(store)));
            tray::create(app.handle(), &language)?;
            Ok(())
        })
        // 点窗口 X 只隐藏到托盘，程序继续运行；退出请使用托盘菜单。
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_data_dir,
            commands::app::get_settings,
            commands::app::save_settings,
            commands::app::reveal_path,
            commands::tray::set_tray_language,
            commands::repos::list_repos,
            commands::repos::repo_detail,
            commands::repos::add_repo,
            commands::repos::clone_repo,
            commands::repos::update_repo,
            commands::repos::save_repo_env_files,
            commands::repos::remove_repo,
            commands::repos::set_repo_remote,
            commands::repos::list_branches,
            commands::repos::checkout_branch,
            commands::repos::create_branch,
            commands::repos::delete_branch,
            commands::repos::repo_log,
            commands::repos::commit_graph,
            commands::repos::repo_status,
            commands::repos::file_diff,
            commands::repos::list_dir,
            commands::repos::read_repo_file,
            commands::repos::write_repo_file,
            commands::repos::find_files,
            commands::repos::search_content,
            commands::repos::replace_content,
            commands::repos::commit_changes,
            commands::repos::sensitive_changes,
            commands::repos::reset_hard,
            commands::repos::fetch_repo,
            commands::repos::pull_repo,
            commands::repos::push_repo,
            commands::repos::resolve_rev,
            commands::servers::list_servers,
            commands::servers::save_server,
            commands::servers::delete_server,
            commands::servers::test_server,
            commands::servers::scan_server_security,
            commands::servers::block_server_ip,
            commands::servers::unblock_server_ip,
            commands::servers::kick_server_session,
            commands::servers::enable_server_guard,
            commands::servers::disable_server_guard,
            commands::deploy::start_deploy,
            commands::deploy::redeploy,
            commands::deploy::cancel_deploy,
            commands::deploy::list_releases,
            commands::deploy::rollback_release,
            commands::backup::start_backup,
            commands::backup::list_backups,
            commands::backup::get_backup,
            commands::backup::delete_backup,
            commands::backup::clear_backups,
            commands::backup::test_backup,
            commands::backup::list_backup_targets,
            commands::backup::save_backup_targets,
            commands::backup::list_backup_configs,
            commands::backup::save_backup_config,
            commands::backup::delete_backup_config,
            commands::pages::start_pages_deploy,
            commands::pages::list_pages_records,
            commands::pages::get_pages_record,
            commands::pages::delete_pages_record,
            commands::pages::clear_pages_records,
            commands::pages::get_pages_config,
            commands::pages::save_pages_config,
            commands::pages::test_pages,
            commands::watch::watch_repo,
            commands::watch::unwatch_repo,
            commands::history::list_history,
            commands::history::get_record,
            commands::history::delete_record,
            commands::history::clear_history,
        ])
        .build(tauri::generate_context!())
        .expect("DeployCode 启动失败");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            // 清理完成后的收尾退出（handle.exit(0)）：直接放行。
            if EXIT_CLEANUP_DONE.load(Ordering::SeqCst) {
                return;
            }
            // 终止仍在运行的本地子进程（构建 / wrangler 等），避免残留。
            deploy_core::process::kill_all_children();

            if !EXIT_CLEANUP_STARTED.swap(true, Ordering::SeqCst) {
                let state = app_handle.state::<AppState>();
                let pages_active = state.has_active_pages();
                let deploys = state.take_deploys();
                let backups = state.take_backups();
                if deploys.is_empty() && backups.is_empty() && !pages_active {
                    // 没有进行中的任务：正常退出。
                    return;
                }
                // 有任务仍在进行：推迟退出，先终止远端脚本（有超时兜底）。
                api.prevent_exit();
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    // 先中止本地任务，防止清理期间它再启动新脚本。
                    for (_, deploy) in &deploys {
                        deploy.abort.abort();
                    }
                    for (_, backup) in &backups {
                        backup.abort.abort();
                    }
                    // 留一点时间给已在途的远端脚本写完 pidfile。
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    cleanup_remote_tasks(&handle, deploys, backups).await;
                    // Pages 是阻塞任务、无法 abort：再兜底杀一次清理期间新拉起的子进程。
                    deploy_core::process::kill_all_children();
                    EXIT_CLEANUP_DONE.store(true, Ordering::SeqCst);
                    handle.exit(0);
                });
            } else {
                // 清理仍在进行：再次请求退出也不能打断清理，等清理完成后统一退出。
                api.prevent_exit();
            }
        }
    });
}

/// 逐个重连服务器，终止仍在运行的部署/备份脚本进程组；单项有独立超时，互不挤占预算。
async fn cleanup_remote_tasks(
    app: &tauri::AppHandle,
    deploys: Vec<(String, state::ActiveDeploy)>,
    backups: Vec<(String, state::ActiveBackup)>,
) {
    // 每台服务器/每个任务最多 5s：慢或不可达的服务器不会吃掉其它任务的清理时间。
    const PER_TASK_TIMEOUT: Duration = Duration::from_secs(5);

    let store = app.state::<AppState>().store.clone();
    let config = match store.load_config() {
        Ok(config) => config,
        Err(_) => return,
    };
    let deploy_engine = DeployEngine::new(store.clone());
    let backup_engine = BackupEngine::new(store);

    for (record_id, active) in &deploys {
        let server = match Store::find_server(&config, &active.server_id) {
            Ok(server) => server.clone(),
            Err(_) => continue,
        };
        let _ = tokio::time::timeout(
            PER_TASK_TIMEOUT,
            deploy_engine.cleanup_remote_script(&server, &active.target_dir, record_id, 5),
        )
        .await;
    }
    for (_, active) in &backups {
        let server = match Store::find_server(&config, &active.server_id) {
            Ok(server) => server.clone(),
            Err(_) => continue,
        };
        let _ = tokio::time::timeout(
            PER_TASK_TIMEOUT,
            backup_engine.cleanup_remote_script(&server, &active.record_id),
        )
        .await;
    }
}
