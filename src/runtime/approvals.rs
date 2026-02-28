//! Approval policy and pending-approval resolution helpers.
//!
//! The runtime actor keeps pending shell approvals in-memory and resolves them
//! either immediately (based on policy) or later when a frontend sends an
//! explicit `RuntimeCommand::Approve`.

use super::tasks::ActiveTask;
use super::{emit_event, truncate_preview, RuntimeActorState};
use crate::runtime::{
    ApprovalDecision, RuntimeApprovalPolicy, RuntimeEvent, RuntimeEventEnvelope, TaskEvent,
    TaskRef, WarningEvent,
};
use crate::tools::shell::ShellApprovalRequest;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Pending approval entry tracked by runtime until resolved.
pub(super) struct PendingRuntimeApproval {
    /// Task that produced this approval request.
    pub(super) task_id: u64,
    /// Broker request handle used to approve/deny the underlying command.
    pub(super) request: ShellApprovalRequest,
}

/// Handle an incoming shell approval request from the tool broker.
pub(super) fn handle_approval_request(
    request: ShellApprovalRequest,
    state: &mut RuntimeActorState,
    active_task: Option<&ActiveTask>,
    pending_approvals: &mut HashMap<String, PendingRuntimeApproval>,
    next_approval_nonce: &mut u64,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    let Some(active_task) = active_task else {
        // Defensive fallback: if runtime lost task context, deny instead of
        // allowing potentially unsafe execution.
        request.deny();
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Warning(WarningEvent {
                task: None,
                message: "approval request arrived without an active task; denied".to_string(),
            }),
        );
        return;
    };

    if let Some(decision) = active_approval_decision(&mut state.approval_policy) {
        // Policy resolved immediately (all/none/until-active), no pending entry.
        resolve_pending_approval(
            PendingRuntimeApproval {
                task_id: active_task.task_id,
                request,
            },
            decision,
            event_tx,
            seq,
        );
        return;
    }

    // Ask-mode path: emit waiting event and store request for later resolution.
    let approval_id = format!("appr-{}-{:04x}", active_task.task_id, *next_approval_nonce);
    *next_approval_nonce = next_approval_nonce.saturating_add(1);
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Task(TaskEvent::WaitingApproval {
            task: TaskRef::from_task_id(active_task.task_id),
            approval_id: approval_id.clone(),
            command: truncate_preview(request.command(), 140),
            risk: request
                .metadata()
                .map(|meta| meta.risk().as_str().to_string()),
            mutation: request.metadata().map(|meta| meta.mutation()),
            privesc: request.metadata().map(|meta| meta.privesc()),
            why: request
                .metadata()
                .map(|meta| truncate_preview(meta.why(), 220)),
        }),
    );
    pending_approvals.insert(
        approval_id,
        PendingRuntimeApproval {
            task_id: active_task.task_id,
            request,
        },
    );
}

/// Compute an immediate approval decision from the active runtime policy.
pub(super) fn active_approval_decision(
    policy: &mut RuntimeApprovalPolicy,
) -> Option<ApprovalDecision> {
    match policy {
        RuntimeApprovalPolicy::Ask => None,
        RuntimeApprovalPolicy::All => Some(ApprovalDecision::Approve),
        RuntimeApprovalPolicy::None => Some(ApprovalDecision::Deny),
        RuntimeApprovalPolicy::Until { expires_at_unix_ms } => {
            // `Until` auto-approves only while the expiration remains in the future.
            if now_unix_millis() < *expires_at_unix_ms {
                Some(ApprovalDecision::Approve)
            } else {
                // Expired windows self-reset to `Ask` so future checks behave predictably.
                *policy = RuntimeApprovalPolicy::Ask;
                None
            }
        }
    }
}

/// Resolve one pending approval and emit a user-visible warning event.
pub(super) fn resolve_pending_approval(
    pending: PendingRuntimeApproval,
    decision: ApprovalDecision,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    let task = TaskRef::from_task_id(pending.task_id);
    match decision {
        ApprovalDecision::Approve => {
            pending.request.approve();
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Warning(WarningEvent {
                    task: Some(task.clone()),
                    message: "approval granted".to_string(),
                }),
            );
        }
        ApprovalDecision::Deny => {
            pending.request.deny();
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Warning(WarningEvent {
                    task: Some(task.clone()),
                    message: "approval denied".to_string(),
                }),
            );
        }
    }
}

/// Deny and remove all pending approvals tied to a task.
pub(super) fn deny_pending_approvals_for_task(
    task_id: u64,
    pending_approvals: &mut HashMap<String, PendingRuntimeApproval>,
) {
    // Collect ids first to avoid mutable iteration + remove conflicts.
    let approval_ids = pending_approvals
        .iter()
        .filter_map(|(id, pending)| (pending.task_id == task_id).then_some(id.clone()))
        .collect::<Vec<_>>();
    for approval_id in approval_ids {
        if let Some(pending) = pending_approvals.remove(&approval_id) {
            pending.request.deny();
        }
    }
}

/// Return the current wall-clock unix time in milliseconds.
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
