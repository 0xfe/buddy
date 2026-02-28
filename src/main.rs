//! CLI binary entry point.

/// Binary-local application orchestration modules.
mod app;
/// CLI argument parsing definitions.
mod cli;

use clap::Parser;

/// Parse CLI arguments, run the app entrypoint, and exit with its status code.
#[tokio::main]
async fn main() {
    let args = cli::Args::parse();
    let code = app::run(args).await;
    std::process::exit(code);
}
