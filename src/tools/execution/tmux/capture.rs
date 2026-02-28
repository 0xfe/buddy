//! Tmux capture-pane helpers for local/ssh/container targets.

use crate::error::ToolError;
use tokio::time::{sleep, Duration};

use crate::tools::execution::process::{
    ensure_success, run_container_tmux_sh_process, run_sh_process, run_ssh_raw_process, shell_quote,
};
use crate::tools::execution::types::{CapturePaneOptions, ContainerTmuxContext};

use super::run::latest_prompt_marker;

/// Build `tmux capture-pane` shell command from capture options.
pub(in crate::tools::execution) fn build_capture_pane_command(
    target: &str,
    options: &CapturePaneOptions,
) -> String {
    let mut cmd = String::from("tmux capture-pane -p");
    if options.join_wrapped_lines {
        cmd.push_str(" -J");
    }
    if options.preserve_trailing_spaces {
        cmd.push_str(" -N");
    }
    if options.include_escape_sequences {
        cmd.push_str(" -e");
    }
    if options.escape_non_printable {
        cmd.push_str(" -C");
    }
    if options.include_alternate_screen {
        cmd.push_str(" -a");
    }
    if let Some(start) = options.start.as_deref() {
        cmd.push_str(" -S ");
        cmd.push_str(&shell_quote(start));
    }
    if let Some(end) = options.end.as_deref() {
        cmd.push_str(" -E ");
        cmd.push_str(&shell_quote(end));
    }
    cmd.push_str(" -t ");
    cmd.push_str(&shell_quote(target));
    cmd
}

/// Capture options used for full tmux history parsing.
pub(in crate::tools::execution) fn full_history_capture_options() -> CapturePaneOptions {
    CapturePaneOptions {
        start: Some("-".to_string()),
        end: Some("-".to_string()),
        ..CapturePaneOptions::default()
    }
}

/// Capture from local tmux target.
pub(in crate::tools::execution) async fn run_local_capture_pane(
    pane_target: &str,
    options: &CapturePaneOptions,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_target, options);
    let output = run_sh_process("sh", &capture_cmd, None).await?;
    match ensure_success(output, "failed to capture tmux pane".into()) {
        Ok(out) => Ok(out.stdout),
        Err(err) if should_fallback_from_alternate_screen(options, &err) => {
            let mut fallback = options.clone();
            fallback.include_alternate_screen = false;
            let fallback_cmd = build_capture_pane_command(pane_target, &fallback);
            let fallback_output = run_sh_process("sh", &fallback_cmd, None).await?;
            let out = ensure_success(
                fallback_output,
                "failed to capture tmux pane after alternate-screen fallback".into(),
            )?;
            Ok(format!(
                "{}\n\n[notice] alternate screen was not active; captured main pane instead.",
                out.stdout
            ))
        }
        Err(err) => Err(err),
    }
}

/// Capture from remote ssh-backed tmux target.
pub(in crate::tools::execution) async fn run_remote_capture_pane(
    target: &str,
    control_path: &std::path::Path,
    pane_target: &str,
    options: &CapturePaneOptions,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_target, options);
    let output = run_ssh_raw_process(target, control_path, &capture_cmd, None).await?;
    match ensure_success(output, "failed to capture tmux pane".into()) {
        Ok(out) => Ok(out.stdout),
        Err(err) if should_fallback_from_alternate_screen(options, &err) => {
            let mut fallback = options.clone();
            fallback.include_alternate_screen = false;
            let fallback_cmd = build_capture_pane_command(pane_target, &fallback);
            let fallback_output =
                run_ssh_raw_process(target, control_path, &fallback_cmd, None).await?;
            let out = ensure_success(
                fallback_output,
                "failed to capture tmux pane after alternate-screen fallback".into(),
            )?;
            Ok(format!(
                "{}\n\n[notice] alternate screen was not active; captured main pane instead.",
                out.stdout
            ))
        }
        Err(err) => Err(err),
    }
}

/// Capture from container tmux target.
pub(in crate::tools::execution) async fn run_container_capture_pane(
    ctx: &ContainerTmuxContext,
    pane_target: &str,
    options: &CapturePaneOptions,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_target, options);
    let output = run_container_tmux_sh_process(ctx, &capture_cmd, None).await?;
    match ensure_success(output, "failed to capture tmux pane".into()) {
        Ok(out) => Ok(out.stdout),
        Err(err) if should_fallback_from_alternate_screen(options, &err) => {
            let mut fallback = options.clone();
            fallback.include_alternate_screen = false;
            let fallback_cmd = build_capture_pane_command(pane_target, &fallback);
            let fallback_output = run_container_tmux_sh_process(ctx, &fallback_cmd, None).await?;
            let out = ensure_success(
                fallback_output,
                "failed to capture tmux pane after alternate-screen fallback".into(),
            )?;
            Ok(format!(
                "{}\n\n[notice] alternate screen was not active; captured main pane instead.",
                out.stdout
            ))
        }
        Err(err) => Err(err),
    }
}

