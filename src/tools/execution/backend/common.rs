//! Shared helpers for duplicated backend tmux workflows.
//!
//! These helpers intentionally keep transport-specific execution (local/ssh/container)
//! separate while consolidating repeated selection, parse, and formatting logic.

use crate::error::ToolError;
use crate::tmux::management::{parse_created_pane, parse_created_session, parse_killed_pane};
use crate::tools::execution::types::{
    CapturePaneOptions, CreatedTmuxPane, CreatedTmuxSession, ResolvedTmuxTarget, SendKeysOptions,
    TmuxTargetSelector,
};

/// True when a managed-target resolution error should fall back to default shared pane.
pub(super) fn should_fallback_to_default_target(err: &ToolError) -> bool {
    let text = err.to_string();
    text.contains("tmux target not found") || text.contains("failed to parse managed tmux target")
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
    format!(
        "sent keys to tmux pane {} ({})",
        target.pane_id, target.session
    )
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
}
