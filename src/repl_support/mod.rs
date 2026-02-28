//! Shared REPL/task helper state and utilities.
//!
//! This module keeps high-churn orchestration data structures and small parsing
//! helpers out of `main.rs`, while remaining binary-local (`pub(crate)`).

pub(crate) mod policy;
pub(crate) mod task_state;
pub(crate) mod tool_payload;

pub(crate) use policy::{
    active_approval_decision, approval_policy_label, parse_approval_decision,
    to_runtime_approval_policy, update_approval_policy, ApprovalDecision, ApprovalPolicy,
};
pub(crate) use task_state::{
    apply_task_timeout_command, format_elapsed, format_elapsed_coarse, has_elapsed_timeouts,
    mark_task_running, mark_task_waiting_for_approval, task_is_waiting_for_approval,
    timeout_suffix_for_task, BackgroundTask, BackgroundTaskState, CompletedBackgroundTask,
    PendingApproval, ResumeRequest, RuntimeContextState, SessionStartupState,
};
pub(crate) use tool_payload::{
    parse_shell_tool_result, parse_tool_arg, quote_preview, tool_result_display_text,
    truncate_preview,
};
#[cfg(test)]
pub(crate) use task_state::parse_duration_arg;
