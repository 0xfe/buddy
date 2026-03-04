//! Approval prompt rendering and decision helpers.

use buddy::repl::{mark_task_running, ApprovalDecision, BackgroundTask, PendingApproval};
use buddy::runtime::{
    ApprovalDecision as RuntimeApprovalDecision, BuddyRuntimeHandle, RuntimeCommand,
};
use buddy::tools::execution::TmuxAttachInfo;
use buddy::ui::render::RenderSink;
use buddy::ui::theme::{self, ThemeToken};
use crossterm::style::{Color, Stylize};

/// Default number of command lines shown in approval preview mode.
const APPROVAL_PREVIEW_LINES: usize = 5;

/// True when an approval command has more lines than the collapsed preview.
pub(crate) fn approval_has_expand(command: &str) -> bool {
    command.lines().count() > APPROVAL_PREVIEW_LINES
}

/// Build the target label used in approval prompts.
pub(crate) fn approval_prompt_actor(
    ssh_target: Option<&str>,
    container: Option<&str>,
    tmux_info: Option<&TmuxAttachInfo>,
    requested_tmux_session: Option<&str>,
    requested_tmux_pane: Option<&str>,
) -> String {
    let mut actor = if let Some(target) = ssh_target {
        format!("ssh:{target}")
    } else if let Some(container) = container {
        format!("container:{container}")
    } else {
        "local".to_string()
    };

    let requested_session = requested_tmux_session
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let requested_pane = requested_tmux_pane
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(info) = tmux_info {
        let default_session = info.session.as_str();
        let effective_session = requested_session.unwrap_or(default_session);
        let effective_pane = requested_pane.unwrap_or("shared");
        let non_default = requested_session.is_some_and(|s| s != default_session)
            || requested_pane.is_some_and(|p| p != "shared");
        if non_default {
            actor.push_str(&format!(" (tmux:{effective_session}/{effective_pane})"));
        } else {
            actor.push_str(&format!(" (tmux:{default_session})"));
        }
    } else if requested_session.is_some() || requested_pane.is_some() {
        let session = requested_session.unwrap_or("default");
        let pane = requested_pane.unwrap_or("shared");
        actor.push_str(&format!(" (tmux:{session}/{pane})"));
    }
    actor
}

/// Render the shell approval request block.
pub(crate) fn render_shell_approval_request(
    color: bool,
    renderer: &dyn RenderSink,
    actor: &str,
    command: &str,
    expanded: bool,
    risk: Option<&str>,
    why: Option<&str>,
) {
    // Render summary line first, then optional reason, then the command block itself.
    let (risk_label, risk_color) = approval_risk_style(risk);
    if color {
        eprintln!(
            "{} {} risk shell command on {}",
            "•".with(theme::color(ThemeToken::SectionBullet)),
            risk_label.with(risk_color).bold(),
            actor.with(theme::color(ThemeToken::FieldValue))
        );
    } else {
        eprintln!("• {risk_label} risk shell command on {actor}");
    }
    if let Some(reason) = why.map(str::trim).filter(|value| !value.is_empty()) {
        if color {
            eprintln!("  {}", reason.with(theme::color(ThemeToken::FieldKey)));
        } else {
            eprintln!("  {reason}");
        }
    }
    let (command_text, truncated_lines) = if expanded {
        (command.to_string(), 0)
    } else {
        approval_command_preview(command, APPROVAL_PREVIEW_LINES)
    };
    let mut block = format_approval_command_block(&command_text);
    if truncated_lines > 0 {
        block.push('\n');
        block.push_str(&format!(
            "  ...{truncated_lines} more lines... (press 'e' to expand)"
        ));
    }
    renderer.approval_block(&block);
}

/// Map risk metadata to a display label/style.
pub(crate) fn approval_risk_style(risk: Option<&str>) -> (&'static str, Color) {
    match risk
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("low")
    {
        "high" => ("high", theme::color(ThemeToken::RiskHigh)),
        "medium" => ("medium", theme::color(ThemeToken::RiskMedium)),
        _ => ("low", theme::color(ThemeToken::RiskLow)),
    }
}

/// Format command text as a shell snippet block for the approval renderer.
pub(crate) fn format_approval_command_block(command: &str) -> String {
    if command.trim().is_empty() {
        return "$".to_string();
    }

    let mut out = String::new();
    for (idx, line) in command.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if idx == 0 {
            out.push_str("$ ");
        } else {
            out.push_str("  ");
        }
        out.push_str(line);
    }
    out
}

