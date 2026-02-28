//! Startup banner and execution-target formatting helpers.

use crate::repl_support::SessionStartupState;
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
