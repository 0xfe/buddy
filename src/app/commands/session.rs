//! Session command helpers for `/session` and CLI resume flows.

use crate::cli;
use crate::repl_support::{format_elapsed, ResumeRequest};
use buddy::agent::Agent;
use buddy::ui::render::RenderSink;
use buddy::runtime::{BuddyRuntimeHandle, RuntimeCommand};
use buddy::session::{SessionStore, SessionSummary};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Handle `/session` command behavior.
pub(crate) async fn handle_session_command(
    renderer: &dyn RenderSink,
    session_store: &SessionStore,
    runtime: &BuddyRuntimeHandle,
    active_session: &mut String,
    verb: Option<&str>,
    name: Option<&str>,
) {
    let action = verb.unwrap_or("list").trim().to_ascii_lowercase();
    match action.as_str() {
        "" | "list" => match session_store.list() {
            Ok(sessions) => render_sessions(renderer, active_session, &sessions),
            Err(e) => renderer.warn(&format!("failed to list sessions: {e}")),
        },
        "resume" => {
            let Some(requested_id) = name.map(str::trim).filter(|s| !s.is_empty()) else {
                renderer.warn("Usage: /session resume <session-id|last>");
                return;
            };

            let target_id = if requested_id.eq_ignore_ascii_case("last") {
                match session_store.resolve_last() {
                    Ok(Some(last)) => last,
                    Ok(None) => {
                        renderer.warn("No saved sessions found.");
                        return;
                    }
                    Err(e) => {
                        renderer.warn(&format!("failed to resolve last session: {e}"));
                        return;
                    }
                }
            } else {
                requested_id.to_string()
            };

            if target_id == *active_session {
                renderer.section(&format!("session already active: {target_id}"));
                eprintln!();
                return;
            }

            if let Err(e) = runtime
                .send(RuntimeCommand::SessionResume {
                    session_id: target_id,
                })
                .await
            {
                renderer.warn(&format!("failed to submit session resume command: {e}"));
            }
        }
        "new" | "create" => {
            if name.is_some() {
                renderer.warn("Usage: /session new");
                return;
            }

            if let Err(e) = runtime.send(RuntimeCommand::SessionNew).await {
                renderer.warn(&format!("failed to submit new session command: {e}"));
            }
        }
        _ => {
            renderer
                .warn("Usage: /session [list] | /session resume <session-id|last> | /session new");
        }
    }
}

/// Render compact session summary list.
pub(crate) fn render_sessions(
    renderer: &dyn RenderSink,
    active_session: &str,
    sessions: &[SessionSummary],
) {
    renderer.section("sessions");
    renderer.field("current", active_session);
    if sessions.is_empty() {
        renderer.field("saved", "none");
        eprintln!();
        return;
    }

    for session in sessions {
        let key = if session.id == active_session {
            format!("* {}", session.id)
        } else {
            session.id.clone()
        };
        renderer.field(
            &key,
            &format!(
                "last used {} ago",
                format_elapsed_since_epoch_millis(session.updated_at_millis)
            ),
        );
    }
    eprintln!();
}

/// Convert a stored unix-millis timestamp into a user-facing elapsed duration.
pub(crate) fn format_elapsed_since_epoch_millis(ts: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    if ts >= now {
        return "0.0s".to_string();
    }
    format_elapsed(Duration::from_millis(now - ts))
}

/// Parse CLI `resume` subcommand variants into internal request enum.
pub(crate) fn resume_request_from_command(
    command: Option<&cli::Command>,
) -> Result<Option<ResumeRequest>, String> {
    let Some(command) = command else {
        return Ok(None);
    };

    match command {
        cli::Command::Resume { session_id, last } => {
            if *last {
                if session_id.is_some() {
                    return Err(
                        "Use either `buddy resume <session-id>` or `buddy resume --last`."
                            .to_string(),
                    );
                }
                return Ok(Some(ResumeRequest::Last));
            }
            let Some(session_id) = session_id.as_deref().map(str::trim) else {
                return Err("Usage: buddy resume <session-id> | buddy resume --last".to_string());
            };
            if session_id.is_empty() {
                return Err("session id cannot be empty".to_string());
            }
            Ok(Some(ResumeRequest::SessionId(session_id.to_string())))
        }
        _ => Ok(None),
    }
}

/// Load or create the active session and sync it back to the store.
pub(crate) fn initialize_active_session(
    renderer: &dyn RenderSink,
    session_store: &SessionStore,
    agent: &mut Agent,
    resume_request: Option<ResumeRequest>,
) -> Result<(crate::repl_support::SessionStartupState, String), String> {
    match resume_request {
        None => {
            let snapshot = agent.snapshot_session();
            let session_id = session_store
                .create_new_session(&snapshot)
                .map_err(|e| format!("failed to create new session: {e}"))?;
            Ok((
                crate::repl_support::SessionStartupState::StartedNew,
                session_id,
            ))
        }
        Some(ResumeRequest::Last) => {
            let Some(last_id) = session_store
                .resolve_last()
                .map_err(|e| format!("failed to resolve last session: {e}"))?
            else {
                return Err(
                    "No saved sessions found in this directory. Start `buddy` to create one."
                        .to_string(),
                );
            };
            let snapshot = session_store
                .load(&last_id)
                .map_err(|e| format!("failed to load session {last_id}: {e}"))?;
            agent.restore_session(snapshot.clone());
            if let Err(e) = session_store.save(&last_id, &snapshot) {
                renderer.warn(&format!("failed to refresh session {last_id}: {e}"));
            }
            Ok((
                crate::repl_support::SessionStartupState::ResumedExisting,
                last_id,
            ))
        }
        Some(ResumeRequest::SessionId(session_id)) => {
            let snapshot = session_store
                .load(&session_id)
                .map_err(|e| format!("failed to load session {session_id}: {e}"))?;
            agent.restore_session(snapshot.clone());
            if let Err(e) = session_store.save(&session_id, &snapshot) {
                renderer.warn(&format!("failed to refresh session {session_id}: {e}"));
            }
            Ok((
                crate::repl_support::SessionStartupState::ResumedExisting,
                session_id,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_request_validation_rejects_ambiguous_forms() {
        let err = resume_request_from_command(Some(&cli::Command::Resume {
            session_id: Some("abc".to_string()),
            last: true,
        }))
        .expect_err("must reject");
        assert!(err.contains("either"));
    }
}
