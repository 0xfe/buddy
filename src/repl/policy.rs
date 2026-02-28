//! Approval policy parsing and runtime conversion helpers.
//!
//! The REPL keeps a local policy representation (`ApprovalPolicy`) that is easy
//! to manipulate from slash commands, then converts it into runtime wire-format
//! values (`RuntimeApprovalPolicy`) before dispatching commands.

use crate::repl::task_state::{format_elapsed, parse_duration_arg};
use crate::runtime::RuntimeApprovalPolicy;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Local REPL approval policy mode.
#[derive(Debug, Clone, Copy)]
pub enum ApprovalPolicy {
    /// Ask the user for every approval request.
    Ask,
    /// Auto-approve every request immediately.
    All,
    /// Auto-deny every request immediately.
    None,
    /// Auto-approve until the instant expires, then fall back to `Ask`.
    Until(Instant),
}

/// Concrete decision value sent to runtime approval responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Accept and unblock the pending action.
    Approve,
    /// Reject and block the pending action.
    Deny,
}

/// Parse `y/n` approval input from the REPL.
pub fn parse_approval_decision(input: &str) -> Option<ApprovalDecision> {
    // Accept common yes/no forms; empty input defaults to "deny" so pressing
    // enter in a prompt is conservative.
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "y" | "yes" => Some(ApprovalDecision::Approve),
        "" | "n" | "no" => Some(ApprovalDecision::Deny),
        _ => None,
    }
}

/// Human-readable label used by `/status`.
pub fn approval_policy_label(policy: ApprovalPolicy) -> String {
    match policy {
        ApprovalPolicy::Ask => "ask".to_string(),
        ApprovalPolicy::All => "all".to_string(),
        ApprovalPolicy::None => "none".to_string(),
        ApprovalPolicy::Until(until) => {
            let now = Instant::now();
            // Expired temporary policy behaves like "ask" from the UI's
            // perspective even before the state is mutated by evaluation.
            if until <= now {
                "ask".to_string()
            } else {
                format!("auto ({})", format_elapsed(until.duration_since(now)))
            }
        }
    }
}

/// Resolve an immediate decision when policy permits auto-approval/denial.
pub fn active_approval_decision(policy: &mut ApprovalPolicy) -> Option<ApprovalDecision> {
    match *policy {
        ApprovalPolicy::Ask => None,
        ApprovalPolicy::All => Some(ApprovalDecision::Approve),
        ApprovalPolicy::None => Some(ApprovalDecision::Deny),
        ApprovalPolicy::Until(until) => {
            // Temporary auto-approve self-resets once it expires so later checks
            // do not keep carrying stale state.
            if until > Instant::now() {
                Some(ApprovalDecision::Approve)
            } else {
                *policy = ApprovalPolicy::Ask;
                None
            }
        }
    }
}

/// Apply `/approve` command update to current policy.
pub fn update_approval_policy(input: &str, policy: &mut ApprovalPolicy) -> Result<String, String> {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Usage: /approve all|ask|none|<duration>".to_string());
    }

    match normalized.as_str() {
        "ask" => {
            *policy = ApprovalPolicy::Ask;
            Ok("approval policy: ask".to_string())
        }
        "all" => {
            *policy = ApprovalPolicy::All;
            Ok("approval policy: all".to_string())
        }
        "none" => {
            *policy = ApprovalPolicy::None;
            Ok("approval policy: none".to_string())
        }
        _ => {
            // Duration mode is interpreted as a temporary auto-approve window.
            let duration = parse_duration_arg(&normalized)
                .ok_or_else(|| "Invalid duration. Examples: 30s, 10m, 1h.".to_string())?;
            *policy = ApprovalPolicy::Until(Instant::now() + duration);
            Ok(format!(
                "approval policy: auto-approve for {}",
                format_elapsed(duration)
            ))
        }
    }
}

/// Convert local policy into runtime wire-format policy.
pub fn to_runtime_approval_policy(policy: ApprovalPolicy) -> RuntimeApprovalPolicy {
    match policy {
        ApprovalPolicy::Ask => RuntimeApprovalPolicy::Ask,
        ApprovalPolicy::All => RuntimeApprovalPolicy::All,
        ApprovalPolicy::None => RuntimeApprovalPolicy::None,
        ApprovalPolicy::Until(until) => {
            // Runtime policy carries an absolute expiration timestamp so actor
            // and frontend can reason about expiry without sharing an `Instant`.
            let remaining = until.saturating_duration_since(Instant::now());
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            RuntimeApprovalPolicy::Until {
                expires_at_unix_ms: now.saturating_add(remaining.as_millis() as u64),
            }
        }
    }
}
