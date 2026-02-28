//! Runtime task lifecycle helpers.
//!
//! This module isolates prompt-task spawning details so the runtime actor loop
//! can stay focused on command/event orchestration.

use crate::agent::Agent;
use crate::runtime::RuntimeEventEnvelope;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};

/// Runtime-owned metadata for the currently active prompt task.
pub(super) struct ActiveTask {
    /// Runtime task identifier for the currently executing prompt.
    pub(super) task_id: u64,
    /// One-shot cancellation signal sender wired into the agent request loop.
    pub(super) cancel_tx: watch::Sender<bool>,
}

/// Completion notification sent from prompt task back to runtime actor.
pub(super) struct TaskDone {
    /// Identifier for the completed task.
    pub(super) task_id: u64,
    /// Final task result captured after `Agent::send` returns.
    pub(super) result: Result<String, crate::error::AgentError>,
}

/// Spawn a background prompt task tied to one runtime task id.
pub(super) fn spawn_prompt_task(
    agent: Arc<Mutex<Agent>>,
    task_id: u64,
    prompt: String,
    cancel_rx: watch::Receiver<bool>,
    event_tx: mpsc::UnboundedSender<RuntimeEventEnvelope>,
    done_tx: mpsc::UnboundedSender<TaskDone>,
) {
    tokio::spawn(async move {
        // Configure the shared agent for runtime-stream mode: direct stderr
        // rendering is suppressed and all live updates are routed to events.
        let mut agent = agent.lock().await;
        agent.set_live_output_suppressed(true);
        agent.set_live_output_sink(None);
        agent.set_runtime_event_sink(Some((task_id, event_tx)));
        agent.set_cancellation_receiver(Some(cancel_rx));
        let result = agent.send(&prompt).await;
        // Always restore baseline settings before releasing the lock so future
        // tasks start from a clean configuration.
        agent.set_cancellation_receiver(None);
        agent.set_runtime_event_sink(None);
        agent.set_live_output_suppressed(false);
        drop(agent);
        let _ = done_tx.send(TaskDone { task_id, result });
    });
}
