//! Model runtime event handlers.

use crate::config::select_model_profile;
use crate::runtime::ModelEvent;

use crate::ui::runtime::RuntimeEventRenderContext;

/// Apply one model event to render output and reducer state.
pub(in crate::ui::runtime) fn handle_model(
    ctx: &mut RuntimeEventRenderContext<'_>,
    event: ModelEvent,
) {
    match event {
        ModelEvent::ReasoningDelta { task, field, delta } => {
            ctx.renderer
                .reasoning_trace(&format!("task #{} {field}", task.task_id), &delta);
        }
        ModelEvent::MessageFinal { task, content } => {
            if let Some(bg) = ctx
                .background_tasks
                .iter_mut()
                .find(|bg| bg.id == task.task_id)
            {
                bg.final_response = Some(content);
            }
        }
        ModelEvent::ProfileSwitched {
            profile,
            model: _,
            base_url: _,
            api: _,
            auth: _,
            reasoning_effort: _,
        } => {
            if let Err(err) = select_model_profile(ctx.config, &profile) {
                ctx.renderer.warn(&format!(
                    "runtime switched model profile `{profile}`, but local config sync failed: {err}"
                ));
            }
        }
        ModelEvent::RequestStarted { .. }
        | ModelEvent::RequestSummary { .. }
        | ModelEvent::TextDelta { .. }
        | ModelEvent::ResponseSummary { .. } => {}
    }
}
