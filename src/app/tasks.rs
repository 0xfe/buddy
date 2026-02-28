//! Background runtime task state helpers for the REPL loop.

use crate::app::approval::send_approval_decision;
use buddy::repl::{
    format_elapsed, format_elapsed_coarse, timeout_suffix_for_task, ApprovalDecision,
    BackgroundTask, BackgroundTaskState, CompletedBackgroundTask, PendingApproval,
    RuntimeContextState,
};
use buddy::config::Config;
use buddy::ui::render::RenderSink;
use buddy::ui::runtime;
use buddy::runtime::{BuddyRuntimeHandle, RuntimeCommand, RuntimeEventEnvelope};
use buddy::ui::terminal::settings;
use std::time::Instant;
use tokio::sync::mpsc;

/// Drain available runtime events without blocking.
pub(crate) fn collect_runtime_events(
    rx: &mut mpsc::UnboundedReceiver<RuntimeEventEnvelope>,
    out: &mut Vec<RuntimeEventEnvelope>,
) -> bool {
    let start_len = out.len();
    while let Ok(envelope) = rx.try_recv() {
        out.push(envelope);
    }
    out.len() > start_len
}

/// Render runtime events into task/session/context state mutations.
pub(crate) fn process_runtime_events(
    renderer: &dyn RenderSink,
    events: &mut Vec<RuntimeEventEnvelope>,
    background_tasks: &mut Vec<BackgroundTask>,
    completed_tasks: &mut Vec<CompletedBackgroundTask>,
    pending_approval: &mut Option<PendingApproval>,
    config: &mut Config,
    active_session: &mut String,
    runtime_context: &mut RuntimeContextState,
) {
    let mut context = runtime::RuntimeEventRenderContext {
        renderer,
        background_tasks,
        completed_tasks,
        pending_approval,
        config,
        active_session,
        runtime_context,
    };
    runtime::process_runtime_events(events, &mut context);
}

/// Emit completion/failure output for completed background tasks.
pub(crate) fn drain_completed_tasks(
    renderer: &dyn RenderSink,
    completed: &mut Vec<CompletedBackgroundTask>,
) -> bool {
    if completed.is_empty() {
        return false;
    }

    for task in completed.drain(..) {
        let elapsed = format_elapsed(task.started_at.elapsed());
        match task.result {
            Ok(response) => {
                renderer.activity(&format!("prompt #{} processed in {elapsed}", task.id));
                renderer.assistant_message(&response);
            }
            Err(message) => {
                renderer.error(&format!(
                    "Background task #{} ({}) failed after {elapsed}: {}",
                    task.id, task.kind, message
                ));
            }
        }
    }
    true
}

/// Build inline liveness line for the active background task set.
pub(crate) fn background_liveness_line(tasks: &[BackgroundTask]) -> Option<String> {
    if tasks.is_empty() {
        return None;
    }

    let spinner = background_liveness_spinner(tasks);

    if tasks.len() == 1 {
        let task = &tasks[0];
        let timeout_suffix = timeout_suffix_for_task(task);
        let body = match &task.state {
            BackgroundTaskState::Running => format!(
                "task #{} running {}{}",
                task.id,
                format_elapsed_coarse(task.started_at.elapsed()),
                timeout_suffix
            ),
            BackgroundTaskState::WaitingApproval { since, .. } => format!(
                "task #{} waiting approval {}{}",
                task.id,
                format_elapsed_coarse(since.elapsed()),
                timeout_suffix
            ),
            BackgroundTaskState::Cancelling { since } => format!(
                "task #{} cancelling {}{}",
                task.id,
                format_elapsed_coarse(since.elapsed()),
                timeout_suffix
            ),
        };
        return Some(format!("[{spinner}] {body}"));
    }

    let running = tasks
        .iter()
        .filter(|task| matches!(task.state, BackgroundTaskState::Running))
        .count();
    let waiting = tasks
        .iter()
        .filter(|task| matches!(task.state, BackgroundTaskState::WaitingApproval { .. }))
        .count();
    let cancelling = tasks
        .iter()
        .filter(|task| matches!(task.state, BackgroundTaskState::Cancelling { .. }))
        .count();
    let with_timeout = tasks
        .iter()
        .filter(|task| task.timeout_at.is_some())
        .count();
    let oldest = tasks
        .iter()
        .map(|task| task.started_at.elapsed())
        .max()
        .unwrap_or_default();
    Some(format!(
        "[{spinner}] {} tasks: {} running, {} waiting approval, {} cancelling, {} with timeout, oldest {}",
        tasks.len(),
        running,
        waiting,
        cancelling,
        with_timeout,
        format_elapsed_coarse(oldest)
    ))
}

