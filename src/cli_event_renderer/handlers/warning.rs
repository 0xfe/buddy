//! Warning/error runtime event handlers.

use buddy::runtime::{ErrorEvent, WarningEvent};

use crate::cli_event_renderer::RuntimeEventRenderContext;

pub(in crate::cli_event_renderer) fn handle_warning(
    ctx: &RuntimeEventRenderContext<'_>,
    event: WarningEvent,
) {
    if is_transient_approval_warning(&event.message) {
        return;
    }
    if let Some(task) = event.task {
        ctx.renderer
            .warn(&format!("[task #{}] {}", task.task_id, event.message));
    } else {
        ctx.renderer.warn(&event.message);
    }
}

pub(in crate::cli_event_renderer) fn handle_error(
    ctx: &RuntimeEventRenderContext<'_>,
    event: ErrorEvent,
) {
    if let Some(task) = event.task {
        ctx.renderer
            .error(&format!("[task #{}] {}", task.task_id, event.message));
    } else {
        ctx.renderer.error(&event.message);
    }
}

fn is_transient_approval_warning(message: &str) -> bool {
    matches!(
        message.trim().to_ascii_lowercase().as_str(),
        "approval granted" | "approval denied"
    )
}
