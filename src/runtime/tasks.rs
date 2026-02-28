//! Runtime task lifecycle helpers.

use crate::agent::Agent;
use crate::runtime::RuntimeEventEnvelope;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};

pub(super) struct ActiveTask {
    pub(super) task_id: u64,
    pub(super) cancel_tx: watch::Sender<bool>,
}

pub(super) struct TaskDone {
    pub(super) task_id: u64,
    pub(super) result: Result<String, crate::error::AgentError>,
}

pub(super) fn spawn_prompt_task(
    agent: Arc<Mutex<Agent>>,
    task_id: u64,
    prompt: String,
    cancel_rx: watch::Receiver<bool>,
    event_tx: mpsc::UnboundedSender<RuntimeEventEnvelope>,
    done_tx: mpsc::UnboundedSender<TaskDone>,
) {
    tokio::spawn(async move {
        let mut agent = agent.lock().await;
        agent.set_live_output_suppressed(true);
        agent.set_live_output_sink(None);
        agent.set_runtime_event_sink(Some((task_id, event_tx)));
        agent.set_cancellation_receiver(Some(cancel_rx));
        let result = agent.send(&prompt).await;
        agent.set_cancellation_receiver(None);
        agent.set_runtime_event_sink(None);
        agent.set_live_output_suppressed(false);
        drop(agent);
        let _ = done_tx.send(TaskDone { task_id, result });
    });
}
