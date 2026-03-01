//! Managed tmux naming and lifecycle script builders.
//!
//! Backends use these helpers to keep session/pane ownership checks consistent
//! across local, container, and SSH transports.

use crate::error::ToolError;
use crate::tools::execution::process::shell_quote;
use crate::tools::execution::types::{
    CreatedTmuxPane, CreatedTmuxSession, ResolvedTmuxTarget, TmuxTargetSelector, TMUX_PANE_TITLE,
    TMUX_WINDOW_NAME,
};

/// Session option flag used to mark buddy-managed tmux sessions.
pub(crate) const TMUX_MANAGED_OPTION: &str = "@buddy_managed";
/// Session/pane option storing the owning buddy namespace prefix.
pub(crate) const TMUX_OWNER_OPTION: &str = "@buddy_owner";

/// Normalize a user-provided tmux fragment into a shell-safe identifier.
pub(crate) fn sanitize_tmux_fragment(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in raw.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_dash = false;
            continue;
        }
        if matches!(ch, '-' | '_') {
            if !previous_dash && !out.is_empty() {
                out.push(ch);
                previous_dash = true;
            }
            continue;
        }
        if !previous_dash && !out.is_empty() {
            out.push('-');
            previous_dash = true;
        }
    }
    let trimmed = out.trim_matches(['-', '_']).to_string();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

/// Canonicalize a requested managed session selector.
pub(crate) fn canonical_session_name(
    owner_prefix: &str,
    default_session: &str,
    requested: Option<&str>,
) -> Result<String, ToolError> {
    let Some(raw) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(default_session.to_string());
    };
    if raw == default_session {
        return Ok(default_session.to_string());
    }
    if raw.starts_with(owner_prefix) {
        return Ok(raw.to_string());
    }
    let fragment = sanitize_tmux_fragment(raw, "session");
    Ok(format!("{owner_prefix}-{fragment}"))
}

/// Canonicalize a requested managed pane selector.
pub(crate) fn canonical_pane_title(
    owner_prefix: &str,
    requested: Option<&str>,
) -> Result<String, ToolError> {
    let Some(raw) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(TMUX_PANE_TITLE.to_string());
    };
    if raw == TMUX_PANE_TITLE {
        return Ok(TMUX_PANE_TITLE.to_string());
    }
    if raw.starts_with(owner_prefix) {
        return Ok(raw.to_string());
    }
    let fragment = sanitize_tmux_fragment(raw, "pane");
    Ok(format!("{owner_prefix}-{fragment}"))
}

