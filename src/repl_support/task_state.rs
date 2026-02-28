//! REPL task/session state and timeout helpers.

use crate::repl_support::tool_payload::truncate_preview;
use std::time::{Duration, Instant};

/// Mutable state for an in-flight background REPL task.
pub(crate) struct BackgroundTask {
    pub id: u64,
    pub kind: String,
    pub details: String,
    pub started_at: Instant,
    pub state: BackgroundTaskState,
    pub timeout_at: Option<Instant>,
    pub final_response: Option<String>,
}

/// Completed task payload carried until UI drains and renders it.
#[derive(Debug, Clone)]
pub(crate) struct CompletedBackgroundTask {
    pub id: u64,
    pub kind: String,
    pub started_at: Instant,
    pub result: Result<String, String>,
}

/// Runtime task state used by liveness/approval rendering.
pub(crate) enum BackgroundTaskState {
    Running,
    WaitingApproval { command: String, since: Instant },
    Cancelling { since: Instant },
}

/// In-flight approval metadata waiting on user input.
pub(crate) struct PendingApproval {
    pub task_id: u64,
    pub approval_id: String,
    pub command: String,
    pub risk: Option<String>,
    pub mutation: Option<bool>,
    pub privesc: Option<bool>,
    pub why: Option<String>,
}

/// Cached runtime context usage displayed by REPL status prompt.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeContextState {
    pub estimated_tokens: u64,
    pub context_limit: u64,
    pub used_percent: f32,
    pub last_prompt_tokens: u64,
    pub last_completion_tokens: u64,
    pub session_total_tokens: u64,
}

impl RuntimeContextState {
    pub fn new(context_limit: Option<u64>) -> Self {
        Self {
            estimated_tokens: 0,
            context_limit: context_limit.unwrap_or(0),
            used_percent: 0.0,
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            session_total_tokens: 0,
        }
    }
}

/// Startup path used for first-session banner copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStartupState {
    ResumedExisting,
    StartedNew,
}

/// `/session resume` selector variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumeRequest {
    SessionId(String),
    Last,
}

/// Parse user durations for timeout/approval slash commands.
pub(crate) fn parse_duration_arg(input: &str) -> Option<Duration> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (digits, unit) = if s.ends_with("ms") {
        (&s[..s.len() - 2], "ms")
    } else if let Some(last) = s.chars().last() {
        if last.is_ascii_alphabetic() {
            (&s[..s.len() - 1], &s[s.len() - 1..])
        } else {
            (s.as_str(), "s")
        }
    } else {
        return None;
    };
    let value = digits.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(value)),
        "s" => Some(Duration::from_secs(value)),
        "m" => value.checked_mul(60).map(Duration::from_secs),
        "h" => value
            .checked_mul(60)
            .and_then(|v| v.checked_mul(60))
            .map(Duration::from_secs),
        "d" => value
            .checked_mul(24)
            .and_then(|v| v.checked_mul(60))
            .and_then(|v| v.checked_mul(60))
            .map(Duration::from_secs),
        _ => None,
    }
}

/// Apply `/timeout` command semantics to current background tasks.
pub(crate) fn apply_task_timeout_command(
    tasks: &mut [BackgroundTask],
    duration_arg: Option<&str>,
    task_id_arg: Option<&str>,
) -> Result<String, String> {
    let Some(duration_arg) = duration_arg else {
        return Err("Usage: /timeout <duration> [task-id]".to_string());
    };
    let duration = parse_duration_arg(duration_arg)
        .ok_or_else(|| "Invalid duration. Examples: 30s, 10m, 1h.".to_string())?;

    let task_id = if let Some(id_arg) = task_id_arg {
        id_arg.parse::<u64>().map_err(|_| {
            "Task id must be a number. Usage: /timeout <duration> [task-id]".to_string()
        })?
    } else if tasks.is_empty() {
        return Err("No running background tasks.".to_string());
    } else if tasks.len() == 1 {
        tasks[0].id
    } else {
        return Err("Task id required when multiple background tasks are running.".to_string());
    };

    let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) else {
        return Err(format!("No running background task with id #{task_id}."));
    };
    task.timeout_at = Some(Instant::now() + duration);
    Ok(format!(
        "task #{task_id} timeout set to {}",
        format_elapsed(duration)
    ))
}

/// Return true if at least one task crossed its timeout deadline.
pub(crate) fn has_elapsed_timeouts(tasks: &[BackgroundTask]) -> bool {
    let now = Instant::now();
    tasks.iter().any(|task| {
        task.timeout_at.is_some_and(|timeout_at| {
            timeout_at <= now && !matches!(task.state, BackgroundTaskState::Cancelling { .. })
        })
    })
}

/// Human-readable timeout suffix for liveness display.
pub(crate) fn timeout_suffix_for_task(task: &BackgroundTask) -> String {
    let Some(timeout_at) = task.timeout_at else {
        return String::new();
    };
    let now = Instant::now();
    if timeout_at <= now {
        " (timeout now)".to_string()
    } else {
        format!(
            " (timeout in {})",
            format_elapsed_coarse(timeout_at.duration_since(now))
        )
    }
}

/// Fine-grained elapsed formatting used in completion/status messages.
pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    if elapsed.as_secs() >= 60 {
        format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

/// Coarse elapsed formatting used in live task lines.
pub(crate) fn format_elapsed_coarse(elapsed: Duration) -> String {
    if elapsed.as_secs() >= 60 {
        format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{}s", elapsed.as_secs())
    }
}

/// Update task state to waiting-for-approval and capture command preview.
pub(crate) fn mark_task_waiting_for_approval(
    tasks: &mut [BackgroundTask],
    task_id: u64,
    command: &str,
    _risk: Option<String>,
    _mutation: Option<bool>,
    _privesc: Option<bool>,
    _why: Option<String>,
    _approval_id: &str,
) -> bool {
    let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) else {
        return false;
    };
    task.state = BackgroundTaskState::WaitingApproval {
        command: truncate_preview(command, 96),
        since: Instant::now(),
    };
    true
}

/// Mark task as actively running (used after approval transition).
pub(crate) fn mark_task_running(tasks: &mut [BackgroundTask], task_id: u64) {
    if let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) {
        task.state = BackgroundTaskState::Running;
    }
}

/// Check whether a task is currently blocked on approval.
pub(crate) fn task_is_waiting_for_approval(tasks: &[BackgroundTask], task_id: u64) -> bool {
    tasks
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| matches!(task.state, BackgroundTaskState::WaitingApproval { .. }))
}
