use deploy_core::models::Settings;
use deploy_core::{CoreError, Result};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn get_data_dir(state: State<AppState>) -> String {
    state.store.base_dir().display().to_string()
}

#[tauri::command(async)]
pub fn get_settings(state: State<AppState>) -> Result<Settings> {
    Ok(state.store.load_config()?.settings)
}

#[tauri::command(async)]
pub fn save_settings(state: State<AppState>, settings: Settings) -> Result<Settings> {
    state.store.mutate_config(|config| {
        config.settings = settings.clone();
        Ok(())
    })?;
    Ok(settings)
}

/// 在系统文件管理器中打开目录。
#[tauri::command(async)]
pub fn reveal_path(path: String) -> Result<()> {
    if path.trim().is_empty() {
        return Err(CoreError::config("路径为空"));
    }

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer")
        .arg(path.replace('/', "\\"))
        .spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&path).spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&path).spawn();

    let child = result.map_err(|e| CoreError::Process(format!("打开目录失败: {e}")))?;
    // 回收子进程，避免 Unix 上 open/xdg-open 变成僵尸进程。
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}
