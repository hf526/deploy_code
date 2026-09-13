//! 系统托盘：关闭窗口时隐藏到托盘，只有托盘菜单「退出程序」才真正退出。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

/// 托盘图标 id（切换语言时通过它更新菜单文案）。
pub const TRAY_ID: &str = "main-tray";

/// 托盘文案：显示窗口 / 退出程序 / 悬浮提示。
fn labels(language: &str) -> (&'static str, &'static str, &'static str) {
    if language == "en-US" {
        ("Show Main Window", "Quit", "DeployCode - Branch Deployment Tool")
    } else {
        ("显示主窗口", "退出程序", "DeployCode · 分支部署工具")
    }
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, language: &str) -> tauri::Result<Menu<R>> {
    let (show, quit, _) = labels(language);
    let show_item = MenuItem::with_id(app, "show", show, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", quit, true, None::<&str>)?;
    Menu::with_items(app, &[&show_item, &quit_item])
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 创建托盘图标：左键点按显示主窗口，菜单支持显示窗口 / 退出程序。
pub fn create<R: Runtime>(app: &AppHandle<R>, language: &str) -> tauri::Result<()> {
    let menu = build_menu(app, language)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(labels(language).2)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            // 托盘退出走正常退出流程，会先终止远端脚本并清理本地子进程。
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 前端切换界面语言时同步托盘文案。
pub fn set_language<R: Runtime>(app: &AppHandle<R>, language: &str) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let menu = build_menu(app, language)?;
        tray.set_menu(Some(menu))?;
        let _ = tray.set_tooltip(Some(labels(language).2));
    }
    Ok(())
}
