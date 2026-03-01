//! Build-script metadata injection for CLI/version surfaces.
//!
//! We intentionally keep this dependency-free and resilient: when git/date
//! tooling is unavailable, we fall back to stable "unknown" markers.

use std::env;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    emit_head_ref_watch();
    println!("cargo:rerun-if-env-changed=BUDDY_BUILD_GIT_HASH");
    println!("cargo:rerun-if-env-changed=BUDDY_BUILD_TIMESTAMP");

    let git_hash = env::var("BUDDY_BUILD_GIT_HASH").unwrap_or_else(|_| git_short_hash());
    let build_timestamp =
        env::var("BUDDY_BUILD_TIMESTAMP").unwrap_or_else(|_| build_timestamp_utc());

    println!("cargo:rustc-env=BUDDY_BUILD_GIT_HASH={git_hash}");
    println!("cargo:rustc-env=BUDDY_BUILD_TIMESTAMP={build_timestamp}");
}

fn emit_head_ref_watch() {
    // Track the current branch ref so commit-hash changes trigger rebuilds.
    let Ok(head) = fs::read_to_string(".git/HEAD") else {
        return;
    };
    let trimmed = head.trim();
    let Some(reference) = trimmed.strip_prefix("ref: ") else {
        return;
    };
    println!("cargo:rerun-if-changed=.git/{reference}");
}

fn git_short_hash() -> String {
    run_cmd("git", &["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".to_string())
}

fn build_timestamp_utc() -> String {
    if let Some(timestamp) = run_cmd("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]) {
        return timestamp;
    }
    let fallback = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0);
    format!("unix:{fallback}")
}

fn run_cmd(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
