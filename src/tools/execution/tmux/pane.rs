//! Tmux managed-session pane resolution helpers.

use crate::error::ToolError;

use crate::tools::execution::process::{
    ensure_success, run_container_tmux_sh_process, run_sh_process, run_ssh_raw_process, shell_quote,
};
use crate::tools::execution::types::{
    ContainerTmuxContext, EnsuredTmuxPane, TMUX_PANE_TITLE, TMUX_WINDOW_NAME,
};

/// Script that ensures a shared pane exists and returns `<pane_id>\n<created_flag>`.
pub(in crate::tools::execution) fn ensure_tmux_pane_script(tmux_session: &str) -> String {
    let session_q = shell_quote(tmux_session);
    let window_q = shell_quote(TMUX_WINDOW_NAME);
    let pane_title_q = shell_quote(TMUX_PANE_TITLE);
    format!(
        "set -e\n\
SESSION={session_q}\n\
WINDOW={window_q}\n\
PANE_TITLE={pane_title_q}\n\
CREATED=0\n\
if tmux has-session -t \"$SESSION\" 2>/dev/null; then\n\
  :\n\
else\n\
  tmux new-session -d -s \"$SESSION\" -n \"$WINDOW\"\n\
  CREATED=1\n\
fi\n\
if ! tmux list-windows -t \"$SESSION\" -F '#{{window_name}}' | grep -Fx -- \"$WINDOW\" >/dev/null 2>&1; then\n\
  tmux new-window -d -t \"$SESSION\" -n \"$WINDOW\"\n\
  CREATED=1\n\
fi\n\
PANE=\"$(tmux list-panes -t \"$SESSION:$WINDOW\" -F '#{{pane_id}}\\t#{{pane_title}}' | awk -F '\\t' '$2==\"'\"$PANE_TITLE\"'\" {{print $1; exit}}')\"\n\
if [ -z \"$PANE\" ]; then\n\
  if [ \"$CREATED\" = \"1\" ]; then\n\
    PANE=\"$(tmux list-panes -t \"$SESSION:$WINDOW\" -F '#{{pane_id}}' | head -n1)\"\n\
  else\n\
    PANE_COUNT=\"$(tmux list-panes -t \"$SESSION:$WINDOW\" -F '#{{pane_id}}' | wc -l | tr -d '[:space:]')\"\n\
    if [ \"$PANE_COUNT\" = \"1\" ]; then\n\
      PANE=\"$(tmux list-panes -t \"$SESSION:$WINDOW\" -F '#{{pane_id}}' | head -n1)\"\n\
    else\n\
      PANE=\"$(tmux split-window -d -P -F '#{{pane_id}}' -t \"$SESSION:$WINDOW\")\"\n\
    fi\n\
    CREATED=1\n\
  fi\n\
  if [ -n \"$PANE\" ]; then\n\
    tmux select-pane -t \"$PANE\" -T \"$PANE_TITLE\" >/dev/null 2>&1 || true\n\
  fi\n\
fi\n\
if [ -z \"$PANE\" ]; then\n\
  echo \"failed to resolve tmux pane target for $SESSION:$WINDOW\" >&2\n\
  exit 1\n\
fi\n\
printf '%s\n%s' \"$PANE\" \"$CREATED\"\n"
    )
}

/// Ensure shared pane exists on ssh target.
pub(in crate::tools::execution) async fn ensure_tmux_pane(
    target: &str,
    control_path: &std::path::Path,
    tmux_session: &str,
) -> Result<EnsuredTmuxPane, ToolError> {
    let script = ensure_tmux_pane_script(tmux_session);
    let output = run_ssh_raw_process(target, control_path, &script, None).await?;
    let output = ensure_success(output, "failed to prepare tmux session pane".into())?;
    parse_ensured_tmux_pane(&output.stdout)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to resolve tmux pane target".into()))
}

/// Ensure shared pane exists on local target.
pub(in crate::tools::execution) async fn ensure_local_tmux_pane(
    tmux_session: &str,
) -> Result<EnsuredTmuxPane, ToolError> {
    let script = ensure_tmux_pane_script(tmux_session);
    let output = run_sh_process("sh", &script, None).await?;
    let output = ensure_success(output, "failed to prepare local tmux session pane".into())?;
    parse_ensured_tmux_pane(&output.stdout)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to resolve tmux pane target".into()))
}

/// Ensure shared pane exists inside container target.
pub(in crate::tools::execution) async fn ensure_container_tmux_pane(
    ctx: &ContainerTmuxContext,
    tmux_session: &str,
) -> Result<EnsuredTmuxPane, ToolError> {
    let script = ensure_tmux_pane_script(tmux_session);
    let output = run_container_tmux_sh_process(ctx, &script, None).await?;
    let output = ensure_success(
        output,
        "failed to prepare container tmux session pane".into(),
    )?;
    parse_ensured_tmux_pane(&output.stdout)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to resolve tmux pane target".into()))
}

/// Parse pane ID and created flag returned by `ensure_tmux_pane_script`.
pub(in crate::tools::execution) fn parse_ensured_tmux_pane(
    output: &str,
) -> Option<EnsuredTmuxPane> {
    let mut lines = output.lines();
    let pane_id = lines.next()?.trim();
    let created_raw = lines.next()?.trim();
    if pane_id.is_empty() {
        return None;
    }
    let created = match created_raw {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(EnsuredTmuxPane {
        pane_id: pane_id.to_string(),
        created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_tmux_pane_script_uses_explicit_session_window_target() {
        let script = ensure_tmux_pane_script("buddy");
        assert!(script.contains("CREATED=0"));
        assert!(script.contains("CREATED=1"));
        assert!(script.contains("tmux new-session -d -s \"$SESSION\" -n \"$WINDOW\""));
        assert!(script.contains("tmux new-window -d -t \"$SESSION\" -n \"$WINDOW\""));
        assert!(script.contains("tmux split-window -d -P -F '#{pane_id}' -t \"$SESSION:$WINDOW\""));
        assert!(script.contains("tmux select-pane -t \"$PANE\" -T \"$PANE_TITLE\""));
    }

    #[test]
    fn parse_ensured_tmux_pane_reads_pane_and_created_flag() {
        assert_eq!(
            parse_ensured_tmux_pane("%3\n1"),
            Some(EnsuredTmuxPane {
                pane_id: "%3".to_string(),
                created: true,
            })
        );
        assert_eq!(
            parse_ensured_tmux_pane("%7\n0"),
            Some(EnsuredTmuxPane {
                pane_id: "%7".to_string(),
                created: false,
            })
        );
        assert!(parse_ensured_tmux_pane("").is_none());
        assert!(parse_ensured_tmux_pane("%3\n2").is_none());
    }
}
