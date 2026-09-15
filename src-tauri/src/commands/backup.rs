use deploy_core::models::{new_id, BackupConfig, BackupEvent, BackupRecord, BackupRequest, BackupTarget};
use deploy_core::{BackupEngine, CoreError, PreparedBackup, Result, Store};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{ActiveBackup, AppState, BackupTaskGuard, ClaimGuard, ClaimKind};

/// 在后台执行备份，并把日志/进度/结果通过事件推送给前端。
fn spawn_backup(
    app: AppHandle,
    engine: BackupEngine,
    prepared: PreparedBackup,
    claim: ClaimGuard,
) -> String {
    let record_id = prepared.record.id.clone();
    let server_id = prepared.record.server_id.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<BackupEvent>();

    let emit_app = app.clone();
    let guard_app = app.clone();
    let run_record_id = record_id.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        // 守卫移入事件转发任务：等 engine.run 返回、事件全部转发完再释放抢占标记，
        // 避免紧接着启动的新任务收到上一轮残留的 finished 事件。
        let _guard = BackupTaskGuard::new(guard_app, run_record_id, claim);
        while let Some(event) = receiver.recv().await {
            let _ = emit_app.emit("backup://event", &event);
        }
    });

    let handle = tokio::spawn(async move {
        // 等登记完成后再开始，避免备份瞬间结束时残留登记项、阻塞后续备份。
        let _ = start_rx.await;
        engine
            .run(
                prepared.record,
                prepared.source,
                prepared.target,
                Some(sender),
            )
            .await;
    });

    app.state::<AppState>().track_backup(
        &record_id,
        ActiveBackup {
            server_id,
            record_id: record_id.clone(),
            abort: handle.abort_handle(),
        },
    );
    let _ = start_tx.send(());

    record_id
}

#[tauri::command(async)]
pub fn start_backup(app: AppHandle, state: State<AppState>, request: BackupRequest) -> Result<String> {
    // 先抢占名额（进程内原子标记 + 跨进程文件锁）再 prepare，避免并发命令同时通过检查。
    let claim = match ClaimGuard::acquire(&app, ClaimKind::Backup)? {
        Some(claim) => claim,
        None => return Err(CoreError::busy("已有备份正在进行，请等待完成后再试")),
    };
    let engine = BackupEngine::new(state.store.clone());
    let prepared = engine.prepare(&request)?;
    Ok(spawn_backup(app, engine, prepared, claim))
}

#[tauri::command(async)]
pub fn list_backups(state: State<AppState>, server_id: Option<String>) -> Result<Vec<BackupRecord>> {
    let mut records = state.store.load_backups()?;
    records.reverse();
    if let Some(server_id) = server_id.filter(|value| !value.is_empty()) {
        records.retain(|record| record.server_id == server_id);
    }
    Ok(records)
}

#[tauri::command(async)]
pub fn get_backup(state: State<AppState>, backup_id: String) -> Result<BackupRecord> {
    state.store.find_backup(&backup_id)
}

#[tauri::command(async)]
pub fn delete_backup(state: State<AppState>, backup_id: String) -> Result<bool> {
    state.store.remove_backup(&backup_id)
}

#[tauri::command(async)]
pub fn clear_backups(state: State<AppState>) -> Result<()> {
    state.store.clear_backups()
}

/// 检查指定服务器的备份环境（pg_dump 版本与 Supabase 连通性）。
#[tauri::command]
pub async fn test_backup(
    state: State<'_, AppState>,
    request: BackupRequest,
) -> Result<String> {
    BackupEngine::new(state.store.clone()).test(&request).await
}

#[tauri::command(async)]
pub fn list_backup_targets(state: State<AppState>) -> Result<Vec<BackupTarget>> {
    Ok(state.store.load_config()?.backup_targets)
}

/// 列出保存的备份配置。
#[tauri::command(async)]
pub fn list_backup_configs(state: State<AppState>) -> Result<Vec<BackupConfig>> {
    Ok(state.store.load_config()?.backup_configs)
}

