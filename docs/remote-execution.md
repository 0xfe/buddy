# Remote Execution — SSH and tmux

The remote execution subsystem lets the agent work on a distant machine as if
it were local. It combines an SSH ControlMaster connection (for low-overhead
command dispatch) with a persistent tmux session (for interactive visibility
and fire-and-forget commands). This document explains how these pieces fit
together, with particular focus on the prompt-marker protocol used to reliably
extract command output from the tmux scrollback buffer.

---

## Overview

When launched with `--ssh user@host`, the agent:

1. Opens one persistent SSH ControlMaster socket (one TCP connection, reused
   for all subsequent operations).
2. Creates (or reattaches to) a named tmux session on the remote host.
3. Ensures a shared tmux window and pane are ready.
4. Installs a custom shell prompt in that pane that embeds a monotonically
   increasing command-id and the last exit code.
5. Runs all `run_shell` commands by injecting them into the tmux pane (not
   via fresh SSH processes), so the operator can attach to the session and
   watch what the agent is doing.

The result is a session that is:
- **Transparent**: the operator can attach to the same tmux session from
  another terminal window.
- **Auditable**: every command appears in the pane's scrollback exactly as
  typed.
- **Persistent**: if the agent process is restarted, it reattaches to the same
  tmux session.

---

## SSH ControlMaster — `ExecutionContext::ssh`

```rust
ssh -MNf
    -o ControlMaster=yes
    -o ControlPersist=yes
    -o ControlPath=/tmp/buddy-ssh-<hash>.sock
    user@host
```

**What this does:**

- `-MNf`: Open a master connection, don't run a remote command, and fork into
  the background.
- `ControlMaster=yes`: This process is the master; subsequent SSH calls will
  mux through it.
- `ControlPersist=yes`: Keep the master alive even after all client sessions
  disconnect.
- `ControlPath`: The Unix domain socket file that serves as the mux point.

The control path is derived from `target + process_id + timestamp` hashed
with `DefaultHasher` to avoid collisions between multiple agent instances
targeting the same host.

All subsequent SSH commands use `-S <control_path> -o ControlMaster=no` to
multiplex through the same connection without opening new TCP sessions.

**Cleanup:** `SshContext` implements `Drop`. When the `ExecutionContext` is
dropped (when the agent exits), it runs:

```
ssh -S <control_path> -O exit <host>
```

This cleanly closes the ControlMaster and removes the socket file. File
removal is attempted even if the control command fails.
This lifecycle is regression-tested (`ssh_context_drop_triggers_control_cleanup`)
so shutdown cleanup remains covered even if the SSH execution path is refactored.

---

## tmux Session Setup

After the ControlMaster is established, the agent checks whether `tmux` is
available on the remote host:

```bash
command -v tmux >/dev/null 2>&1
```

If tmux is present, it creates (or reattaches to) a named session:

```bash
tmux has-session -t <session> 2>/dev/null || tmux new-session -d -s <session> -n buddy-shared
```

**Default session name:** Derived from a 4-hex-digit hash of the target
string: `buddy-a3f1`. This is deterministic per host, so restarting the agent
reconnects to the same session.

**Custom session name:** Pass `--tmux my-session` to override.

If `--tmux` is explicitly passed but tmux is not installed, the agent errors
rather than silently falling back to direct SSH.

---

## The Shared Pane — `ensure_tmux_pane`

The agent works in a fixed window named `buddy-shared` inside the tmux session.
That window has a single shared pane by default; operators can add additional
panes/windows manually if desired.

```bash
# Ensure the window exists
if ! tmux list-windows -t "$SESSION" -F '#{window_name}' | grep -Fx -- "$WINDOW"; then
  tmux new-window -d -t "$SESSION" -n "$WINDOW"
fi

# Get the first pane id
PANE="$(tmux list-panes -t "$SESSION:$WINDOW" -F '#{pane_id}' | head -n1)"
```

The pane ID (e.g. `%3`) is stored and reused for all subsequent commands. If
the pane disappears (e.g. the user closes it), `ensure_tmux_pane` recreates it
on the next command.

---

## The Prompt Marker System — `ensure_tmux_prompt_setup`

This is the most nuanced part of the design. The agent needs to reliably know
when a command has finished executing in the shared pane, and what its exit
code was. It does this by modifying the remote shell's prompt to embed a
structured marker.

### The Marker Format

After prompt setup, every shell prompt in the shared pane looks like:

```
[buddy 7: 0] user@host:~$
```

Breaking it down:

| Part | Description |
|------|-------------|
| `[buddy` | Fixed prefix for detection |
| `7` | Command ID — increments on every `PROMPT_COMMAND` invocation |
| `0` | Exit code of the last command |
| `]` | End of marker |
| ` user@host:~$` | The original `$PS1` / `$PROMPT` |

The marker appears before the original prompt string, so the user still sees
their familiar prompt. The agent identifies markers by scanning for the
`[buddy ` prefix.

### Shell Setup Script (v3)

The setup runs once per pane via `tmux send-keys`:

