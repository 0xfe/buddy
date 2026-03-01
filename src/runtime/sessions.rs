//! Runtime session management helpers.
//!
//! These helpers implement session lifecycle commands for the runtime actor:
//! create new session, resume session, compact history, and persist snapshots
//! after task completion.

use super::{emit_event, RuntimeActorState};
use crate::agent::Agent;
use crate::runtime::{RuntimeEvent, RuntimeEventEnvelope, SessionEvent, WarningEvent};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::debug;

/// Create a new session, persisting the current active session first if needed.
pub(super) async fn runtime_session_new(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    debug!(
        active_session = %state.active_session.as_deref().unwrap_or("none"),
        "starting new runtime session flow"
    );
    // Session commands are a no-op without persistence backing.
    let Some(store) = state.session_store.as_ref() else {
        return Err("session store is unavailable".to_string());
    };

    if let Some(active_id) = state.active_session.as_deref() {
        // Persist the current active snapshot before starting a fresh session.
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|e| format!("failed to persist session {active_id}: {e}"))?;
    }

    // Reset the in-memory agent and seed a brand new persisted session id.
    let snapshot = {
        let mut guard = agent.lock().await;
        guard.reset_session();
        guard.snapshot_session()
    };
    let new_id = store
        .create_new_session(&snapshot)
        .map_err(|e| format!("failed to create new session: {e}"))?;
    state.active_session = Some(new_id.clone());
    debug!(session_id = %new_id, "created runtime session");
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Created { session_id: new_id }),
    );
    Ok(())
}

/// Resume a specific persisted session id and make it active.
pub(super) async fn runtime_session_resume(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    session_id: &str,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    debug!(
        target_session = %session_id,
        active_session = %state.active_session.as_deref().unwrap_or("none"),
        "starting runtime session resume flow"
    );
    // Session commands are a no-op without persistence backing.
    let Some(store) = state.session_store.as_ref() else {
        return Err("session store is unavailable".to_string());
    };

    if let Some(active_id) = state.active_session.as_deref() {
        // Persist current active state before swapping to a different session.
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|e| format!("failed to persist session {active_id}: {e}"))?;
    }

    // Load and restore the requested snapshot into the in-memory agent.
    let snapshot = store
        .load(session_id)
        .map_err(|e| format!("failed to load session {session_id}: {e}"))?;
    {
        let mut guard = agent.lock().await;
        guard.restore_session(snapshot.clone());
    }
    // Save immediately so it becomes "last active" and has a refreshed mtime.
    store
        .save(session_id, &snapshot)
        .map_err(|e| format!("failed to refresh session {session_id}: {e}"))?;
    state.active_session = Some(session_id.to_string());
    debug!(session_id = %session_id, "resumed runtime session");
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Resumed {
            session_id: session_id.to_string(),
        }),
    );
    Ok(())
}

/// Compact the active session history and emit summary events.
pub(super) async fn runtime_session_compact(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    debug!(
        active_session = %state.active_session.as_deref().unwrap_or("default"),
        "starting runtime session compaction"
    );
    // Compaction is performed by the agent so token accounting stays centralized.
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

    // Persist compacted history when session persistence is available.
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
    debug!(
        session_id = %session_id,
        removed_turns = report.removed_turns,
        removed_messages = report.removed_messages,
        estimated_before = report.estimated_before,
        estimated_after = report.estimated_after,
        "compacted runtime session"
    );
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Compacted {
            session_id: session_id.clone(),
            estimated_before: Some(report.estimated_before),
            estimated_after: Some(report.estimated_after),
            removed_messages: Some(report.removed_messages as u64),
            removed_turns: Some(report.removed_turns as u64),
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

/// Persist the latest in-memory snapshot for the active session (if any).
pub(super) async fn persist_active_session_snapshot(
    agent: &Arc<Mutex<Agent>>,
    state: &RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    // Persistence is best-effort; missing session/store simply means nothing to save.
    let Some(store) = state.session_store.as_ref() else {
        return;
    };
    let Some(active_session) = state.active_session.as_deref() else {
        return;
    };

    let snapshot = agent.lock().await.snapshot_session();
    if store.save(active_session, &snapshot).is_ok() {
        // Emit "saved" only on success to avoid noisy transient errors in UX.
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Session(SessionEvent::Saved {
                session_id: active_session.to_string(),
            }),
        );
    }
}
