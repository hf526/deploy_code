use deploy_core::models::{BackupConfig, BackupTarget, ImportPreview, Settings};
use deploy_core::{CoreError, Result};
use tauri::State;

use crate::state::AppState;

#[tauri::command(async)]
pub fn export_config(state: State<AppState>) -> Result<String> {
    state.store.export_config()
}

/// 只读比对：不落盘，供确认框展示新增 / 覆盖 / 保留本机凭据的条数。
#[tauri::command(async)]
pub fn preview_config_import(state: State<AppState>, json_str: String) -> Result<ImportPreview> {
    state.store.preview_import(&json_str)
}

#[tauri::command(async)]
pub fn import_config(state: State<AppState>, json_str: String) -> Result<ImportPreview> {
    state.store.import_config(&json_str)
}

#[cfg(target_os = "windows")]
const AUTOSTART_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// 同上，但不含 HKCU 前缀（注册表 API 直接传子键）。
#[cfg(target_os = "windows")]
const AUTOSTART_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const AUTOSTART_VALUE: &str = "DeployCode";

#[tauri::command]
pub fn get_data_dir(state: State<AppState>) -> String {
    state.store.base_dir().display().to_string()
}

#[tauri::command(async)]
pub fn get_settings(state: State<AppState>) -> Result<Settings> {
    Ok(state.store.load_config()?.settings)
}

/// 引用的备份配置 / 备份目标被删除后，设置里残留的悬空引用一律清空，
/// 避免旧快照（CLI 删除目标 / 多页面并发保存）把已删除的 id 写回，导致备份解析失败。
fn sanitize_settings(
    settings: &mut Settings,
    configs: &[BackupConfig],
    targets: &[BackupTarget],
) {
    if settings
        .scheduled_backup_config_id
        .as_ref()
        .is_some_and(|id| !configs.iter().any(|item| item.id == *id))
    {
        settings.scheduled_backup_config_id = None;
    }
    if settings
        .default_backup_target_id
        .as_ref()
        .is_some_and(|id| !targets.iter().any(|item| item.id == *id))
    {
        settings.default_backup_target_id = None;
    }
}

#[tauri::command(async)]
pub fn save_settings(state: State<AppState>, mut settings: Settings) -> Result<Settings> {
    state.store.mutate_config(|config| {
        sanitize_settings(
            &mut settings,
            &config.backup_configs,
            &config.backup_targets,
        );
        config.settings = settings.clone();
        Ok(())
    })?;
    Ok(settings)
}

/// 查询开机自启动当前值（Windows：Run 注册表项内容；None 表示未设置）。
///
/// 直接用注册表 API 读取 UTF-16：reg.exe 输出到管道时使用 ANSI 代码页，
/// 中文等非 ASCII 路径经 from_utf8_lossy 会变成乱码，导致自启动状态误判。
#[cfg(target_os = "windows")]
fn autostart_value() -> Result<Option<String>> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let subkey: Vec<u16> = AUTOSTART_SUBKEY
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value: Vec<u16> = AUTOSTART_VALUE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // 第一次调用只取数据大小（含结尾 NUL）。
    let mut size: u32 = 0;
    let mut status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(CoreError::Process(format!(
            "读取开机自启动注册表失败（错误码 {status}）"
        )));
    }

    let mut buffer = vec![0u16; (size as usize).div_ceil(2).max(1)];
    status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(CoreError::Process(format!(
            "读取开机自启动注册表失败（错误码 {status}）"
        )));
    }
    while buffer.last() == Some(&0) {
        buffer.pop();
    }
    Ok(Some(String::from_utf16_lossy(&buffer)))
}

