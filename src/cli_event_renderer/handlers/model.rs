//! Model runtime event handlers.

use buddy::config::select_model_profile;
use buddy::runtime::ModelEvent;

use crate::cli_event_renderer::RuntimeEventRenderContext;

pub(in crate::cli_event_renderer) fn handle_model(
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
            model,
            base_url,
            api,
            auth,
        } => {
            if let Err(err) = select_model_profile(ctx.config, &profile) {
                ctx.renderer.warn(&format!(
                    "runtime switched model profile `{profile}`, but local config sync failed: {err}"
                ));
            }
            ctx.renderer
                .section(&format!("switched model profile: {profile}"));
            ctx.renderer.field("model", &model);
            ctx.renderer.field("base_url", &base_url);
            ctx.renderer
                .field("api", &format!("{api:?}").to_ascii_lowercase());
            ctx.renderer
                .field("auth", &format!("{auth:?}").to_ascii_lowercase());
            ctx.renderer.field(
                "context_limit",
                &ctx.config
                    .api
                    .context_limit
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "auto".to_string()),
            );
            eprintln!();
        }
        ModelEvent::RequestStarted { .. } | ModelEvent::TextDelta { .. } => {}
    }
}
