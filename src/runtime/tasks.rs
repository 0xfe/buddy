//! Runtime task lifecycle helpers.
//!
//! This module isolates prompt-task spawning details so the runtime actor loop
//! can stay focused on command/event orchestration.

use crate::agent::Agent;
use crate::runtime::{RuntimeEventEnvelope, TaskRef};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};
use tracing::Instrument;

/// Runtime-owned metadata for the currently active prompt task.
pub(super) struct ActiveTask {
    /// Runtime task identifier for the currently executing prompt.
    pub(super) task_id: u64,
    /// Task metadata propagated to emitted task-scoped events.
    pub(super) task_ref: TaskRef,
    /// One-shot cancellation signal sender wired into the agent request loop.
    pub(super) cancel_tx: watch::Sender<bool>,
}

/// Completion notification sent from prompt task back to runtime actor.
pub(super) struct TaskDone {
    /// Identifier for the completed task.
    pub(super) task_id: u64,
    /// Task metadata propagated to emitted task-scoped events.
    pub(super) task_ref: TaskRef,
    /// Final task result captured after `Agent::send` returns.
    pub(super) result: Result<String, crate::error::AgentError>,
}

/// Borrowed + owned inputs needed to spawn one prompt task.
pub(super) struct SpawnPromptTask {
    /// Shared runtime-owned agent handle.
    pub(super) agent: Arc<Mutex<Agent>>,
    /// Runtime task id for event correlation.
    pub(super) task_id: u64,
    /// Task metadata shared across emitted events.
    pub(super) task_ref: TaskRef,
    /// User prompt text to send to the agent.
    pub(super) prompt: String,
    /// Structured tracing span for this prompt turn.
    pub(super) turn_span: tracing::Span,
    /// Cancellation receiver watched by the agent loop.
    pub(super) cancel_rx: watch::Receiver<bool>,
    /// Runtime event sink forwarded into the agent.
    pub(super) event_tx: mpsc::UnboundedSender<RuntimeEventEnvelope>,
    /// Completion channel back to runtime actor.
    pub(super) done_tx: mpsc::UnboundedSender<TaskDone>,
}

/// Spawn a background prompt task tied to one runtime task id.
pub(super) fn spawn_prompt_task(args: SpawnPromptTask) {
    let SpawnPromptTask {
        agent,
        task_id,
        task_ref,
        prompt,
        turn_span,
        cancel_rx,
        event_tx,
        done_tx,
    } = args;
    tokio::spawn(
        async move {
            // Configure the shared agent for runtime-stream mode: direct stderr
            // rendering is suppressed and all live updates are routed to events.
            let mut agent = agent.lock().await;
            agent.set_live_output_suppressed(true);
            agent.set_live_output_sink(None);
            agent.set_runtime_event_sink(Some((task_id, event_tx)));
            agent.set_runtime_event_task_context(
                task_ref.session_id.clone(),
                task_ref.correlation_id.clone(),
            );
            agent.set_cancellation_receiver(Some(cancel_rx));
            let result = agent.send(&prompt).await;
            // Always restore baseline settings before releasing the lock so future
            // tasks start from a clean configuration.
            agent.set_cancellation_receiver(None);
            agent.set_runtime_event_sink(None);
            agent.set_runtime_event_task_context(None, None);
            agent.set_live_output_suppressed(false);
            drop(agent);
            let _ = done_tx.send(TaskDone {
                task_id,
                task_ref,
                result,
            });
        }
        .instrument(turn_span),
    );
}
