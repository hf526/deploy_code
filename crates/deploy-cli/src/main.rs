mod cli;
mod commands;
mod output;

use clap::Parser;

use crate::cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(error) = commands::dispatch(&cli).await {
        output::error(error.to_string());
        std::process::exit(1);
    }
}
