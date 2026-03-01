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
    if let Err(err) = app::logging::init_logging(&args) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
    let code = app::run(args).await;
    std::process::exit(code);
}
