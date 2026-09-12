use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;
use deploy_core::{CoreError, Result, Store};

const EVENT_NAME: &str = "repo://fs-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsChanged {
    repo_id: String,
}

/// 判断事件路径是否应忽略，避免构建 / 装依赖时的事件风暴触发无谓刷新。
/// `.git` 与任意层级的 `node_modules` 一律忽略；`target` / `dist` 只忽略仓库第一层，
/// 防止误伤源码里同名的子目录。
fn ignored_path(path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    for component in rel.components() {
        match component.as_os_str().to_str() {
            Some(".git") | Some("node_modules") => return true,
            _ => {}
        }
    }
    matches!(
        rel.components().next().and_then(|c| c.as_os_str().to_str()),
        Some("target") | Some("dist")
    )
}

/// 监听仓库工作区文件变化（忽略 .git，避免 git 命令自触发回环）。
/// 使用同步命令：保证与 unwatch 的调用顺序一致，避免快速切换仓库时 watcher 泄漏。
#[tauri::command]
pub fn watch_repo(
    state: State<AppState>,
    app: AppHandle,
    repo_id: String,
    path: String,
) -> Result<()> {
    // 只信任配置中登记的仓库路径，防止传入任意目录（如整个磁盘）被递归监听。
    let config = state.store.load_config()?;
    let repo = Store::find_repo(&config, &repo_id)?;
    let root = PathBuf::from(&repo.path);
    if !root.is_dir() {
        return Err(CoreError::not_found(format!("仓库目录不存在: {path}")));
    }

    let mut watchers = state
        .watchers
        .lock()
        .map_err(|_| CoreError::git("监听器状态异常"))?;
    if watchers.contains_key(&repo_id) {
        return Ok(());
    }

    let (tx, rx) = mpsc::channel::<()>();
    let filter_root = root.clone();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if event
                    .paths
                    .iter()
                    .any(|p| !ignored_path(p, &filter_root))
                {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| CoreError::git(format!("启动文件监听失败: {e}")))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| CoreError::git(format!("启动文件监听失败: {e}")))?;

    let id = repo_id.clone();
    // 去抖：等到 600ms 内没有新事件后，向前端发一次通知；通道断开则直接退出，不误发。
    thread::spawn(move || loop {
        if rx.recv().is_err() {
            return;
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(600)) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let _ = app.emit(EVENT_NAME, FsChanged { repo_id: id.clone() });
    });

    watchers.insert(repo_id, watcher);
    Ok(())
}

/// 停止监听（watcher 移除后，去抖线程随通道关闭自动退出）。
#[tauri::command]
pub fn unwatch_repo(state: State<AppState>, repo_id: String) -> Result<()> {
    if let Ok(mut watchers) = state.watchers.lock() {
        watchers.remove(&repo_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_path_skips_git_and_build_dirs() {
        let root = Path::new("/repo");
        assert!(ignored_path(Path::new("/repo/.git/index"), root));
        assert!(ignored_path(
            Path::new("/repo/packages/app/node_modules/x/index.js"),
            root
        ));
        assert!(ignored_path(Path::new("/repo/target/debug/app.exe"), root));
        assert!(ignored_path(Path::new("/repo/dist/index.html"), root));
        // 源码与深层同名目录不受影响。
        assert!(!ignored_path(Path::new("/repo/src/main.rs"), root));
        assert!(!ignored_path(Path::new("/repo/src/dist/tool.rs"), root));
        assert!(!ignored_path(Path::new("/repo/crates/app/target/x"), root));
    }

    #[test]
    fn ignored_path_falls_back_when_not_under_root() {
        let root = Path::new("/repo");
        assert!(ignored_path(Path::new("/other/node_modules/x"), root));
        assert!(!ignored_path(Path::new("/other/src/x.rs"), root));
    }
}
