//! Session runtime event handlers.

use crate::runtime::SessionEvent;

use crate::ui::runtime::RuntimeEventRenderContext;

pub(in crate::ui::runtime) fn handle_session(
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
