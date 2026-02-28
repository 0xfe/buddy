//! CLI binary entry point.

mod app;
mod cli;

use clap::Parser;

#[tokio::main]
async fn main() {
    let args = cli::Args::parse();
    let code = app::run(args).await;
    std::process::exit(code);
}
