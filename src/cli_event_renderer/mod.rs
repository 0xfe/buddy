//! CLI runtime-event renderer adapter.
//!
//! This module translates typed runtime events into terminal rendering updates.
//! Keeping the mapping here lets alternate frontends reuse the same runtime
//! stream without depending on `main.rs` orchestration details.

mod handlers;

use buddy::config::Config;
use buddy::render::RenderSink;
use buddy::runtime::{RuntimeEvent, RuntimeEventEnvelope};

use crate::repl_support::{
    BackgroundTask, CompletedBackgroundTask, PendingApproval, RuntimeContextState,
};

/// Mutable render-time state mirrored from the interactive loop.
pub(crate) struct RuntimeEventRenderContext<'a> {
    pub renderer: &'a dyn RenderSink,
    pub background_tasks: &'a mut Vec<BackgroundTask>,
    pub completed_tasks: &'a mut Vec<CompletedBackgroundTask>,
    pub pending_approval: &'a mut Option<PendingApproval>,
    pub config: &'a mut Config,
    pub active_session: &'a mut String,
    pub runtime_context: &'a mut RuntimeContextState,
}

/// Consume queued runtime events and update render/runtime state.
pub(crate) fn process_runtime_events(
    events: &mut Vec<RuntimeEventEnvelope>,
    ctx: &mut RuntimeEventRenderContext<'_>,
) {
    for envelope in events.drain(..) {
        match envelope.event {
            RuntimeEvent::Lifecycle(_) => {}
            RuntimeEvent::Warning(event) => handlers::warning::handle_warning(ctx, event),
            RuntimeEvent::Error(event) => handlers::warning::handle_error(ctx, event),
            RuntimeEvent::Session(event) => handlers::session::handle_session(ctx, event),
            RuntimeEvent::Task(event) => handlers::task::handle_task(ctx, event),
            RuntimeEvent::Model(event) => handlers::model::handle_model(ctx, event),
            RuntimeEvent::Tool(event) => handlers::tool::handle_tool(ctx, event),
            RuntimeEvent::Metrics(event) => handlers::metrics::handle_metrics(ctx, event),
        }
    }
}
