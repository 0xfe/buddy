//! Task runtime event handlers.

use buddy::ui::render::set_progress_enabled;
use buddy::runtime::TaskEvent;

use crate::cli_event_renderer::RuntimeEventRenderContext;
use crate::repl_support::{
    mark_task_running, mark_task_waiting_for_approval, BackgroundTask, BackgroundTaskState,
    CompletedBackgroundTask, PendingApproval,
};

pub(in crate::cli_event_renderer) fn handle_task(
    ctx: &mut RuntimeEventRenderContext<'_>,
    event: TaskEvent,
) {
    match event {
        TaskEvent::Queued {
            task,
            kind,
            details,
        } => {
            ctx.background_tasks.push(BackgroundTask {
                id: task.task_id,
                kind,
                details,
                started_at: std::time::Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            });
            set_progress_enabled(false);
        }
        TaskEvent::Started { task } => {
            mark_task_running(ctx.background_tasks, task.task_id);
        }
        TaskEvent::WaitingApproval {
            task,
            approval_id,
            command,
            risk,
            mutation,
            privesc,
            why,
        } => {
            if mark_task_waiting_for_approval(
                ctx.background_tasks,
                task.task_id,
                &command,
                risk.clone(),
                mutation,
                privesc,
                why.clone(),
                &approval_id,
            ) && ctx.pending_approval.is_none()
            {
                *ctx.pending_approval = Some(PendingApproval {
                    task_id: task.task_id,
                    approval_id,
                    command,
                    risk,
                    mutation,
                    privesc,
                    why,
                });
            }
        }
        TaskEvent::Cancelling { task } => {
            if let Some(bg) = ctx
                .background_tasks
                .iter_mut()
                .find(|bg| bg.id == task.task_id)
            {
                bg.state = BackgroundTaskState::Cancelling {
                    since: std::time::Instant::now(),
                };
            }
        }
        TaskEvent::Completed { task } => {
            if ctx
                .pending_approval
                .as_ref()
                .is_some_and(|approval| approval.task_id == task.task_id)
            {
                *ctx.pending_approval = None;
            }
            if let Some(index) = ctx
                .background_tasks
                .iter()
                .position(|bg| bg.id == task.task_id)
            {
                let task = ctx.background_tasks.swap_remove(index);
                ctx.completed_tasks.push(CompletedBackgroundTask {
                    id: task.id,
                    kind: task.kind,
                    started_at: task.started_at,
                    result: Ok(task.final_response.unwrap_or_default()),
                });
            }
        }
        TaskEvent::Failed { task, message } => {
            if ctx
                .pending_approval
                .as_ref()
                .is_some_and(|approval| approval.task_id == task.task_id)
            {
                *ctx.pending_approval = None;
            }
            if let Some(index) = ctx
                .background_tasks
                .iter()
                .position(|bg| bg.id == task.task_id)
            {
                let task = ctx.background_tasks.swap_remove(index);
                ctx.completed_tasks.push(CompletedBackgroundTask {
                    id: task.id,
                    kind: task.kind,
                    started_at: task.started_at,
                    result: Err(message),
                });
            }
        }
    }
}
