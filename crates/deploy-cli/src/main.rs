mod cli;
mod commands;
mod output;

use clap::Parser;

use crate::cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    reconcile_interrupted(&cli);
    if let Err(error) = commands::dispatch(&cli).await {
        output::error(error.to_string());
        std::process::exit(1);
    }
}

/// 启动时把上次异常退出遗留的 Running 记录收敛为失败。
/// 只有三种任务锁都能拿到（说明 GUI / 其他进程没有任务在跑）才执行，避免误伤正在进行的任务。
fn reconcile_interrupted(cli: &Cli) {
    let store = match commands::open_store(cli) {
        Ok(store) => store,
        Err(_) => return,
    };
    let locks = (
        store.try_task_lock("deploy"),
        store.try_task_lock("backup"),
        store.try_task_lock("pages"),
    );
    if let (Ok(Some(_deploy)), Ok(Some(_backup)), Ok(Some(_pages))) = locks {
        let _ = store.mark_interrupted();
    }
}
