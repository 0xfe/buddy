//! Approval prompt rendering and decision helpers.

use buddy::repl::{mark_task_running, ApprovalDecision, BackgroundTask, PendingApproval};
use buddy::runtime::{
    ApprovalDecision as RuntimeApprovalDecision, BuddyRuntimeHandle, RuntimeCommand,
};
use buddy::tools::execution::TmuxAttachInfo;
use buddy::ui::render::RenderSink;
use buddy::ui::theme::{self, ThemeToken};
use crossterm::style::{Color, Stylize};

/// Build the target label used in approval prompts.
pub(crate) fn approval_prompt_actor(
    ssh_target: Option<&str>,
    container: Option<&str>,
    tmux_info: Option<&TmuxAttachInfo>,
) -> String {
    let mut actor = if let Some(target) = ssh_target {
        format!("ssh:{target}")
    } else if let Some(container) = container {
        format!("container:{container}")
    } else {
        "local".to_string()
    };

    if let Some(info) = tmux_info {
        actor.push_str(&format!(" (tmux:{})", info.session));
    }
    actor
}

/// Render the shell approval request block.
pub(crate) fn render_shell_approval_request(
    color: bool,
    renderer: &dyn RenderSink,
    actor: &str,
    command: &str,
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
    renderer.approval_block(&format_approval_command_block(command));
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
            approval_prompt_actor(Some("dev@host"), Some("box"), None),
            "ssh:dev@host"
        );
        assert_eq!(
            approval_prompt_actor(None, Some("box"), None),
            "container:box"
        );
        assert_eq!(approval_prompt_actor(None, None, None), "local");
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
            approval_prompt_actor(None, None, Some(&info)),
            "local (tmux:buddy-a1b2)"
        );
    }

    #[test]
    fn approval_command_block_formats_multiline_commands() {
        // Multiline commands should preserve subsequent lines with continuation indent.
        let block = format_approval_command_block("echo 1\necho 2");
        assert_eq!(block, "$ echo 1\n  echo 2");
    }
}