```bash
if [ "${BUDDY_PROMPT_LAYOUT:-}" != "v3" ]; then
  BUDDY_PROMPT_LAYOUT=v3
  BUDDY_CMD_SEQ=${BUDDY_CMD_SEQ:-0}

  __buddy_next_id() { BUDDY_CMD_SEQ=$((BUDDY_CMD_SEQ + 1)); BUDDY_CMD_ID=$BUDDY_CMD_SEQ; }
  __buddy_prompt_id() { printf '%s' "${BUDDY_CMD_ID:-0}"; }

  # bash
  if [ -n "${BASH_VERSION:-}" ]; then
    BUDDY_BASE_PS1=${BUDDY_BASE_PS1:-$PS1}
    __buddy_precmd() { __buddy_next_id; }
    PROMPT_COMMAND="__buddy_precmd${PROMPT_COMMAND:+;${PROMPT_COMMAND}}"
    PS1='[buddy $(__buddy_prompt_id): \?] '"$BUDDY_BASE_PS1"

  # zsh
  elif [ -n "${ZSH_VERSION:-}" ]; then
    BUDDY_BASE_PROMPT=${BUDDY_BASE_PROMPT:-$PROMPT}
    __buddy_precmd() { __buddy_next_id; }
    precmd_functions=(__buddy_precmd $precmd_functions)
    setopt PROMPT_SUBST
    PROMPT='[buddy $(__buddy_prompt_id): %?] '"$BUDDY_BASE_PROMPT"

  # POSIX sh fallback
  else
    PS1='[buddy $(__buddy_next_id): $?] '"$BUDDY_BASE_PS1"
  fi
fi
```

**Key properties:**

- **Idempotent**: The `BUDDY_PROMPT_LAYOUT=v3` guard prevents double-installation
  if the command is run more than once.
- **Non-destructive**: The original `$PS1`/`$PROMPT` is saved in
  `BUDDY_BASE_PS1` / `BUDDY_BASE_PROMPT` and appended after the marker.
- **Shell-portable**: Works for bash, zsh, and POSIX sh.
- **Exit code accuracy**: The exit code (`\?` for bash, `%?` for zsh) is
  captured in `$PS1` at the moment the prompt renders, after `__buddy_next_id`
  has run, so `$?` has not been clobbered.

---

## Running Commands — `run_ssh_tmux_process`

When `run_shell` is called on an SSH+tmux target, execution follows this
sequence:

### 1. Capture the baseline

```
tmux capture-pane -p -J -S - -E - -t <pane_id>
```

This reads the full scrollback history (`-S -` from the beginning, `-E -` to
the end). The agent finds the **latest prompt marker** in this output and
records its command ID as `start_command_id`.

### 2. Inject the command

```
tmux send-keys -l -t <pane_id> '<command>'
tmux send-keys -t <pane_id> Enter
```

`-l` sends literal text (no key binding interpretation). The Enter press
causes the shell to execute the command.

### 3. Poll for completion

The agent polls the pane in a tight loop (50 ms interval):

```rust
loop {
    let capture = capture_tmux_pane(...).await?;
    if let Some(result) = parse_tmux_capture_output(&capture, start_command_id, command) {
        return result;
    }
    sleep(Duration::from_millis(50)).await;
}
```

On each poll, it scans the scrollback for a marker whose command ID is
`start_command_id + 1`. That marker's appearance means the command has
finished and the next prompt has rendered.

### 4. Extract output

Once the completion marker is found:

```rust
// lines between start marker and completion marker
let output_lines = &lines[start_idx + 1 .. end_idx];
```

The first line is checked for the echoed command text (terminals typically
echo input) and dropped if it matches. Leading and trailing blank lines are
also trimmed.

The exit code is read directly from the completion marker's `exit_code` field.

### Full flow diagram

```
baseline capture:
  ...
  [buddy 6: 0] user@host:~$         ← start_command_id = 6
                                     ← cursor here, ready

inject: "du -sh /var" + Enter

  poll #1:
  [buddy 6: 0] user@host:~$
  du -sh /var                        ← echoed command

  poll #2:
  [buddy 6: 0] user@host:~$
  du -sh /var
  512M    /var
  [buddy 7: 0] user@host:~$         ← completion_command_id = 7, exit_code = 0

  ✓ extract lines between idx(6) and idx(7):
    ["du -sh /var", "512M    /var"]
    drop echo: ["512M    /var"]

return ExecOutput {
  exit_code: 0,
  stdout: "512M    /var",
  stderr: "",
}
```

---

## Handling stdin — Staged Input Files

Commands that require stdin (e.g., `write_file` is implemented as
`cat > /path` with the content piped) cannot use `tmux send-keys` for
the data, because send-keys is not designed for binary or large inputs.

The agent stages stdin in a temporary file on the remote host via a direct SSH
raw command (not through tmux):

```bash
# via raw SSH (not tmux):
mkdir -p /tmp/buddy-tmux-<token>
cat > /tmp/buddy-tmux-<token>/stdin
```

