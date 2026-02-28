//! Shared slash-command dispatch helpers for the interactive REPL.
//!
//! The main loop has two command-entry points (normal prompt input and approval
//! prompt input). This module centralizes behavior shared by both paths so
//! command semantics remain consistent and easier to test.

use crate::app::approval::send_approval_decision;
use crate::app::tasks::{kill_background_task, render_background_tasks};
use buddy::repl::{
    active_approval_decision, apply_task_timeout_command, mark_task_running,
    task_is_waiting_for_approval, to_runtime_approval_policy, update_approval_policy,
    ApprovalPolicy, BackgroundTask, PendingApproval,
};
use buddy::ui::render::RenderSink;
use buddy::runtime::BuddyRuntimeHandle;
use buddy::runtime::RuntimeCommand;
use buddy::ui::terminal as term_ui;

/// Invocation mode for shared slash-command dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SharedSlashDispatchMode {
    /// Dispatch from the normal REPL prompt.
    Repl,
    /// Dispatch while waiting on a specific approval request.
    Approval { task_id: u64 },
}

/// Result of handling a shared slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SharedSlashDispatchOutcome {
    /// Command was not a shared command; caller should handle it elsewhere.
    Unhandled,
    /// Shared command was handled; no special approval follow-up required.
    Handled,
    /// Shared command was handled and approval prompt should be re-shown.
    RequeueApproval,
    /// Shared command resolved the approval (approved/denied), so no requeue.
    ApprovalResolved,
}

/// Handle slash commands shared between normal REPL and approval REPL modes.
pub(crate) async fn dispatch_shared_slash_action(
    renderer: &dyn RenderSink,
    action: &term_ui::SlashCommandAction,
    runtime: &BuddyRuntimeHandle,
    background_tasks: &mut Vec<BackgroundTask>,
    pending_approval: &mut Option<PendingApproval>,
    active_approval: Option<&PendingApproval>,
    approval_policy: &mut ApprovalPolicy,
    mode: SharedSlashDispatchMode,
) -> SharedSlashDispatchOutcome {
    let mut resolved_approval = false;
    let handled = match action {
        term_ui::SlashCommandAction::Ps => {
            render_background_tasks(renderer, background_tasks);
            true
        }
        term_ui::SlashCommandAction::Kill(id_arg) => {
            let Some(id_arg) = id_arg.as_deref() else {
                renderer.warn("Usage: /kill <task-id>");
                return outcome_for_mode(mode, false, background_tasks);
            };
            let Ok(task_id) = id_arg.parse::<u64>() else {
                renderer.warn("Task id must be a number. Usage: /kill <task-id>");
                return outcome_for_mode(mode, false, background_tasks);
            };
            kill_background_task(
                renderer,
                runtime,
                background_tasks,
                pending_approval,
                task_id,
            )
            .await;
            true
        }
        term_ui::SlashCommandAction::Timeout { duration, task_id } => {
            match apply_task_timeout_command(
                background_tasks,
                duration.as_deref(),
                task_id.as_deref(),
            ) {
                Ok(msg) => {
                    renderer.section(&msg);
                    eprintln!();
                }
                Err(msg) => renderer.warn(&msg),
            }
            true
        }
        term_ui::SlashCommandAction::Approve(mode_arg) => {
            let mode_arg = mode_arg.as_deref().unwrap_or("");
            match update_approval_policy(mode_arg, approval_policy) {
                Ok(msg) => {
                    renderer.section(&msg);
                    eprintln!();
                    if let Err(err) = runtime
                        .send(RuntimeCommand::SetApprovalPolicy {
                            policy: to_runtime_approval_policy(*approval_policy),
                        })
                        .await
                    {
                        renderer.warn(&format!("failed to update runtime approval policy: {err}"));
                    }
                }
                Err(msg) => {
                    renderer.warn(&msg);
                    return outcome_for_mode(mode, false, background_tasks);
                }
            }

            if let SharedSlashDispatchMode::Approval { task_id } = mode {
                if let Some(decision) = active_approval_decision(approval_policy) {
                    if let Some(approval) = active_approval {
                        if approval.task_id == task_id {
                            if let Err(err) =
                                send_approval_decision(runtime, approval, decision).await
                            {
                                renderer.warn(&err);
                            } else {
                                mark_task_running(background_tasks, task_id);
                                resolved_approval = true;
                            }
                        }
                    }
                }
            }

            true
        }
        _ => false,
    };

    if !handled {
        return SharedSlashDispatchOutcome::Unhandled;
    }
    if resolved_approval {
        return SharedSlashDispatchOutcome::ApprovalResolved;
    }
    outcome_for_mode(mode, true, background_tasks)
}

fn outcome_for_mode(
    mode: SharedSlashDispatchMode,
    handled: bool,
    background_tasks: &[BackgroundTask],
) -> SharedSlashDispatchOutcome {
    if !handled {
        return SharedSlashDispatchOutcome::Unhandled;
    }
    match mode {
        SharedSlashDispatchMode::Repl => SharedSlashDispatchOutcome::Handled,
        SharedSlashDispatchMode::Approval { task_id } => {
            if task_is_waiting_for_approval(background_tasks, task_id) {
                SharedSlashDispatchOutcome::RequeueApproval
            } else {
                SharedSlashDispatchOutcome::Handled
            }
        }
    }
}