/// Build shell script that resolves and validates a managed tmux pane target.
pub(crate) fn resolve_managed_target_script(
    owner_prefix: &str,
    default_session: &str,
    selector: &TmuxTargetSelector,
) -> Result<String, ToolError> {
    let session =
        canonical_session_name(owner_prefix, default_session, selector.session.as_deref())?;
    let pane = canonical_pane_title(owner_prefix, selector.pane.as_deref())?;
    let owner_q = shell_quote(owner_prefix);
    let default_session_q = shell_quote(default_session);
    let session_q = shell_quote(&session);
    let pane_q = shell_quote(&pane);
    let target_q = shell_quote(selector.target.as_deref().unwrap_or_default());
    Ok(format!(
        "set -e\n\
OWNER={owner_q}\n\
DEFAULT_SESSION={default_session_q}\n\
SESSION={session_q}\n\
PANE_TITLE={pane_q}\n\
TARGET={target_q}\n\
if [ -n \"$TARGET\" ]; then\n\
  SESSION=\"$(tmux display-message -p -t \"$TARGET\" '#{{session_name}}' 2>/dev/null || true)\"\n\
  PANE=\"$(tmux display-message -p -t \"$TARGET\" '#{{pane_id}}' 2>/dev/null || true)\"\n\
  PANE_TITLE=\"$(tmux display-message -p -t \"$TARGET\" '#{{pane_title}}' 2>/dev/null || true)\"\n\
else\n\
  PANE=\"$(tmux list-panes -a -F '#{{session_name}}\\t#{{pane_id}}\\t#{{pane_title}}' 2>/dev/null | awk -F '\\t' -v session=\"$SESSION\" -v pane_title=\"$PANE_TITLE\" '$1==session && $3==pane_title {{print $2; exit}}')\"\n\
fi\n\
if [ -z \"$SESSION\" ] || [ -z \"$PANE\" ]; then\n\
  echo \"tmux target not found\" >&2\n\
  exit 1\n\
fi\n\
SESSION_MANAGED=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
SESSION_OWNER=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
if [ \"$SESSION_MANAGED\" != \"1\" ] || [ \"$SESSION_OWNER\" != \"$OWNER\" ]; then\n\
  echo \"tmux session '$SESSION' is not managed by this buddy instance\" >&2\n\
  exit 1\n\
fi\n\
PANE_MANAGED=\"$(tmux show-options -v -p -t \"$PANE\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
PANE_OWNER=\"$(tmux show-options -v -p -t \"$PANE\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
if [ \"$PANE_MANAGED\" != \"1\" ] || [ \"$PANE_OWNER\" != \"$OWNER\" ]; then\n\
  echo \"tmux pane '$PANE' is not managed by this buddy instance\" >&2\n\
  exit 1\n\
fi\n\
IS_DEFAULT=0\n\
if [ \"$SESSION\" = \"$DEFAULT_SESSION\" ] && [ \"$PANE_TITLE\" = '{TMUX_PANE_TITLE}' ]; then\n\
  IS_DEFAULT=1\n\
fi\n\
printf '%s\\n%s\\n%s\\n%s' \"$SESSION\" \"$PANE\" \"$PANE_TITLE\" \"$IS_DEFAULT\"\n"
    ))
}

/// Build shell script that creates or reuses a managed tmux session.
pub(crate) fn create_managed_session_script(
    owner_prefix: &str,
    session: &str,
    max_sessions: usize,
) -> String {
    let owner_q = shell_quote(owner_prefix);
    let session_q = shell_quote(session);
    format!(
        "set -e\n\
OWNER={owner_q}\n\
SESSION={session_q}\n\
WINDOW='{TMUX_WINDOW_NAME}'\n\
PANE_TITLE='{TMUX_PANE_TITLE}'\n\
MAX_SESSIONS={max_sessions}\n\
COUNT=\"$(tmux list-sessions -F '#{{session_name}}\\t#{{@buddy_managed}}\\t#{{@buddy_owner}}' 2>/dev/null | awk -F '\\t' -v owner=\"$OWNER\" '$2==\"1\" && $3==owner {{c++}} END {{print c+0}}')\"\n\
CREATED=0\n\
if tmux has-session -t \"$SESSION\" 2>/dev/null; then\n\
  SESSION_MANAGED=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
  SESSION_OWNER=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
  if [ \"$SESSION_MANAGED\" != \"1\" ] || [ \"$SESSION_OWNER\" != \"$OWNER\" ]; then\n\
    echo \"tmux session '$SESSION' exists but is not managed by this buddy instance\" >&2\n\
    exit 1\n\
  fi\n\
else\n\
  if [ \"$COUNT\" -ge \"$MAX_SESSIONS\" ]; then\n\
    echo \"managed tmux session limit reached ($COUNT/$MAX_SESSIONS)\" >&2\n\
    exit 1\n\
  fi\n\
  tmux new-session -d -s \"$SESSION\" -n \"$WINDOW\"\n\
  CREATED=1\n\
fi\n\
if ! tmux list-windows -t \"$SESSION\" -F '#{{window_name}}' | grep -Fx -- \"$WINDOW\" >/dev/null 2>&1; then\n\
  tmux new-window -d -t \"$SESSION\" -n \"$WINDOW\"\n\
fi\n\
PANE=\"$(tmux list-panes -a -F '#{{session_name}}\\t#{{pane_id}}\\t#{{pane_title}}' | awk -F '\\t' -v session=\"$SESSION\" -v pane_title=\"$PANE_TITLE\" '$1==session && $3==pane_title {{print $2; exit}}')\"\n\
if [ -z \"$PANE\" ]; then\n\
  PANE=\"$(tmux list-panes -t \"$SESSION:$WINDOW\" -F '#{{pane_id}}' | head -n1)\"\n\
  tmux select-pane -t \"$PANE\" -T \"$PANE_TITLE\" >/dev/null 2>&1 || true\n\
fi\n\
tmux set-option -q -t \"$SESSION\" {TMUX_MANAGED_OPTION} 1\n\
tmux set-option -q -t \"$SESSION\" {TMUX_OWNER_OPTION} \"$OWNER\"\n\
tmux set-option -q -p -t \"$PANE\" {TMUX_MANAGED_OPTION} 1\n\
tmux set-option -q -p -t \"$PANE\" {TMUX_OWNER_OPTION} \"$OWNER\"\n\
printf '%s\\n%s\\n%s' \"$SESSION\" \"$PANE\" \"$CREATED\"\n"
    )
}

/// Build shell script that creates or reuses a managed tmux pane.
pub(crate) fn create_managed_pane_script(
    owner_prefix: &str,
    default_session: &str,
    session: Option<&str>,
    pane: &str,
    max_panes: usize,
) -> Result<String, ToolError> {
    let session = canonical_session_name(owner_prefix, default_session, session)?;
    let pane_title = canonical_pane_title(owner_prefix, Some(pane))?;
    let owner_q = shell_quote(owner_prefix);
    let session_q = shell_quote(&session);
    let pane_q = shell_quote(&pane_title);
    Ok(format!(
        "set -e\n\
OWNER={owner_q}\n\
SESSION={session_q}\n\
PANE_TITLE={pane_q}\n\
WINDOW='{TMUX_WINDOW_NAME}'\n\
MAX_PANES={max_panes}\n\
if ! tmux has-session -t \"$SESSION\" 2>/dev/null; then\n\
  echo \"tmux session '$SESSION' was not found\" >&2\n\
  exit 1\n\
fi\n\
SESSION_MANAGED=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
SESSION_OWNER=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
if [ \"$SESSION_MANAGED\" != \"1\" ] || [ \"$SESSION_OWNER\" != \"$OWNER\" ]; then\n\
  echo \"tmux session '$SESSION' is not managed by this buddy instance\" >&2\n\
  exit 1\n\
fi\n\
PANE=\"$(tmux list-panes -a -F '#{{session_name}}\\t#{{pane_id}}\\t#{{pane_title}}' | awk -F '\\t' -v session=\"$SESSION\" -v pane_title=\"$PANE_TITLE\" '$1==session && $3==pane_title {{print $2; exit}}')\"\n\
CREATED=0\n\
if [ -z \"$PANE\" ]; then\n\
  COUNT=\"$(tmux list-panes -a -F '#{{session_name}}\\t#{{@buddy_managed}}\\t#{{@buddy_owner}}' | awk -F '\\t' -v session=\"$SESSION\" -v owner=\"$OWNER\" '$1==session && $2==\"1\" && $3==owner {{c++}} END {{print c+0}}')\"\n\
  if [ \"$COUNT\" -ge \"$MAX_PANES\" ]; then\n\
    echo \"managed tmux pane limit reached in session '$SESSION' ($COUNT/$MAX_PANES)\" >&2\n\
    exit 1\n\
  fi\n\
  if ! tmux list-windows -t \"$SESSION\" -F '#{{window_name}}' | grep -Fx -- \"$WINDOW\" >/dev/null 2>&1; then\n\
    tmux new-window -d -t \"$SESSION\" -n \"$WINDOW\"\n\
  fi\n\
  PANE=\"$(tmux split-window -d -P -F '#{{pane_id}}' -t \"$SESSION:$WINDOW\")\"\n\
  tmux select-pane -t \"$PANE\" -T \"$PANE_TITLE\" >/dev/null 2>&1 || true\n\
  tmux set-option -q -p -t \"$PANE\" {TMUX_MANAGED_OPTION} 1\n\
  tmux set-option -q -p -t \"$PANE\" {TMUX_OWNER_OPTION} \"$OWNER\"\n\
  CREATED=1\n\
fi\n\
printf '%s\\n%s\\n%s\\n%s' \"$SESSION\" \"$PANE\" \"$PANE_TITLE\" \"$CREATED\"\n"
    ))
}

/// Build shell script that kills one managed tmux pane.
pub(crate) fn kill_managed_pane_script(
    owner_prefix: &str,
    default_session: &str,
    session: Option<&str>,
    pane: &str,
) -> Result<String, ToolError> {
    let session = canonical_session_name(owner_prefix, default_session, session)?;
    let pane_title = canonical_pane_title(owner_prefix, Some(pane))?;
    let owner_q = shell_quote(owner_prefix);
    let session_q = shell_quote(&session);
    let pane_q = shell_quote(&pane_title);
    let default_session_q = shell_quote(default_session);
    Ok(format!(
        "set -e\n\
OWNER={owner_q}\n\
SESSION={session_q}\n\
PANE_TITLE={pane_q}\n\
DEFAULT_SESSION={default_session_q}\n\
if ! tmux has-session -t \"$SESSION\" 2>/dev/null; then\n\
  echo \"tmux session '$SESSION' was not found\" >&2\n\
  exit 1\n\
fi\n\
SESSION_MANAGED=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
SESSION_OWNER=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
if [ \"$SESSION_MANAGED\" != \"1\" ] || [ \"$SESSION_OWNER\" != \"$OWNER\" ]; then\n\
  echo \"tmux session '$SESSION' is not managed by this buddy instance\" >&2\n\
  exit 1\n\
fi\n\
PANE=\"$(tmux list-panes -a -F '#{{session_name}}\\t#{{pane_id}}\\t#{{pane_title}}' | awk -F '\\t' -v session=\"$SESSION\" -v pane_title=\"$PANE_TITLE\" '$1==session && $3==pane_title {{print $2; exit}}')\"\n\
if [ -z \"$PANE\" ]; then\n\
  echo \"tmux pane '$PANE_TITLE' was not found in session '$SESSION'\" >&2\n\
  exit 1\n\
fi\n\
if [ \"$SESSION\" = \"$DEFAULT_SESSION\" ] && [ \"$PANE_TITLE\" = '{TMUX_PANE_TITLE}' ]; then\n\
  echo \"cannot kill default shared pane\" >&2\n\
  exit 1\n\
fi\n\
PANE_MANAGED=\"$(tmux show-options -v -p -t \"$PANE\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
PANE_OWNER=\"$(tmux show-options -v -p -t \"$PANE\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
if [ \"$PANE_MANAGED\" != \"1\" ] || [ \"$PANE_OWNER\" != \"$OWNER\" ]; then\n\
  echo \"tmux pane '$PANE_TITLE' is not managed by this buddy instance\" >&2\n\
  exit 1\n\
fi\n\
tmux kill-pane -t \"$PANE\"\n\
printf '%s\\n%s' \"$SESSION\" \"$PANE\"\n"
    ))
}

/// Build shell script that kills one managed tmux session.
pub(crate) fn kill_managed_session_script(
    owner_prefix: &str,
    default_session: &str,
    session: &str,
) -> Result<String, ToolError> {
    let session = canonical_session_name(owner_prefix, default_session, Some(session))?;
    let owner_q = shell_quote(owner_prefix);
    let session_q = shell_quote(&session);
    let default_session_q = shell_quote(default_session);
    Ok(format!(
        "set -e\n\
OWNER={owner_q}\n\
SESSION={session_q}\n\
DEFAULT_SESSION={default_session_q}\n\
if [ \"$SESSION\" = \"$DEFAULT_SESSION\" ]; then\n\
  echo \"cannot kill default managed tmux session\" >&2\n\
  exit 1\n\
fi\n\
if ! tmux has-session -t \"$SESSION\" 2>/dev/null; then\n\
  echo \"tmux session '$SESSION' was not found\" >&2\n\
  exit 1\n\
fi\n\
SESSION_MANAGED=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_MANAGED_OPTION} 2>/dev/null || true)\"\n\
SESSION_OWNER=\"$(tmux show-options -v -t \"$SESSION\" {TMUX_OWNER_OPTION} 2>/dev/null || true)\"\n\
if [ \"$SESSION_MANAGED\" != \"1\" ] || [ \"$SESSION_OWNER\" != \"$OWNER\" ]; then\n\
  echo \"tmux session '$SESSION' is not managed by this buddy instance\" >&2\n\
  exit 1\n\
fi\n\
tmux kill-session -t \"$SESSION\"\n\
printf '%s' \"$SESSION\"\n"
    ))
}

/// Parse `resolve_managed_target_script` output.
pub(crate) fn parse_resolved_target(output: &str) -> Option<ResolvedTmuxTarget> {
    let mut lines = output.lines();
    let session = lines.next()?.trim().to_string();
    let pane_id = lines.next()?.trim().to_string();
    let pane_title = lines.next()?.trim().to_string();
    let is_default_shared = matches!(lines.next()?.trim(), "1");
    if session.is_empty() || pane_id.is_empty() {
        return None;
    }
    Some(ResolvedTmuxTarget {
        session,
        pane_id,
        pane_title,
        is_default_shared,
    })
}

/// Parse `create_managed_session_script` output.
pub(crate) fn parse_created_session(output: &str) -> Option<CreatedTmuxSession> {
    let mut lines = output.lines();
    let session = lines.next()?.trim().to_string();
    let pane_id = lines.next()?.trim().to_string();
    let created = matches!(lines.next()?.trim(), "1");
    if session.is_empty() || pane_id.is_empty() {
        return None;
    }
    Some(CreatedTmuxSession {
        session,
        pane_id,
        created,
    })
}

/// Parse `create_managed_pane_script` output.
pub(crate) fn parse_created_pane(output: &str) -> Option<CreatedTmuxPane> {
    let mut lines = output.lines();
    let session = lines.next()?.trim().to_string();
    let pane_id = lines.next()?.trim().to_string();
    let pane_title = lines.next()?.trim().to_string();
    let created = matches!(lines.next()?.trim(), "1");
    if session.is_empty() || pane_id.is_empty() || pane_title.is_empty() {
        return None;
    }
    Some(CreatedTmuxPane {
        session,
        pane_id,
        pane_title,
        created,
    })
}

/// Parse `<session>\n<pane_id>` output from pane-kill script.
pub(crate) fn parse_killed_pane(output: &str) -> Option<(String, String)> {
    let mut lines = output.lines();
    let session = lines.next()?.trim().to_string();
    let pane_id = lines.next()?.trim().to_string();
    if session.is_empty() || pane_id.is_empty() {
        return None;
    }
    Some((session, pane_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_session_name_prefixes_non_prefixed_values() {
        let out =
            canonical_session_name("buddy-agent-mo", "buddy-agent-mo", Some("build")).unwrap();
        assert_eq!(out, "buddy-agent-mo-build");
    }

    #[test]
    fn canonical_pane_title_keeps_shared_default() {
        let out = canonical_pane_title("buddy-agent-mo", None).unwrap();
        assert_eq!(out, "shared");
    }
}