/// Whether alternate-screen fallback should be attempted.
pub(in crate::tools::execution) fn should_fallback_from_alternate_screen(
    options: &CapturePaneOptions,
    err: &ToolError,
) -> bool {
    options.include_alternate_screen
        && err
            .to_string()
            .to_ascii_lowercase()
            .contains("no alternate screen")
}

/// Capture full history for ssh tmux pane parsing.
pub(in crate::tools::execution) async fn capture_tmux_pane(
    target: &str,
    control_path: &std::path::Path,
    pane_id: &str,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_id, &full_history_capture_options());
    let capture = run_ssh_raw_process(target, control_path, &capture_cmd, None).await?;
    ensure_success(capture, "failed to capture tmux pane".into()).map(|out| out.stdout)
}

/// Capture full history for local tmux pane parsing.
pub(in crate::tools::execution) async fn capture_local_tmux_pane(
    pane_id: &str,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_id, &full_history_capture_options());
    let capture = run_sh_process("sh", &capture_cmd, None).await?;
    ensure_success(capture, "failed to capture tmux pane".into()).map(|out| out.stdout)
}

/// Capture full history for container tmux pane parsing.
pub(in crate::tools::execution) async fn capture_container_tmux_pane(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_id, &full_history_capture_options());
    let capture = run_container_tmux_sh_process(ctx, &capture_cmd, None).await?;
    ensure_success(capture, "failed to capture tmux pane".into()).map(|out| out.stdout)
}

/// Wait until any prompt marker is visible in ssh tmux pane.
pub(in crate::tools::execution) async fn wait_for_tmux_any_prompt(
    target: &str,
    control_path: &std::path::Path,
    pane_id: &str,
) -> Result<(), ToolError> {
    const MAX_POLLS: usize = 72000;
    for _ in 0..MAX_POLLS {
        let capture = capture_tmux_pane(target, control_path, pane_id).await?;
        if latest_prompt_marker(&capture).is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }

    Err(ToolError::ExecutionFailed(
        "timed out waiting for tmux prompt initialization".into(),
    ))
}

/// Wait until any prompt marker is visible in local tmux pane.
pub(in crate::tools::execution) async fn wait_for_local_tmux_any_prompt(
    pane_id: &str,
) -> Result<(), ToolError> {
    const MAX_POLLS: usize = 72000;
    for _ in 0..MAX_POLLS {
        let capture = capture_local_tmux_pane(pane_id).await?;
        if latest_prompt_marker(&capture).is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }

    Err(ToolError::ExecutionFailed(
        "timed out waiting for tmux prompt initialization".into(),
    ))
}

/// Wait until any prompt marker is visible in container tmux pane.
pub(in crate::tools::execution) async fn wait_for_container_tmux_any_prompt(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
) -> Result<(), ToolError> {
    const MAX_POLLS: usize = 72000;
    for _ in 0..MAX_POLLS {
        let capture = capture_container_tmux_pane(ctx, pane_id).await?;
        if latest_prompt_marker(&capture).is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }

    Err(ToolError::ExecutionFailed(
        "timed out waiting for tmux prompt initialization".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_capture_pane_command_uses_defaults() {
        let cmd = build_capture_pane_command("%1", &CapturePaneOptions::default());
        assert_eq!(cmd, "tmux capture-pane -p -J -t '%1'");
    }

    #[test]
    fn full_history_capture_options_sets_explicit_history_bounds() {
        let opts = full_history_capture_options();
        assert_eq!(opts.start.as_deref(), Some("-"));
        assert_eq!(opts.end.as_deref(), Some("-"));
    }

    #[test]
    fn build_capture_pane_command_honors_requested_flags() {
        let cmd = build_capture_pane_command(
            "%11",
            &CapturePaneOptions {
                target: None,
                start: Some("-40".to_string()),
                end: Some("20".to_string()),
                join_wrapped_lines: false,
                preserve_trailing_spaces: true,
                include_escape_sequences: true,
                escape_non_printable: true,
                include_alternate_screen: true,
                delay: Duration::from_millis(250),
            },
        );
        assert_eq!(
            cmd,
            "tmux capture-pane -p -N -e -C -a -S '-40' -E '20' -t '%11'"
        );
    }

    #[test]
    fn fallback_from_alternate_screen_only_when_requested() {
        let err = ToolError::ExecutionFailed(
            "failed to capture tmux pane: no alternate screen".to_string(),
        );
        let mut with_alt = CapturePaneOptions::default();
        with_alt.include_alternate_screen = true;
        assert!(should_fallback_from_alternate_screen(&with_alt, &err));

        let without_alt = CapturePaneOptions::default();
        assert!(!should_fallback_from_alternate_screen(&without_alt, &err));
    }
}
