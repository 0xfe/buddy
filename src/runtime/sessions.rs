//! Runtime session management helpers.

use super::{emit_event, RuntimeActorState};
use crate::agent::Agent;
use crate::runtime::{RuntimeEvent, RuntimeEventEnvelope, SessionEvent, WarningEvent};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub(super) async fn runtime_session_new(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    let Some(store) = state.session_store.as_ref() else {
        return Err("session store is unavailable".to_string());
    };

    if let Some(active_id) = state.active_session.as_deref() {
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|e| format!("failed to persist session {active_id}: {e}"))?;
    }

    let snapshot = {
        let mut guard = agent.lock().await;
        guard.reset_session();
        guard.snapshot_session()
    };
    let new_id = store
        .create_new_session(&snapshot)
        .map_err(|e| format!("failed to create new session: {e}"))?;
    state.active_session = Some(new_id.clone());
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Created { session_id: new_id }),
    );
    Ok(())
}

pub(super) async fn runtime_session_resume(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    session_id: &str,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    let Some(store) = state.session_store.as_ref() else {
        return Err("session store is unavailable".to_string());
    };

    if let Some(active_id) = state.active_session.as_deref() {
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|e| format!("failed to persist session {active_id}: {e}"))?;
    }

    let snapshot = store
        .load(session_id)
        .map_err(|e| format!("failed to load session {session_id}: {e}"))?;
    {
        let mut guard = agent.lock().await;
        guard.restore_session(snapshot.clone());
    }
    store
        .save(session_id, &snapshot)
        .map_err(|e| format!("failed to refresh session {session_id}: {e}"))?;
    state.active_session = Some(session_id.to_string());
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Resumed {
            session_id: session_id.to_string(),
        }),
    );
    Ok(())
}

pub(super) async fn runtime_session_compact(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    let report = {
        let mut guard = agent.lock().await;
        guard.compact_history()
    };

    let Some(report) = report else {
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Warning(WarningEvent {
                task: None,
                message: "nothing to compact; history is already focused on recent turns"
                    .to_string(),
            }),
        );
        return Ok(());
    };

    if let (Some(store), Some(active_id)) = (
        state.session_store.as_ref(),
        state.active_session.as_deref(),
    ) {
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|err| format!("failed to persist compacted session {active_id}: {err}"))?;
    }

    let session_id = state
        .active_session
        .clone()
        .unwrap_or_else(|| "default".to_string());
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Compacted {
            session_id: session_id.clone(),
        }),
    );
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Warning(WarningEvent {
            task: None,
            message: format!(
                "compacted session {session_id}: removed {} turn(s), {} message(s) (estimated {} -> {})",
                report.removed_turns,
                report.removed_messages,
                report.estimated_before,
                report.estimated_after
            ),
        }),
    );

    Ok(())
}

pub(super) async fn persist_active_session_snapshot(
    agent: &Arc<Mutex<Agent>>,
    state: &RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    let Some(store) = state.session_store.as_ref() else {
        return;
    };
    let Some(active_session) = state.active_session.as_deref() else {
        return;
    };

    let snapshot = agent.lock().await.snapshot_session();
    if store.save(active_session, &snapshot).is_ok() {
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Session(SessionEvent::Saved {
                session_id: active_session.to_string(),
            }),
        );
    }
}
