//! Minimal alternate frontend consuming Buddy runtime events.
//!
//! Run with:
//!   cargo run --example alternate_frontend -- "list files"

use buddy::agent::Agent;
use buddy::config::load_config;
use buddy::runtime::{
    spawn_runtime_with_agent, PromptMetadata, RuntimeCommand, RuntimeEvent, TaskEvent,
};
use buddy::tools::ToolRegistry;

#[tokio::main]
async fn main() -> Result<(), String> {
    let prompt = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if prompt.trim().is_empty() {
        return Err("usage: cargo run --example alternate_frontend -- \"<prompt>\"".to_string());
    }

    let config = load_config(None).map_err(|err| format!("failed to load config: {err}"))?;
    let tools = ToolRegistry::new();
    let agent = Agent::new(config.clone(), tools);
    let (runtime, mut events) = spawn_runtime_with_agent(agent, config, None, None, None);

    runtime
        .send(RuntimeCommand::SubmitPrompt {
            prompt,
            metadata: PromptMetadata {
                source: Some("alternate-frontend-example".to_string()),
                correlation_id: None,
            },
        })
        .await?;

    while let Some(envelope) = events.recv().await {
        match envelope.event {
            RuntimeEvent::Model(model_event) => {
                println!("model: {model_event:?}");
            }
            RuntimeEvent::Tool(tool_event) => {
                println!("tool: {tool_event:?}");
            }
            RuntimeEvent::Task(TaskEvent::Completed { .. }) => {
                println!("task completed");
                break;
            }
            RuntimeEvent::Task(TaskEvent::Failed { message, .. }) => {
                return Err(format!("task failed: {message}"));
            }
            RuntimeEvent::Error(err) => {
                return Err(format!("runtime error: {}", err.message));
            }
            _ => {}
        }
    }

    runtime.send(RuntimeCommand::Shutdown).await?;
    Ok(())
}
