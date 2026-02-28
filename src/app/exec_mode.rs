//! One-shot exec mode orchestration.
//!
//! This module keeps `buddy exec` runtime flow out of `app::entry::run` so
//! the top-level entrypoint can focus on setup and dispatch.

use buddy::agent::Agent;
use buddy::config::Config;
use buddy::runtime::{
    spawn_runtime_with_agent, ModelEvent, PromptMetadata, RuntimeCommand, RuntimeEvent, TaskEvent,
};
use buddy::ui::render::RenderSink;

/// Execute a single prompt through the runtime actor and exit.
pub(crate) async fn run_exec_mode(
    renderer: &dyn RenderSink,
    agent: Agent,
    config: Config,
    prompt: String,
) -> i32 {
    let (runtime, mut events) = spawn_runtime_with_agent(agent, config, None, None, None);
    if let Err(err) = runtime
        .send(RuntimeCommand::SubmitPrompt {
            prompt,
            metadata: PromptMetadata {
                source: Some("cli-exec".to_string()),
                correlation_id: None,
            },
        })
        .await
    {
        renderer.error(&format!("failed to submit prompt: {err}"));
        return 1;
    }

    let mut final_response: Option<String> = None;
    let mut failure_message: Option<String> = None;
    while let Some(envelope) = events.recv().await {
        match envelope.event {
            RuntimeEvent::Model(ModelEvent::MessageFinal { content, .. }) => {
                final_response = Some(content);
            }
            RuntimeEvent::Task(TaskEvent::Failed { message, .. }) => {
                failure_message = Some(message);
            }
            RuntimeEvent::Task(TaskEvent::Completed { .. }) => break,
            _ => {}
        }
    }
    let _ = runtime.send(RuntimeCommand::Shutdown).await;

    if let Some(message) = failure_message {
        renderer.error(&message);
        return 1;
    }
    if let Some(response) = final_response {
        renderer.assistant_message(&response);
        return 0;
    }

    renderer.error("runtime finished without a final assistant message");
    1
}
