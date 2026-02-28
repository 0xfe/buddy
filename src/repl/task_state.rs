//! REPL task/session state and timeout helpers.
//!
//! This module is intentionally side-effect free: it owns lightweight parsing
//! and state transition helpers that the top-level REPL loop can call while
//! keeping UI orchestration code separate.

use crate::repl::tool_payload::truncate_preview;
use std::time::{Duration, Instant};

/// Mutable state for an in-flight background REPL task.
pub struct BackgroundTask {
    /// Runtime-assigned task identifier.
    pub id: u64,
    /// Human-facing task category (for example `prompt`).
    pub kind: String,
    /// Short task summary shown in status lines.
    pub details: String,
    /// Wall-clock instant used to compute elapsed duration in the UI.
    pub started_at: Instant,
    /// Current task lifecycle state.
    pub state: BackgroundTaskState,
    /// Optional cancellation deadline configured via `/timeout`.
    pub timeout_at: Option<Instant>,
    /// Final assistant response retained until consumed by the REPL renderer.
    pub final_response: Option<String>,
}

/// Completed task payload carried until UI drains and renders it.
#[derive(Debug, Clone)]
pub struct CompletedBackgroundTask {
    /// Identifier of the task that finished.
    pub id: u64,
    /// Task kind copied from the active entry at completion time.
    pub kind: String,
    /// Original task start time used for elapsed formatting.
    pub started_at: Instant,
    /// Final result payload (assistant response or failure message).
    pub result: Result<String, String>,
}

/// Runtime task state used by liveness/approval rendering.
pub enum BackgroundTaskState {
    /// Task is actively running.
    Running,
    /// Task is blocked on an approval decision.
    WaitingApproval {
        /// Command preview displayed while waiting.
        command: String,
        /// Instant when the task entered waiting state.
        since: Instant,
    },
    /// Cancellation has been requested and completion is pending.
    Cancelling {
        /// Instant when cancellation was requested.
        since: Instant,
    },
}

/// In-flight approval metadata waiting on user input.
pub struct PendingApproval {
    /// Task that owns this approval prompt.
    pub task_id: u64,
    /// Runtime approval id used for approve/deny command routing.
    pub approval_id: String,
    /// Command preview displayed to the user.
    pub command: String,
    /// Optional risk classification from policy metadata.
    pub risk: Option<String>,
    /// Optional mutation flag from policy metadata.
    pub mutation: Option<bool>,
    /// Optional privilege-escalation flag from policy metadata.
    pub privesc: Option<bool>,
    /// Optional human-readable rationale for why approval is required.
    pub why: Option<String>,
}

/// Cached runtime context usage displayed by REPL status prompt.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeContextState {
    /// Current estimated context usage in tokens.
    pub estimated_tokens: u64,
    /// Model context window limit in tokens.
    pub context_limit: u64,
    /// Convenience percentage (`estimated_tokens / context_limit * 100`).
    pub used_percent: f32,
    /// Prompt token usage reported for the most recent model request.
    pub last_prompt_tokens: u64,
    /// Completion token usage reported for the most recent model request.
    pub last_completion_tokens: u64,
    /// Rolling prompt+completion total for the session.
    pub session_total_tokens: u64,
}

impl RuntimeContextState {
    /// Create an empty context snapshot for startup before first metrics event.
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
pub enum SessionStartupState {
    /// Startup resumed an existing persisted session.
    ResumedExisting,
    /// Startup created a new session from defaults.
    StartedNew,
}

/// `/session resume` selector variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeRequest {
    /// Resume a specific session id.
    SessionId(String),
    /// Resume whatever session the store marks as "last active".
    Last,
}

/// Parse user durations for timeout/approval slash commands.
pub fn parse_duration_arg(input: &str) -> Option<Duration> {
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
    // Parse into u64 so we can use checked arithmetic and avoid silent overflow
    // for large values (especially day/hour conversions).
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
pub fn apply_task_timeout_command(
    tasks: &mut [BackgroundTask],
    duration_arg: Option<&str>,
    task_id_arg: Option<&str>,
) -> Result<String, String> {
    // Keep command UX strict and explicit so callers can display actionable
    // error text directly without additional interpretation.
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
        // Convenience path: single-task sessions can omit task id.
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
pub fn has_elapsed_timeouts(tasks: &[BackgroundTask]) -> bool {
    let now = Instant::now();
    tasks.iter().any(|task| {
        // Tasks already transitioning to cancellation should not retrigger
        // timeout handling every loop iteration.
        task.timeout_at.is_some_and(|timeout_at| {
            timeout_at <= now && !matches!(task.state, BackgroundTaskState::Cancelling { .. })
        })
    })
}

/// Human-readable timeout suffix for liveness display.
pub fn timeout_suffix_for_task(task: &BackgroundTask) -> String {
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
pub fn format_elapsed(elapsed: Duration) -> String {
    // Keep sub-minute values at one decimal place for responsiveness, while
    // minute-scale values stay compact and stable.
    if elapsed.as_secs() >= 60 {
        format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

/// Coarse elapsed formatting used in live task lines.
pub fn format_elapsed_coarse(elapsed: Duration) -> String {
    // Live rows refresh frequently, so integer seconds avoid distracting jitter.
    if elapsed.as_secs() >= 60 {
        format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{}s", elapsed.as_secs())
    }
}

/// Update task state to waiting-for-approval and capture command preview.
pub fn mark_task_waiting_for_approval(
    tasks: &mut [BackgroundTask],
    task_id: u64,
    command: &str,
    _risk: Option<String>,
    _mutation: Option<bool>,
    _privesc: Option<bool>,
    _why: Option<String>,
) -> bool {
    let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) else {
        return false;
    };
    // Store a clipped preview so long commands do not blow up line wrapping.
    task.state = BackgroundTaskState::WaitingApproval {
        command: truncate_preview(command, 96),
        since: Instant::now(),
    };
    true
}

/// Mark task as actively running (used after approval transition).
pub fn mark_task_running(tasks: &mut [BackgroundTask], task_id: u64) {
    if let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) {
        task.state = BackgroundTaskState::Running;
    }
}

/// Check whether a task is currently blocked on approval.
pub fn task_is_waiting_for_approval(tasks: &[BackgroundTask], task_id: u64) -> bool {
    tasks
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| matches!(task.state, BackgroundTaskState::WaitingApproval { .. }))
}