/// Build a line-limited preview for approval command rendering.
pub(crate) fn approval_command_preview(command: &str, max_lines: usize) -> (String, usize) {
    if max_lines == 0 {
        return (String::new(), command.lines().count());
    }
    let lines = command.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return (command.to_string(), 0);
    }
    let preview = lines
        .iter()
        .take(max_lines)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    (preview, lines.len() - max_lines)
}

/// Convert CLI-side approval decision into runtime command decision.
pub(crate) fn runtime_approval_decision(decision: ApprovalDecision) -> RuntimeApprovalDecision {
    match decision {
        ApprovalDecision::Approve => RuntimeApprovalDecision::Approve,
        ApprovalDecision::Deny => RuntimeApprovalDecision::Deny,
    }
}

/// Send approval decision for a specific approval request id.
pub(crate) async fn send_approval_decision(
    runtime: &BuddyRuntimeHandle,
    approval: &PendingApproval,
    decision: ApprovalDecision,
) -> Result<(), String> {
    runtime
        .send(RuntimeCommand::Approve {
            approval_id: approval.approval_id.clone(),
            decision: runtime_approval_decision(decision),
        })
        .await
        .map_err(|e| format!("failed to send approval decision: {e}"))
}

/// Deny active approval request and restore task state to running.
pub(crate) async fn deny_pending_approval(
    runtime: &BuddyRuntimeHandle,
    tasks: &mut [BackgroundTask],
    approval: PendingApproval,
) {
    mark_task_running(tasks, approval.task_id);
    let _ = send_approval_decision(runtime, &approval, ApprovalDecision::Deny).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::tools::execution::{TmuxAttachInfo, TmuxAttachTarget};

    #[test]
    fn approval_prompt_actor_prefers_ssh_then_container_then_local() {
        // Priority order should prefer explicit remote/container context over local default.
        assert_eq!(
            approval_prompt_actor(Some("dev@host"), Some("box"), None, None, None),
            "ssh:dev@host"
        );
        assert_eq!(
            approval_prompt_actor(None, Some("box"), None, None, None),
            "container:box"
        );
        assert_eq!(approval_prompt_actor(None, None, None, None, None), "local");
    }

    #[test]
    fn approval_prompt_actor_includes_tmux_session_when_available() {
        // tmux context should be appended when approval is scoped to a shared pane.
        let info = TmuxAttachInfo {
            session: "buddy-a1b2".to_string(),
            window: "shared",
            target: TmuxAttachTarget::Local,
        };
        assert_eq!(
            approval_prompt_actor(None, None, Some(&info), None, None),
            "local (tmux:buddy-a1b2)"
        );
    }

    #[test]
    fn approval_prompt_actor_shows_non_default_requested_pane() {
        let info = TmuxAttachInfo {
            session: "buddy-a1b2".to_string(),
            window: "shared",
            target: TmuxAttachTarget::Local,
        };
        assert_eq!(
            approval_prompt_actor(
                None,
                None,
                Some(&info),
                Some("buddy-a1b2-build"),
                Some("builder"),
            ),
            "local (tmux:buddy-a1b2-build/builder)"
        );
    }

    #[test]
    fn approval_command_block_formats_multiline_commands() {
        // Multiline commands should preserve subsequent lines with continuation indent.
        let block = format_approval_command_block("echo 1\necho 2");
        assert_eq!(block, "$ echo 1\n  echo 2");
    }

    #[test]
    fn approval_command_preview_limits_to_configured_lines() {
        // Preview mode should cap output to the first N lines and report remaining lines.
        let (preview, remaining) =
            approval_command_preview("a\nb\nc\nd\ne\nf\ng", APPROVAL_PREVIEW_LINES);
        assert_eq!(preview, "a\nb\nc\nd\ne");
        assert_eq!(remaining, 2);
    }

    #[test]
    fn approval_command_preview_keeps_short_commands_intact() {
        // Commands with <= max preview lines should remain unchanged.
        let (preview, remaining) =
            approval_command_preview("echo 1\necho 2", APPROVAL_PREVIEW_LINES);
        assert_eq!(preview, "echo 1\necho 2");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn approval_has_expand_only_for_long_multiline_commands() {
        // Expansion is offered only when preview mode would hide additional lines.
        assert!(!approval_has_expand("echo 1"));
        assert!(!approval_has_expand("a\nb\nc\nd\ne"));
        assert!(approval_has_expand("a\nb\nc\nd\ne\nf"));
    }
}
