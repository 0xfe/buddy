//! Tmux key injection helpers for local/ssh/container targets.

use crate::error::ToolError;

use crate::tools::execution::process::{
    ensure_success, run_container_tmux_sh_process, run_sh_process, run_ssh_raw_process, shell_quote,
};
use crate::tools::execution::types::{ContainerTmuxContext, SendKeysOptions};

/// Send a full command line (text + Enter) over ssh tmux.
pub(in crate::tools::execution) async fn send_tmux_line(
    target: &str,
    control_path: &std::path::Path,
    pane_id: &str,
    text: &str,
) -> Result<(), ToolError> {
    let pane_q = shell_quote(pane_id);
    let text_q = shell_quote(text);

    let send_text = run_ssh_raw_process(
        target,
        control_path,
        &format!("tmux send-keys -l -t {pane_q} {text_q}"),
        None,
    )
    .await?;
    ensure_success(send_text, "failed to send keys to tmux pane".into())?;

    let send_enter = run_ssh_raw_process(
        target,
        control_path,
        &format!("tmux send-keys -t {pane_q} Enter"),
        None,
    )
    .await?;
    ensure_success(send_enter, "failed to send Enter to tmux pane".into())?;

    Ok(())
}

/// Send a full command line (text + Enter) to local tmux.
pub(in crate::tools::execution) async fn send_local_tmux_line(
    pane_id: &str,
    text: &str,
) -> Result<(), ToolError> {
    let pane_q = shell_quote(pane_id);
    let text_q = shell_quote(text);
    let send_text = run_sh_process(
        "sh",
        &format!("tmux send-keys -l -t {pane_q} {text_q}"),
        None,
    )
    .await?;
    ensure_success(send_text, "failed to send keys to tmux pane".into())?;

    let send_enter =
        run_sh_process("sh", &format!("tmux send-keys -t {pane_q} Enter"), None).await?;
    ensure_success(send_enter, "failed to send Enter to tmux pane".into())?;
    Ok(())
}

/// Send a full command line (text + Enter) to container tmux.
pub(in crate::tools::execution) async fn send_container_tmux_line(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
    text: &str,
) -> Result<(), ToolError> {
    let pane_q = shell_quote(pane_id);
    let text_q = shell_quote(text);
    let send_text = run_container_tmux_sh_process(
        ctx,
        &format!("tmux send-keys -l -t {pane_q} {text_q}"),
        None,
    )
    .await?;
    ensure_success(send_text, "failed to send keys to tmux pane".into())?;

    let send_enter =
        run_container_tmux_sh_process(ctx, &format!("tmux send-keys -t {pane_q} Enter"), None)
            .await?;
    ensure_success(send_enter, "failed to send Enter to tmux pane".into())?;
    Ok(())
}

/// Send key/literal/enter sequence to local tmux.
pub(in crate::tools::execution) async fn send_local_tmux_keys(
    target: &str,
    options: &SendKeysOptions,
) -> Result<(), ToolError> {
    if let Some(text) = options.literal_text.as_deref() {
        if !text.is_empty() {
            let cmd = build_tmux_send_literal_command(target, text);
            let output = run_sh_process("sh", &cmd, None).await?;
            ensure_success(output, "failed to send literal keys to tmux pane".into())?;
        }
    }
    if !options.keys.is_empty() {
        let cmd = build_tmux_send_keys_command(target, &options.keys);
        let output = run_sh_process("sh", &cmd, None).await?;
        ensure_success(output, "failed to send key sequence to tmux pane".into())?;
    }
    if options.press_enter {
        let cmd = build_tmux_send_enter_command(target);
        let output = run_sh_process("sh", &cmd, None).await?;
        ensure_success(output, "failed to send Enter to tmux pane".into())?;
    }
    Ok(())
}

/// Send key/literal/enter sequence to ssh tmux.
pub(in crate::tools::execution) async fn send_remote_tmux_keys(
    target: &str,
    control_path: &std::path::Path,
    pane_id: &str,
    options: &SendKeysOptions,
) -> Result<(), ToolError> {
    if let Some(text) = options.literal_text.as_deref() {
        if !text.is_empty() {
            let cmd = build_tmux_send_literal_command(pane_id, text);
            let output = run_ssh_raw_process(target, control_path, &cmd, None).await?;
            ensure_success(output, "failed to send literal keys to tmux pane".into())?;
        }
    }
    if !options.keys.is_empty() {
        let cmd = build_tmux_send_keys_command(pane_id, &options.keys);
        let output = run_ssh_raw_process(target, control_path, &cmd, None).await?;
        ensure_success(output, "failed to send key sequence to tmux pane".into())?;
    }
    if options.press_enter {
        let cmd = build_tmux_send_enter_command(pane_id);
        let output = run_ssh_raw_process(target, control_path, &cmd, None).await?;
        ensure_success(output, "failed to send Enter to tmux pane".into())?;
    }
    Ok(())
}

/// Send key/literal/enter sequence to container tmux.
pub(in crate::tools::execution) async fn send_container_tmux_keys(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
    options: &SendKeysOptions,
) -> Result<(), ToolError> {
    if let Some(text) = options.literal_text.as_deref() {
        if !text.is_empty() {
            let cmd = build_tmux_send_literal_command(pane_id, text);
            let output = run_container_tmux_sh_process(ctx, &cmd, None).await?;
            ensure_success(output, "failed to send literal keys to tmux pane".into())?;
        }
    }
    if !options.keys.is_empty() {
        let cmd = build_tmux_send_keys_command(pane_id, &options.keys);
        let output = run_container_tmux_sh_process(ctx, &cmd, None).await?;
        ensure_success(output, "failed to send key sequence to tmux pane".into())?;
    }
    if options.press_enter {
        let cmd = build_tmux_send_enter_command(pane_id);
        let output = run_container_tmux_sh_process(ctx, &cmd, None).await?;
        ensure_success(output, "failed to send Enter to tmux pane".into())?;
    }
    Ok(())
}

/// Build literal tmux send-keys shell command.
pub(in crate::tools::execution) fn build_tmux_send_literal_command(
    target: &str,
    text: &str,
) -> String {
    let target_q = shell_quote(target);
    let text_q = shell_quote(text);
    format!("tmux send-keys -l -t {target_q} {text_q}")
}

/// Build tmux send-keys command for key sequence arguments.
pub(in crate::tools::execution) fn build_tmux_send_keys_command(
    target: &str,
    keys: &[String],
) -> String {
    let target_q = shell_quote(target);
    let keys_q = keys
        .iter()
        .map(|key| shell_quote(key))
        .collect::<Vec<_>>()
        .join(" ");
    format!("tmux send-keys -t {target_q} {keys_q}")
}

/// Build tmux Enter key command.
pub(in crate::tools::execution) fn build_tmux_send_enter_command(target: &str) -> String {
    let target_q = shell_quote(target);
    format!("tmux send-keys -t {target_q} Enter")
}
