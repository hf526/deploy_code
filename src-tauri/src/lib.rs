mod commands;
mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deploy_core::{BackupEngine, DeployEngine, Store};
use state::AppState;
use tauri::{Manager, RunEvent};

/// 退出清理只执行一次。
static EXIT_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let store = Store::default_store()?;
            // 上次异常退出遗留的 Running 记录在此收敛为失败，避免状态永久卡住。
            let _ = store.mark_interrupted();
            app.manage(AppState::new(Arc::new(store)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_data_dir,
            commands::app::get_settings,
            commands::app::save_settings,
            commands::app::reveal_path,
            commands::repos::list_repos,
            commands::repos::repo_detail,
            commands::repos::add_repo,
            commands::repos::update_repo,
            commands::repos::remove_repo,
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
            commands::backup::start_backup,
            commands::backup::list_backups,
            commands::backup::get_backup,
            commands::backup::delete_backup,
            commands::backup::clear_backups,
            commands::backup::test_backup,
            commands::backup::list_backup_targets,
            commands::backup::save_backup_targets,
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
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() {
                // 终止仍在运行的本地子进程（构建 / wrangler 等），避免残留。
                deploy_core::process::kill_all_children();
                if !EXIT_CLEANUP_STARTED.swap(true, Ordering::SeqCst) {
                    let state = app_handle.state::<AppState>();
                    let deploys = state.take_deploys();
                    let backups = state.take_backups();
                    if !deploys.is_empty() || !backups.is_empty() {
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
                            handle.exit(0);
                        });
                    }
                }
            }
        }
    });
}

/// 逐个重连服务器，终止仍在运行的部署/备份脚本进程组；总时长有上限，避免退出卡死。
async fn cleanup_remote_tasks(
    app: &tauri::AppHandle,
    deploys: Vec<(String, state::ActiveDeploy)>,
    backups: Vec<(String, state::ActiveBackup)>,
) {
    let store = app.state::<AppState>().store.clone();
    let config = match store.load_config() {
        Ok(config) => config,
        Err(_) => return,
    };
    let deploy_engine = DeployEngine::new(store.clone());
    let backup_engine = BackupEngine::new(store);

    let cleanup = async {
        for (record_id, active) in &deploys {
            let server = match Store::find_server(&config, &active.server_id) {
                Ok(server) => server.clone(),
                Err(_) => continue,
            };
            let _ = deploy_engine
                .cleanup_remote_script(&server, &active.target_dir, record_id, 5)
                .await;
        }
        for (_, active) in &backups {
            let server = match Store::find_server(&config, &active.server_id) {
                Ok(server) => server.clone(),
                Err(_) => continue,
            };
            let _ = backup_engine
                .cleanup_remote_script(&server, &active.record_id)
                .await;
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(10), cleanup).await;
}
