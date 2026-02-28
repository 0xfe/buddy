//! Metrics runtime event handlers.

use buddy::runtime::MetricsEvent;

use crate::cli_event_renderer::RuntimeEventRenderContext;

pub(in crate::cli_event_renderer) fn handle_metrics(
    ctx: &mut RuntimeEventRenderContext<'_>,
    event: MetricsEvent,
) {
    match event {
        MetricsEvent::TokenUsage {
            task,
            prompt_tokens,
            completion_tokens,
            session_total_tokens,
        } => {
            ctx.runtime_context.last_prompt_tokens = prompt_tokens;
            ctx.runtime_context.last_completion_tokens = completion_tokens;
            ctx.runtime_context.session_total_tokens = session_total_tokens;
            ctx.renderer.section("task");
            ctx.renderer.field(
                "tokens",
                &format!(
                    "#{} prompt:{prompt_tokens} completion:{completion_tokens} session:{session_total_tokens}",
                    task.task_id
                ),
            );
            eprintln!();
        }
        MetricsEvent::ContextUsage {
            estimated_tokens,
            context_limit,
            used_percent,
            ..
        } => {
            ctx.runtime_context.estimated_tokens = estimated_tokens;
            ctx.runtime_context.context_limit = context_limit;
            ctx.runtime_context.used_percent = used_percent;
        }
        MetricsEvent::PhaseDuration { .. } => {}
    }
}
