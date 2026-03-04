//! Shared helpers for duplicated backend tmux workflows.
//!
//! These helpers intentionally keep transport-specific execution (local/ssh/container)
//! separate while consolidating repeated selection, parse, and formatting logic.

use crate::error::ToolError;
use crate::tmux::management::{
    parse_created_pane, parse_created_session, parse_killed_pane, parse_managed_sessions,
};
use crate::tools::execution::types::{
    CapturePaneOptions, CreatedTmuxPane, CreatedTmuxSession, ManagedTmuxSession,
    ResolvedTmuxTarget, SendKeysOptions, TmuxTargetSelector,
};

/// True when a managed-target resolution error should fall back to default shared pane.
pub(super) fn should_fallback_to_default_target(err: &ToolError) -> bool {
    let text = err.to_string();
    text.contains("tmux target not found") || text.contains("failed to parse managed tmux target")
}

/// True when missing-target fallback to default pane is allowed.
///
/// Explicit selector requests should not silently fall back, because the model
/// asked for a specific managed target and needs a clear "not found" error.
pub(super) fn allow_missing_target_fallback(
    selector: &TmuxTargetSelector,
    ensure_default_shared: bool,
) -> bool {
    ensure_default_shared && !selector.is_explicit()
}

/// True when an explicit `tmux_capture_pane` selector should retry on default pane.
///
/// We keep explicit-target failures strict for mutating tools, but pane capture is
/// read-only and can safely recover to the default shared pane when a previously
/// selected managed target has been removed.
pub(super) fn should_retry_capture_with_default(
    selector: &TmuxTargetSelector,
    err: &ToolError,
) -> bool {
    selector.is_explicit() && should_fallback_to_default_target(err)
}

/// Append guidance to a tool error without duplicating display prefixes.
pub(super) fn append_tool_error_context(err: ToolError, context: &str) -> ToolError {
    let base = match err {
        ToolError::ExecutionFailed(message) => message,
        other => other.to_string(),
    };
    ToolError::ExecutionFailed(format!("{base}; {context}"))
}

/// Build a managed tmux selector from `tmux_capture_pane` options.
pub(super) fn selector_from_capture_options(options: &CapturePaneOptions) -> TmuxTargetSelector {
    TmuxTargetSelector {
        target: options.target.clone(),
        session: options.session.clone(),
        pane: options.pane.clone(),
    }
}

/// Build a managed tmux selector from `tmux_send_keys` options.
pub(super) fn selector_from_send_keys_options(options: &SendKeysOptions) -> TmuxTargetSelector {
    TmuxTargetSelector {
        target: options.target.clone(),
        session: options.session.clone(),
        pane: options.pane.clone(),
    }
}

/// Format consistent send-keys success text across backends.
pub(super) fn sent_keys_message(target: &ResolvedTmuxTarget) -> String {
    let mut parts = target.notices.clone();
    parts.push(format!(
        "sent keys to tmux pane {} ({})",
        target.pane_id, target.session
    ));
    parts.join("\n")
}

/// Notice shown when default shared pane/session had to be recovered.
pub(super) fn default_shared_pane_recovered_notice() -> String {
    "The default shared tmux pane was lost and has been recovered (recreated when needed). Previous terminal state may be gone, so rerun the last command if needed. Be careful with commands that exit the shell or kill tmux."
        .to_string()
}

/// Notice shown when a non-default managed target is missing and default is used.
pub(super) fn missing_target_fallback_notice(selector: &TmuxTargetSelector) -> String {
    format!(
        "Requested managed tmux target {} was not found (it was likely killed or closed). Using the default shared pane instead.",
        selector_debug_label(selector)
    )
}

/// Build explicit-target missing guidance without silently retargeting.
pub(super) fn missing_target_error_notice(
    selector: &TmuxTargetSelector,
    default_recovered: bool,
) -> String {
    let mut message = format!(
        "Requested managed tmux target {} was not found. Omit target/session/pane to use the default shared pane, or create a managed pane first with tmux_create_pane.",
        selector_debug_label(selector)
    );
    if default_recovered {
        message.push_str(" The default shared pane was recreated and is now ready.");
    }
    message
}

