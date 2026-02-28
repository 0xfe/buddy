//! Startup banner and execution-target formatting helpers.

use buddy::repl::SessionStartupState;
use buddy::tools::execution::{TmuxAttachInfo, TmuxAttachTarget};
use crossterm::style::{Color, Stylize};

/// Build startup line describing session reuse/new session.
pub(crate) fn session_startup_message(
    state: SessionStartupState,
    session_id: &str,
    context_used: u16,
) -> String {
    match state {
        SessionStartupState::ResumedExisting => {
            format!("using existing session \"{session_id}\" ({context_used}% context used)")
        }
        SessionStartupState::StartedNew => {
            format!("using new session \"{session_id}\" ({context_used}% context used)")
        }
    }
}

/// Format a direct attach command for the current tmux target.
pub(crate) fn execution_tmux_attach_command(info: &TmuxAttachInfo) -> String {
    match &info.target {
        TmuxAttachTarget::Local => format!("tmux attach -t {}", info.session),
        TmuxAttachTarget::Ssh { target } => {
            format!("ssh -t {target} tmux attach -t {}", info.session)
        }
        TmuxAttachTarget::Container { engine, container } => {
            format!(
                "{engine} exec -it {container} tmux attach -t {}",
                info.session
            )
        }
    }
}

/// Human-readable target label used in startup banner.
pub(crate) fn execution_target_label(info: Option<&TmuxAttachInfo>) -> String {
    match info {
        Some(TmuxAttachInfo {
            target: TmuxAttachTarget::Local,
            ..
        }) => "localhost".to_string(),
        Some(TmuxAttachInfo {
            target: TmuxAttachTarget::Ssh { target },
            ..
        }) => format!("ssh:{target}"),
        Some(TmuxAttachInfo {
            target: TmuxAttachTarget::Container { container, .. },
            ..
        }) => format!("container:{container}"),
        None => "localhost".to_string(),
    }
}

/// Render initial execution banner with optional attach instructions.
pub(crate) fn render_startup_banner(color: bool, model: &str, tmux_info: Option<&TmuxAttachInfo>) {
    let target = execution_target_label(tmux_info);
    if color {
        eprintln!(
            "{} {} running on {} with model {}",
            "•".with(Color::DarkGrey),
            "buddy".with(Color::Green).bold(),
            target.as_str().with(Color::White).bold(),
            model.with(Color::Yellow).bold(),
        );
    } else {
        eprintln!("• buddy running on {target} with model {model}");
    }

    if let Some(info) = tmux_info {
        let attach = execution_tmux_attach_command(info);
        if color {
            eprintln!(
                "  attach with: {}",
                attach.as_str().with(Color::White).bold()
            );
        } else {
            eprintln!("  attach with: {attach}");
        }
    }
    eprintln!();
}

/// Render session startup line after session creation/resume.
pub(crate) fn render_session_startup_line(
    color: bool,
    state: SessionStartupState,
    session_id: &str,
    context_used: u16,
) {
    let message = session_startup_message(state, session_id, context_used);
    if color {
        eprintln!("{} {}", "•".with(Color::DarkGrey), message);
    } else {
        eprintln!("• {message}");
    }
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::tools::execution::{TmuxAttachInfo, TmuxAttachTarget};

    #[test]
    fn session_startup_message_is_clear() {
        // Startup line should clearly distinguish resumed vs new sessions.
        assert_eq!(
            session_startup_message(SessionStartupState::ResumedExisting, "abcd-1234", 4),
            "using existing session \"abcd-1234\" (4% context used)"
        );
        assert_eq!(
            session_startup_message(SessionStartupState::StartedNew, "abcd-1234", 0),
            "using new session \"abcd-1234\" (0% context used)"
        );
    }

    #[test]
    fn execution_tmux_attach_command_formats_local_target() {
        // Local tmux targets should produce a direct attach command.
        let cmd = execution_tmux_attach_command(&TmuxAttachInfo {
            session: "buddy-ef1d".to_string(),
            window: "shared",
            target: TmuxAttachTarget::Local,
        });
        assert_eq!(cmd, "tmux attach -t buddy-ef1d");
    }
}
