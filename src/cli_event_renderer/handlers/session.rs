//! Session runtime event handlers.

use buddy::runtime::SessionEvent;

use crate::cli_event_renderer::RuntimeEventRenderContext;

pub(in crate::cli_event_renderer) fn handle_session(
    ctx: &mut RuntimeEventRenderContext<'_>,
    event: SessionEvent,
) {
    match event {
        SessionEvent::Created { session_id } => {
            *ctx.active_session = session_id.clone();
            ctx.renderer
                .section(&format!("created session: {session_id}"));
            eprintln!();
        }
        SessionEvent::Resumed { session_id } => {
            *ctx.active_session = session_id.clone();
            ctx.renderer
                .section(&format!("resumed session: {session_id}"));
            eprintln!();
        }
        SessionEvent::Compacted { session_id } => {
            ctx.renderer
                .section(&format!("compacted session: {session_id}"));
            eprintln!();
        }
        SessionEvent::Saved { .. } => {}
    }
}