The command is then modified to redirect from the staged file:

```
original command: cat > /target/file
modified command: cat > /target/file < /tmp/buddy-tmux-<token>/stdin
```

The tmpdir is cleaned up after command completion.

---

## Fire-and-Forget — `wait=false`

When `run_shell` is called with `wait=false`, the command is injected into the
tmux pane but the agent returns immediately without waiting for a completion
marker:

```rust
send_tmux_line(target, control_path, pane_id, command).await?;
return Ok(ExecOutput {
    exit_code: 0,
    stdout: "command dispatched to tmux pane %3; still running in background. \
             Use capture-pane (optionally with delay) to poll output.",
    ...
});
```

The model is then expected to use `capture-pane` to poll progress and
`send-keys` to interact with interactive programs.

This is particularly useful for:

- Long builds (`npm run build`, `cargo build`)
- Servers (`python -m http.server`, `uvicorn app:app`)
- Interactive programs that need navigation (`htop`, `vim`)

---

## Direct SSH Fallback (no tmux)

If the remote host does not have tmux installed (and `--tmux` was not
explicitly requested), the agent falls back to raw SSH for each command:

```
ssh -T -S <control_path> -o ControlMaster=no <host> '<command>'
```

In this mode:
- `wait=false` is not available (returns an error).
- `capture-pane` and `send-keys` are not available.
- No shared interactive session; the operator cannot observe commands.
- Each command is a new SSH channel multiplexed over the existing ControlMaster.

---

## Container Mode

`--container <name>` uses the local Docker or Podman daemon instead of SSH:

```bash
# auto-detected
docker exec <container> sh -lc '<command>'
# or
podman exec --interactive <container> sh -lc '<command>'
```

The engine is probed at startup by running `docker --version` and
`podman --version`. Docker's CLI with Podman's backend (podman-docker) is
detected by checking the version output for "podman".

With plain `--container`, commands run directly via `exec` and there is no
tmux backend.

With `--container ... --tmux [session]`, buddy creates/reuses a tmux session
inside the container and uses the same shared-pane prompt-marker protocol as
SSH+tmux. In this mode, `wait=false`, `capture-pane`, and `send-keys` are
available for container execution.

---

## Prompt Marker Parsing — `parse_prompt_marker`

```rust
fn parse_prompt_marker(line: &str) -> Option<PromptMarker> {
    parse_prompt_marker_with_prefix(line, "[buddy ")
        .or_else(|| parse_prompt_marker_with_prefix(line, "[agent "))
}
```

The parser is deliberately tolerant:
- It searches for the prefix anywhere in the line, so the original `$PS1`
  content before or after the marker doesn't interfere.
- It handles extra whitespace around the numbers.
- It accepts both `[buddy ...]` and legacy `[agent ...]` markers.
- It returns `None` rather than panicking on malformed lines.

`latest_prompt_marker` scans lines in reverse to find the most recent marker,
which handles the case where a previous prompt is still visible in the
scrollback.

---

## Edge Cases

### Marker scrolled out of history

If the scrollback history is very small, the start marker might scroll out of
view before the completion marker appears. The parser detects this:

```rust
if let Some(latest) = latest_prompt_marker(capture) {
    if latest.command_id > start_command_id {
        return Some(Err(ToolError::ExecutionFailed(format!(
            "tmux prompt marker {} is no longer visible in capture history",
            start_command_id
        ))));
    }
}
```

### Non-sequential command IDs

If the completion marker's command ID is not exactly `start_command_id + 1`,
the agent errors rather than silently producing wrong output. This catches
cases where the shell's `PROMPT_COMMAND` was modified externally.

### Alternate screen capture

Some programs (Vim, htop, `less`) switch to the terminal alternate screen.
`capture-pane` with `include_alternate_screen=true` captures that buffer.
If no alternate screen is active, the tool falls back to the main pane and
appends a notice so the model knows what happened.

### Timeout

Both `wait=true` with a duration string and the global `/timeout` command can
bound how long the agent waits. If the tmux poll loop exceeds the deadline,
the agent returns an error. The model can then use `capture-pane` to inspect
the partial state and `send-keys C-c` to cancel.

---

## Summary of Remote Capabilities by Mode

| Capability | Local | Local+tmux | SSH+tmux | SSH (no tmux) | Container | Container+tmux |
|-----------|-------|------------|----------|---------------|-----------|----------------|
| `run_shell` (wait) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `run_shell` (wait=false) | ✗ | ✓ | ✓ | ✗ | ✗ | ✓ |
| `read_file` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `write_file` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `capture-pane` | only in attached tmux | ✓ | ✓ | ✗ | ✗ | ✓ |
| `send-keys` | only in attached tmux | ✓ | ✓ | ✗ | ✗ | ✓ |
| Operator visibility | – | ✓ (attach) | ✓ (attach) | ✗ | ✗ | ✓ (attach) |
| Persistent session | – | ✓ | ✓ | ✗ | ✗ | ✓ |
| Exit code accuracy | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