/// 比较注册表中的启动路径与当前程序路径（Windows 大小写不敏感）。
#[cfg(target_os = "windows")]
fn same_exe_path(stored: &str, current: &std::path::Path) -> bool {
    let normalize = |value: &str| {
        value
            .trim()
            .trim_matches('"')
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    !stored.trim().is_empty() && normalize(stored) == normalize(&current.display().to_string())
}

/// 查询是否已开启开机自启动。
///
/// 只有启动项存在且指向当前程序才算开启：程序换目录（绿色版移动 / 重装）后显示为未开启，
/// 重新勾选即可写入新路径。
#[tauri::command(async)]
pub fn get_autostart() -> Result<bool> {
    #[cfg(target_os = "windows")]
    {
        let Some(value) = autostart_value()? else {
            return Ok(false);
        };
        let exe = std::env::current_exe()
            .map_err(|e| CoreError::Process(format!("无法获取程序路径: {e}")))?;
        Ok(same_exe_path(&value, &exe))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(false)
    }
}

/// 开启 / 关闭开机自启动（Windows：写入当前用户 Run 注册表项，无需管理员权限）。
#[tauri::command(async)]
pub fn set_autostart(enabled: bool) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        if enabled {
            let exe = std::env::current_exe()
                .map_err(|e| CoreError::Process(format!("无法获取程序路径: {e}")))?;
            let args = vec![
                "add".to_string(),
                AUTOSTART_KEY.to_string(),
                "/v".to_string(),
                AUTOSTART_VALUE.to_string(),
                "/t".to_string(),
                "REG_SZ".to_string(),
                "/d".to_string(),
                format!("\"{}\"", exe.display()),
                "/f".to_string(),
            ];
            let output = deploy_core::process::run("reg", &args, None)?;
            if output.code != 0 {
                return Err(CoreError::Process(format!(
                    "写入开机自启动失败（退出码 {}）：{}",
                    output.code,
                    output.combined()
                )));
            }
            return Ok(());
        }

        // 关闭：先确认存在，避免把「值不存在」返回的非 0 误判为失败；存在则要求删除成功。
        if autostart_value()?.is_none() {
            return Ok(());
        }
        let args = vec![
            "delete".to_string(),
            AUTOSTART_KEY.to_string(),
            "/v".to_string(),
            AUTOSTART_VALUE.to_string(),
            "/f".to_string(),
        ];
        let output = deploy_core::process::run("reg", &args, None)?;
        if output.code != 0 {
            return Err(CoreError::Process(format!(
                "关闭开机自启动失败（退出码 {}）：{}",
                output.code,
                output.combined()
            )));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        Err(CoreError::Process("开机自启动目前仅支持 Windows".to_string()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::models::DbBackupSource;

    fn backup_config(id: &str) -> BackupConfig {
        let mut config = BackupConfig::new(
            "config".to_string(),
            "server".to_string(),
            DbBackupSource::default(),
        );
        config.id = id.to_string();
        config
    }

    #[test]
    fn sanitize_settings_clears_dangling_scheduled_config() {
        let mut settings = Settings::default();
        settings.scheduled_backup_config_id = Some("gone".to_string());
        sanitize_settings(&mut settings, &[], &[]);
        assert_eq!(settings.scheduled_backup_config_id, None);
    }

    #[test]
    fn sanitize_settings_keeps_existing_scheduled_config() {
        let mut settings = Settings::default();
        settings.scheduled_backup_config_id = Some("keep".to_string());
        sanitize_settings(&mut settings, &[backup_config("keep")], &[]);
        assert_eq!(settings.scheduled_backup_config_id.as_deref(), Some("keep"));
    }

    #[test]
    fn sanitize_settings_clears_dangling_default_target() {
        let mut settings = Settings::default();
        settings.default_backup_target_id = Some("gone".to_string());
        sanitize_settings(&mut settings, &[], &[]);
        assert_eq!(settings.default_backup_target_id, None);

        let mut settings = Settings::default();
        settings.default_backup_target_id = Some("t1".to_string());
        let mut target = BackupTarget::new("t1".to_string(), "postgresql://u@h/db".to_string());
        target.id = "t1".to_string();
        sanitize_settings(&mut settings, &[], &[target]);
        assert_eq!(settings.default_backup_target_id.as_deref(), Some("t1"));
    }
}
