//! Process-wide logging/tracing subscriber setup.
//!
//! The CLI keeps runtime-event JSONL tracing separate from structured process
//! logs. This module wires `-v/-vv/-vvv` and env-based filters into
//! `tracing-subscriber` for operator diagnostics.

use crate::cli::Args;
use std::env;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Buddy-specific log-filter override.
const BUDDY_LOG_ENV_VAR: &str = "BUDDY_LOG";
/// Standard Rust log-filter override.
const RUST_LOG_ENV_VAR: &str = "RUST_LOG";

/// Initialize structured logging for the current process.
///
/// Precedence:
/// 1. `BUDDY_LOG` filter expression
/// 2. `RUST_LOG` filter expression
/// 3. `-v` verbosity mapping (`-v` info, `-vv` debug, `-vvv` trace)
///
/// When none are set, logging remains disabled to preserve normal REPL UX.
pub(crate) fn init_logging(args: &Args) -> Result<(), String> {
    let Some(filter_spec) = resolve_filter_spec(args.verbose) else {
        return Ok(());
    };

    // Respect existing global subscribers (embedding/tests may set one first).
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }

    let env_filter = EnvFilter::try_new(filter_spec.clone())
        .map_err(|err| format!("invalid log filter `{filter_spec}`: {err}"))?;
    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_ansi(!args.no_color)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .try_init()
        .map_err(|err| format!("failed to initialize logging subscriber: {err}"))?;
    Ok(())
}

/// Resolve the final subscriber filter expression from env + CLI verbosity.
fn resolve_filter_spec(verbose: u8) -> Option<String> {
    resolve_filter_spec_with(
        verbose,
        first_non_empty_env(BUDDY_LOG_ENV_VAR),
        first_non_empty_env(RUST_LOG_ENV_VAR),
    )
}

/// Resolve final filter expression from explicit env-value inputs.
fn resolve_filter_spec_with(
    verbose: u8,
    buddy_log: Option<String>,
    rust_log: Option<String>,
) -> Option<String> {
    first_non_empty_value(buddy_log)
        .or_else(|| first_non_empty_value(rust_log))
        .or_else(|| verbose_filter_spec(verbose))
}

/// Return non-empty environment variable content.
fn first_non_empty_env(key: &str) -> Option<String> {
    first_non_empty_value(env::var(key).ok())
}

/// Trim and discard empty string values.
fn first_non_empty_value(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Convert stackable `-v` count into a conservative filter expression.
fn verbose_filter_spec(verbose: u8) -> Option<String> {
    let level = match verbose {
        0 => return None,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    Some(format!(
        "buddy={level},reqwest=warn,hyper=warn,h2=warn,rustls=warn"
    ))
}

#[cfg(test)]
mod tests {
    use super::{resolve_filter_spec_with, verbose_filter_spec};

    // Verifies default behavior keeps logging disabled when no flags/env are set.
    #[test]
    fn resolve_filter_defaults_to_none() {
        assert!(resolve_filter_spec_with(0, None, None).is_none());
    }

    // Verifies verbosity levels map to expected severity ranges.
    #[test]
    fn verbose_filter_mapping() {
        let info = verbose_filter_spec(1).expect("filter");
        assert!(info.contains("buddy=info"));
        let debug = verbose_filter_spec(2).expect("filter");
        assert!(debug.contains("buddy=debug"));
        let trace = verbose_filter_spec(4).expect("filter");
        assert!(trace.contains("buddy=trace"));
    }

    // Verifies env filter expressions take precedence over CLI verbosity.
    #[test]
    fn env_filter_overrides_verbosity() {
        let filter = resolve_filter_spec_with(
            1,
            Some("buddy::runtime=trace".to_string()),
            Some("buddy=debug".to_string()),
        )
        .expect("filter");
        assert_eq!(filter, "buddy::runtime=trace");
    }
}