fn selector_debug_label(selector: &TmuxTargetSelector) -> String {
    let mut fields = Vec::new();
    if let Some(target) = selector.target.as_deref() {
        fields.push(format!("target={target}"));
    }
    if let Some(session) = selector.session.as_deref() {
        fields.push(format!("session={session}"));
    }
    if let Some(pane) = selector.pane.as_deref() {
        fields.push(format!("pane={pane}"));
    }
    if fields.is_empty() {
        "<default>".to_string()
    } else {
        format!("({})", fields.join(", "))
    }
}

/// Parse managed session-create script output with shared error text.
pub(super) fn parse_created_session_output(output: &str) -> Result<CreatedTmuxSession, ToolError> {
    parse_created_session(output)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to parse created tmux session".into()))
}

/// Parse managed pane-create script output with shared error text.
pub(super) fn parse_created_pane_output(output: &str) -> Result<CreatedTmuxPane, ToolError> {
    parse_created_pane(output)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to parse created tmux pane".into()))
}

/// Parse managed session-kill script output with shared error text.
pub(super) fn parse_killed_session_output(output: &str) -> Result<String, ToolError> {
    let killed = output.trim();
    if killed.is_empty() {
        return Err(ToolError::ExecutionFailed(
            "failed to parse killed tmux session".into(),
        ));
    }
    Ok(killed.to_string())
}

/// Parse managed pane-kill script output with shared error text.
pub(super) fn parse_killed_pane_output(output: &str) -> Result<String, ToolError> {
    let (session, pane_id) = parse_killed_pane(output)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to parse killed tmux pane".into()))?;
    Ok(format!("killed pane {pane_id} in session {session}"))
}

/// Parse managed tmux inventory output from lifecycle list scripts.
pub(super) fn parse_managed_sessions_output(
    output: &str,
) -> Result<Vec<ManagedTmuxSession>, ToolError> {
    let parsed = parse_managed_sessions(output);
    if output.trim().is_empty() || !parsed.is_empty() {
        return Ok(parsed);
    }
    Err(ToolError::ExecutionFailed(
        "failed to parse managed tmux sessions".into(),
    ))
}

