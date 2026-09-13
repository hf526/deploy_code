use tauri::AppHandle;

/// 把托盘菜单文案同步为前端解析后的界面语言（zh-CN / en-US）。
#[tauri::command]
pub fn set_tray_language(app: AppHandle, language: String) -> Result<(), String> {
    crate::tray::set_language(&app, &language).map_err(|error| error.to_string())
}