/// 新建或更新一条备份配置。
#[tauri::command(async)]
pub fn save_backup_config(
    state: State<AppState>,
    mut config: BackupConfig,
) -> Result<BackupConfig> {
    config.name = config.name.trim().to_string();
    if config.name.is_empty() {
        return Err(CoreError::config("备份配置名称不能为空"));
    }
    if config.source.database.trim().is_empty() {
        return Err(CoreError::config("数据库名不能为空"));
    }

    let saved = state.store.mutate_config(|app| {
        // 服务器必须以 id 形式存在；名称 / host 也允许（兼容 CLI）。
        let server = Store::find_server(app, &config.server_id)?.clone();
        config.server_id = server.id.clone();

        // 悬空的目标 id 一律按未绑定处理，避免执行时找不到目标。
        if let Some(target_id) = config.target_id.clone() {
            if !app.backup_targets.iter().any(|item| item.id == target_id) {
                config.target_id = None;
            }
        }
        if config
            .supabase_url
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            config.supabase_url = None;
        }
        if config.id.trim().is_empty() {
            config.id = new_id();
        }
        if app
            .backup_configs
            .iter()
            .any(|item| item.id != config.id && item.name == config.name)
        {
            return Err(CoreError::config(format!(
                "备份配置名称已存在: {}",
                config.name
            )));
        }
        match app.backup_configs.iter_mut().find(|item| item.id == config.id) {
            Some(existing) => *existing = config.clone(),
            None => app.backup_configs.push(config.clone()),
        }
        Ok(config.clone())
    })?;
    Ok(saved)
}

/// 删除一条备份配置（不影响已有备份记录）。
#[tauri::command(async)]
pub fn delete_backup_config(state: State<AppState>, config_id: String) -> Result<bool> {
    state.store.mutate_config(|app| {
        let removed: Vec<String> = app
            .backup_configs
            .iter()
            .filter(|item| item.id == config_id || item.name == config_id)
            .map(|item| item.id.clone())
            .collect();
        if removed.is_empty() {
            return Ok(false);
        }
        app.backup_configs.retain(|item| !removed.contains(&item.id));
        // 被删除的配置若正被定时备份使用，清空引用，避免每天到点报错。
        if app
            .settings
            .scheduled_backup_config_id
            .as_ref()
            .is_some_and(|id| removed.contains(id))
        {
            app.settings.scheduled_backup_config_id = None;
        }
        Ok(true)
    })
}

/// 整体保存备份目标列表（前端维护列表与增删）。
#[tauri::command(async)]
pub fn save_backup_targets(
    state: State<AppState>,
    targets: Vec<BackupTarget>,
) -> Result<Vec<BackupTarget>> {
    let mut normalized = Vec::with_capacity(targets.len());
    for mut target in targets {
        target.name = target.name.trim().to_string();
        target.url = target.url.trim().to_string();
        if target.name.is_empty() {
            return Err(CoreError::config("备份目标名称不能为空"));
        }
        if !target.url.starts_with("postgres://") && !target.url.starts_with("postgresql://") {
            return Err(CoreError::config(format!(
                "备份目标「{}」的连接串必须以 postgres:// 或 postgresql:// 开头",
                target.name
            )));
        }
        if target.id.trim().is_empty() {
            target.id = deploy_core::models::new_id();
        }
        normalized.push(target);
    }

    let mut names: Vec<&str> = normalized.iter().map(|target| target.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CoreError::config("备份目标名称不能重复"));
    }

    let mut ids: Vec<&str> = normalized.iter().map(|target| target.id.as_str()).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CoreError::config("备份目标 ID 不能重复"));
    }

    state.store.mutate_config(|config| {
        // 清理指向已删除目标的默认设置，避免解析时找不到目标。
        if let Some(default_id) = config.settings.default_backup_target_id.clone() {
            if !normalized.iter().any(|target| target.id == default_id) {
                config.settings.default_backup_target_id = None;
            }
        }
        for server in config.servers.iter_mut() {
            if let Some(target_id) = server.backup_target_id.clone() {
                if !normalized.iter().any(|target| target.id == target_id) {
                    server.backup_target_id = None;
                }
            }
        }
        for saved in config.backup_configs.iter_mut() {
            if let Some(target_id) = saved.target_id.clone() {
                if !normalized.iter().any(|target| target.id == target_id) {
                    saved.target_id = None;
                }
            }
        }
        config.backup_targets = normalized.clone();
        Ok(())
    })?;
    Ok(normalized)
}
