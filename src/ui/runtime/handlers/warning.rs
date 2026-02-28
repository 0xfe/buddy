//! Warning/error runtime event handlers.

use crate::runtime::{ErrorEvent, WarningEvent};

use crate::ui::runtime::RuntimeEventRenderContext;

/// Render a warning event unless it is known transient approval chatter.
pub(in crate::ui::runtime) fn handle_warning(
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

/// Render an error event with task context when present.
pub(in crate::ui::runtime) fn handle_error(ctx: &RuntimeEventRenderContext<'_>, event: ErrorEvent) {
    if let Some(task) = event.task {
        ctx.renderer
            .error(&format!("[task #{}] {}", task.task_id, event.message));
    } else {
        ctx.renderer.error(&event.message);
    }
}

/// Filter warnings that are expected during approval handshakes.
fn is_transient_approval_warning(message: &str) -> bool {
    matches!(
        message.trim().to_ascii_lowercase().as_str(),
        "approval granted" | "approval denied"
    )
}
