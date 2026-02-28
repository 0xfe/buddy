//! Tmux prompt bootstrap for buddy command markers.

use crate::error::ToolError;
use crate::tools::execution::types::ContainerTmuxContext;

use super::capture::{
    wait_for_container_tmux_any_prompt, wait_for_local_tmux_any_prompt, wait_for_tmux_any_prompt,
};
use super::send_keys::{send_container_tmux_line, send_local_tmux_line, send_tmux_line};

/// Prompt setup script injected into managed tmux panes.
pub(crate) fn tmux_prompt_setup_script() -> &'static str {
    "if [ \"${BUDDY_PROMPT_LAYOUT:-}\" != \"v3\" ]; then \
BUDDY_PROMPT_LAYOUT=v3; \
BUDDY_CMD_SEQ=${BUDDY_CMD_SEQ:-0}; \
__buddy_next_id() { BUDDY_CMD_SEQ=$((BUDDY_CMD_SEQ + 1)); BUDDY_CMD_ID=$BUDDY_CMD_SEQ; }; \
__buddy_prompt_id() { printf '%s' \"${BUDDY_CMD_ID:-0}\"; }; \
if [ -n \"${BASH_VERSION:-}\" ]; then \
BUDDY_BASE_PS1=${BUDDY_BASE_PS1:-$PS1}; \
__buddy_precmd() { __buddy_next_id; }; \
case \";${PROMPT_COMMAND:-};\" in \
  *\";__buddy_precmd;\"*) ;; \
  *) PROMPT_COMMAND=\"__buddy_precmd${PROMPT_COMMAND:+;${PROMPT_COMMAND}}\" ;; \
esac; \
PS1='[buddy $(__buddy_prompt_id): \\?] '\"$BUDDY_BASE_PS1\"; \
elif [ -n \"${ZSH_VERSION:-}\" ]; then \
BUDDY_BASE_PROMPT=${BUDDY_BASE_PROMPT:-$PROMPT}; \
__buddy_precmd() { __buddy_next_id; }; \
if (( ${precmd_functions[(Ie)__buddy_precmd]} == 0 )); then \
  precmd_functions=(__buddy_precmd $precmd_functions); \
fi; \
setopt PROMPT_SUBST; \
PROMPT='[buddy $(__buddy_prompt_id): %?] '\"$BUDDY_BASE_PROMPT\"; \
else \
BUDDY_BASE_PS1=${BUDDY_BASE_PS1:-$PS1}; \
PS1='[buddy $(__buddy_next_id): $?] '\"$BUDDY_BASE_PS1\"; \
fi; \
fi"
}

/// Ensure prompt bootstrap is installed in ssh tmux pane.
pub(crate) async fn ensure_tmux_prompt_setup(
    target: &str,
    control_path: &std::path::Path,
    pane_id: &str,
) -> Result<(), ToolError> {
    let configure_prompt = tmux_prompt_setup_script();

    send_tmux_line(target, control_path, pane_id, configure_prompt).await?;
    wait_for_tmux_any_prompt(target, control_path, pane_id).await?;
    send_tmux_line(target, control_path, pane_id, "clear").await?;
    wait_for_tmux_any_prompt(target, control_path, pane_id).await
}

/// Ensure prompt bootstrap is installed in local tmux pane.
pub(crate) async fn ensure_local_tmux_prompt_setup(pane_id: &str) -> Result<(), ToolError> {
    let configure_prompt = tmux_prompt_setup_script();
    send_local_tmux_line(pane_id, configure_prompt).await?;
    wait_for_local_tmux_any_prompt(pane_id).await?;
    send_local_tmux_line(pane_id, "clear").await?;
    wait_for_local_tmux_any_prompt(pane_id).await
}

/// Ensure prompt bootstrap is installed in container tmux pane.
pub(crate) async fn ensure_container_tmux_prompt_setup(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
) -> Result<(), ToolError> {
    let configure_prompt = tmux_prompt_setup_script();
    send_container_tmux_line(ctx, pane_id, configure_prompt).await?;
    wait_for_container_tmux_any_prompt(ctx, pane_id).await?;
    send_container_tmux_line(ctx, pane_id, "clear").await?;
    wait_for_container_tmux_any_prompt(ctx, pane_id).await
}
