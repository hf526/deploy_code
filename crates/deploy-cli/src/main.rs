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
/// `Store::reconcile_interrupted` 内部会先尝试三种任务锁，确认没有任务在跑才执行。
fn reconcile_interrupted(cli: &Cli) {
    let store = match commands::open_store(cli) {
        Ok(store) => store,
        Err(_) => return,
    };
    let _ = store.reconcile_interrupted();
}
