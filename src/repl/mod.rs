//! Shared REPL/task helper state and utilities.
//!
//! This module keeps high-churn orchestration data structures and small parsing
//! helpers out of `main.rs`, exposed as a reusable facade for CLI/runtime code.
//! The submodules are intentionally focused:
//! - `policy` manages approval policy parsing/labels.
//! - `task_state` tracks background task lifecycle and timeout utilities.
//! - `tool_payload` normalizes tool output payloads for display.

pub mod policy;
pub mod task_state;
pub mod tool_payload;

/// Re-export approval policy helpers for command handling in the REPL loop.
pub use policy::{
    active_approval_decision, approval_policy_label, parse_approval_decision,
    to_runtime_approval_policy, update_approval_policy, ApprovalDecision, ApprovalPolicy,
};
/// Re-export timeout parsing utility used by slash-command handlers.
pub use task_state::parse_duration_arg;
/// Re-export background task state model and task utility helpers.
pub use task_state::{
    apply_task_timeout_command, format_elapsed, format_elapsed_coarse, has_elapsed_timeouts,
    mark_task_running, mark_task_waiting_for_approval, task_is_waiting_for_approval,
    timeout_suffix_for_task, BackgroundTask, BackgroundTaskState, CompletedBackgroundTask,
    PendingApproval, ResumeRequest, RuntimeContextState, SessionStartupState,
};
/// Re-export tool payload parsing and preview formatting helpers.
pub use tool_payload::{
    parse_shell_tool_result, parse_tool_arg, quote_preview, tool_result_display_text,
    truncate_preview,
};