/// Resolve spinner frame for inline liveness line.
pub(crate) fn background_liveness_spinner(tasks: &[BackgroundTask]) -> char {
    let elapsed = tasks
        .iter()
        .map(|task| match &task.state {
            BackgroundTaskState::Running => task.started_at.elapsed(),
            BackgroundTaskState::WaitingApproval { since, .. } => since.elapsed(),
            BackgroundTaskState::Cancelling { since } => since.elapsed(),
        })
        .max()
        .unwrap_or_default();
    let idx = ((elapsed.as_millis() / 250) as usize) % settings::PROGRESS_FRAMES.len();
    settings::PROGRESS_FRAMES[idx]
}

/// Request runtime cancellation for a running task.
pub(crate) async fn request_task_cancellation(
    runtime: &BuddyRuntimeHandle,
    tasks: &mut [BackgroundTask],
    pending_approval: &mut Option<PendingApproval>,
    task_id: u64,
) -> bool {
    let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) else {
        return false;
    };

    if pending_approval
        .as_ref()
        .is_some_and(|approval| approval.task_id == task_id)
    {
        if let Some(approval) = pending_approval.take() {
            let _ = send_approval_decision(runtime, &approval, ApprovalDecision::Deny).await;
        }
    }

    if let Err(err) = runtime.send(RuntimeCommand::CancelTask { task_id }).await {
        return {
            let _ = err;
            false
        };
    }
    task.state = BackgroundTaskState::Cancelling {
        since: Instant::now(),
    };
    true
}

/// User-facing `/kill` helper for background tasks.
pub(crate) async fn kill_background_task(
    renderer: &dyn RenderSink,
    runtime: &BuddyRuntimeHandle,
    tasks: &mut [BackgroundTask],
    pending_approval: &mut Option<PendingApproval>,
    task_id: u64,
) {
    if request_task_cancellation(runtime, tasks, pending_approval, task_id).await {
        renderer.warn(&format!("Cancelling task #{task_id}..."));
    } else {
        renderer.warn(&format!("No running background task with id #{task_id}."));
    }
}

/// Cancel tasks whose timeout deadline has elapsed.
pub(crate) async fn enforce_task_timeouts(
    renderer: &dyn RenderSink,
    runtime: &BuddyRuntimeHandle,
    tasks: &mut [BackgroundTask],
    pending_approval: &mut Option<PendingApproval>,
) {
    let now = Instant::now();
    let expired_ids = tasks
        .iter()
        .filter_map(|task| {
            let timeout_at = task.timeout_at?;
            if timeout_at <= now && !matches!(task.state, BackgroundTaskState::Cancelling { .. }) {
                Some(task.id)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for task_id in expired_ids {
        if request_task_cancellation(runtime, tasks, pending_approval, task_id).await {
            renderer.warn(&format!("Task #{task_id} hit timeout; cancelling."));
        }
    }
}

/// Render `/ps` output.
pub(crate) fn render_background_tasks(renderer: &dyn RenderSink, tasks: &[BackgroundTask]) {
    renderer.section("background tasks");
    if tasks.is_empty() {
        renderer.field("running", "none");
        eprintln!();
        return;
    }

    for task in tasks {
        let state = match &task.state {
            BackgroundTaskState::Running => {
                format!("running ({})", format_elapsed(task.started_at.elapsed()))
            }
            BackgroundTaskState::WaitingApproval { command, since, .. } => {
                format!(
                    "waiting approval {} for: {}",
                    format_elapsed(since.elapsed()),
                    command
                )
            }
            BackgroundTaskState::Cancelling { since } => {
                format!("cancelling ({})", format_elapsed(since.elapsed()))
            }
        };
        let timeout_note = timeout_suffix_for_task(task);
        renderer.field(
            &format!("#{}", task.id),
            &format!(
                "{} \"{}\" [{}{}]",
                task.kind, task.details, state, timeout_note
            ),
        );
    }
    eprintln!();
}