/// Parse managed-session cleanup count (`<usize>`).
pub(super) fn parse_removed_sessions_output(output: &str) -> Result<usize, ToolError> {
    output.trim().parse::<usize>().map_err(|_| {
        ToolError::ExecutionFailed("failed to parse removed managed tmux sessions".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::execution::types::CapturePaneOptions;

    #[test]
    fn selector_builders_preserve_fields() {
        let capture = CapturePaneOptions {
            target: Some("%1".to_string()),
            session: Some("s".to_string()),
            pane: Some("p".to_string()),
            ..CapturePaneOptions::default()
        };
        let selector = selector_from_capture_options(&capture);
        assert_eq!(selector.target.as_deref(), Some("%1"));
        assert_eq!(selector.session.as_deref(), Some("s"));
        assert_eq!(selector.pane.as_deref(), Some("p"));

        let send = SendKeysOptions {
            target: Some("%2".to_string()),
            session: Some("s2".to_string()),
            pane: Some("p2".to_string()),
            ..SendKeysOptions::default()
        };
        let selector = selector_from_send_keys_options(&send);
        assert_eq!(selector.target.as_deref(), Some("%2"));
        assert_eq!(selector.session.as_deref(), Some("s2"));
        assert_eq!(selector.pane.as_deref(), Some("p2"));
    }

    #[test]
    fn fallback_detection_matches_target_resolution_errors() {
        let err = ToolError::ExecutionFailed(
            "failed to resolve managed tmux target: tmux target not found".to_string(),
        );
        assert!(should_fallback_to_default_target(&err));
        let err = ToolError::ExecutionFailed("different failure".to_string());
        assert!(!should_fallback_to_default_target(&err));
    }

    #[test]
    fn missing_target_fallback_disallowed_for_explicit_selectors() {
        assert!(allow_missing_target_fallback(
            &TmuxTargetSelector {
                target: None,
                session: None,
                pane: None,
            },
            true
        ));
        assert!(!allow_missing_target_fallback(
            &TmuxTargetSelector {
                target: None,
                session: Some("build".to_string()),
                pane: None,
            },
            true
        ));
        assert!(!allow_missing_target_fallback(
            &TmuxTargetSelector {
                target: Some("%7".to_string()),
                session: None,
                pane: None,
            },
            true
        ));
    }

    #[test]
    fn explicit_capture_can_retry_on_missing_target() {
        let selector = TmuxTargetSelector {
            target: None,
            session: Some("build".to_string()),
            pane: Some("worker".to_string()),
        };
        let missing = ToolError::ExecutionFailed(
            "failed to resolve managed tmux target: tmux target not found".to_string(),
        );
        assert!(should_retry_capture_with_default(&selector, &missing));

        let non_explicit = TmuxTargetSelector {
            target: None,
            session: None,
            pane: None,
        };
        assert!(!should_retry_capture_with_default(&non_explicit, &missing));

        let other = ToolError::ExecutionFailed("different failure".to_string());
        assert!(!should_retry_capture_with_default(&selector, &other));
    }

    #[test]
    fn append_tool_error_context_keeps_single_execution_failed_prefix() {
        let err = ToolError::ExecutionFailed("failed to resolve managed tmux target".to_string());
        let rendered = append_tool_error_context(err, "retry with default pane").to_string();
        assert_eq!(
            rendered,
            "execution failed: failed to resolve managed tmux target; retry with default pane"
        );
    }

    #[test]
    fn parse_managed_sessions_output_handles_empty_and_invalid_payloads() {
        // Empty inventory is valid, but malformed tagged lines should be rejected.
        let empty = parse_managed_sessions_output("").expect("empty should parse");
        assert!(empty.is_empty());

        let invalid = parse_managed_sessions_output("S\t\nP\t\t\t");
        assert!(invalid.is_err());
    }

    #[test]
    fn parse_removed_sessions_output_reads_count() {
        // Cleanup parser should accept integer counts and reject non-integers.
        assert_eq!(parse_removed_sessions_output("2").unwrap(), 2);
        assert!(parse_removed_sessions_output("nope").is_err());
    }

    #[test]
    fn sent_keys_message_includes_resolution_notices() {
        let message = sent_keys_message(&ResolvedTmuxTarget {
            session: "buddy-agent-mo".to_string(),
            pane_id: "%3".to_string(),
            pane_title: "shared".to_string(),
            is_default_shared: true,
            notices: vec!["notice line".to_string()],
        });
        assert!(message.contains("notice line"));
        assert!(message.contains("sent keys to tmux pane %3 (buddy-agent-mo)"));
    }

    #[test]
    fn missing_target_fallback_notice_mentions_selector_fields() {
        let notice = missing_target_fallback_notice(&TmuxTargetSelector {
            target: None,
            session: Some("buddy-agent-mo".to_string()),
            pane: Some("build".to_string()),
        });
        assert!(notice.contains("session=buddy-agent-mo"));
        assert!(notice.contains("pane=build"));
    }

    #[test]
    fn missing_target_error_notice_mentions_default_recovery_when_present() {
        let notice = missing_target_error_notice(
            &TmuxTargetSelector {
                target: None,
                session: Some("buddy-agent-mo".to_string()),
                pane: Some("worker".to_string()),
            },
            true,
        );
        assert!(notice.contains("session=buddy-agent-mo"));
        assert!(notice.contains("pane=worker"));
        assert!(notice.contains("recreated"));
    }
}
