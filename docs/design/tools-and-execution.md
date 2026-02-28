# Tools and Execution

This document captures tool contracts, execution backends, and tmux operational details.

For the high-level overview, see [DESIGN.md](../../DESIGN.md).

## Tool Contract

All tools implement `Tool`:

- `name() -> &'static str`
- `definition() -> ToolDefinition`
- `execute(arguments, context) -> Result<String, ToolError>`

`ToolRegistry` provides:

- tool registration
- schema collection for model requests
- tool dispatch by name

`ToolContext` optionally carries a stream channel for incremental runtime events:

- `Started`
- `StdoutChunk`
- `StderrChunk`
- `Info`
- `Completed`

## Standard Tool Result Envelope

Every built-in tool returns JSON in this shape:

```json
{
  "harness_timestamp": {
    "source": "harness",
    "unix_millis": 1730000000000
  },
  "result": "...tool-specific payload..."
}
```

Purpose:

- stable response contract across tools
- explicit harness-time metadata for downstream reasoning

## Built-in Tools

### `run_shell`

- Runs commands via selected execution backend.
- Required metadata fields:
  - `risk` (`low|medium|high`)
  - `mutation` (`bool`)
  - `privesc` (`bool`)
  - `why` (`non-empty string`)
- Optional wait behavior:
  - `true`: wait for completion
  - `false`: dispatch and return immediately (tmux-backed contexts)
  - duration string / integer seconds: bounded wait with timeout
- Enforces `tools.shell_denylist` patterns.
- Optional confirmations (`tools.shell_confirm`), mediated by runtime broker in interactive mode.
- Output truncation: 4K for stdout/stderr payload text.

### `read_file`

- Reads text through active backend.
- Output truncation: 8K.

### `write_file`

- Writes text through active backend.
- Path policy:
  - optional allowlist roots (`tools.files_allowed_paths`)
  - sensitive root deny policy unless explicitly allowlisted

### `fetch_url`

- HTTP(S) GET via timeout-bound reqwest client.
- URL policy:
  - blocks localhost aliases
  - blocks private/link-local/multicast IP targets by default
  - checks domain allowlist/denylist
  - resolves hostnames and blocks private-local resolutions unless explicitly allowlisted
- Optional confirmation (`tools.fetch_confirm`).
- Output truncation: 8K response body.

### `web_search`

- Queries DuckDuckGo HTML endpoint.
- Parses title/url/snippet rows via CSS selectors.
- Returns parser-break diagnostics when the page parses but results are not extractable.
- Result cap: 8 entries.

### `capture-pane`

- Captures tmux pane output with options close to native `tmux capture-pane` flags:
  - `target`, `start`, `end`
  - `join_wrapped_lines`, `preserve_trailing_spaces`
  - `include_escape_sequences`, `escape_non_printable`
  - `include_alternate_screen`
  - optional delay
- Output truncates from the start (keeps most recent tail) to preserve fresh state.
- Alternate-screen fallback retries capture without `-a` when no alternate screen is active.

### `send-keys`

- Injects tmux key events/literal text.
- Inputs:
  - `keys`
  - `literal_text`
  - `enter`
  - optional delay
- Required metadata fields:
  - `risk`, `mutation`, `privesc`, `why`
- Requires at least one actionable input (`keys`, `literal_text`, or `enter=true`).

### `time`

- Returns harness clock snapshot with:
  - unix seconds/millis/nanos
  - ISO-8601 UTC (seconds and millis)
  - RFC 2822 UTC
  - date/time UTC fragments

## Execution Context Abstraction

`ExecutionContext` selects one backend implementation implementing `ExecutionBackendOps`.

Common operations:

- `run_shell_command`
- `read_file`
- `write_file`
- `capture_pane`
- `send_keys`
- `summary`
- optional tmux attach metadata

## Backend Matrix

### Local backend (`ExecutionContext::local`)

- Shell: local `sh` process.
- Files: direct `tokio::fs`.
- `capture-pane` / `send-keys`: only if currently inside an active tmux pane.
- `wait=false` in `run_shell`: requires active tmux pane.

### Local tmux backend (`ExecutionContext::local_tmux`)

- Creates/reuses managed local tmux session and shared pane.
- All shell and file operations run through tmux-aware command backend.
- `capture-pane` and `send-keys` always available.
- Startup rejects launching Buddy from the managed shared pane.

### Container backend (`ExecutionContext::container`)

- Executes commands via container engine (`docker`/`podman`) without tmux mediation.
- `capture-pane` and `send-keys` unavailable.
- `wait=false` in `run_shell` rejected.

### Container tmux backend (`ExecutionContext::container_tmux`)

- Requires tmux available inside container.
- Manages shared session/pane in container tmux namespace.
- Supports `capture-pane`, `send-keys`, and `wait=false` shell dispatch.

### SSH backend (`ExecutionContext::ssh`)

- Uses persistent SSH control master socket.
- If remote tmux is available, establishes managed tmux session/pane.
- Without tmux:
  - shell/file operations still work
  - `capture-pane`, `send-keys`, and `wait=false` are unavailable

## Managed tmux Lifecycle

Buddy standardizes managed tmux behavior across local/SSH/container:

- session name default: `buddy-<sanitized agent.name>`
- managed window name: `shared`
- managed pane title: `shared`
- pane ensure script:
  - create session/window if absent
  - prefer existing titled pane
  - create split pane when needed
  - return pane id + created flag

Prompt initialization installs command markers used by run parser logic.

## Tmux Command Completion Parsing

For tmux-backed `run_shell wait=true|timeout`:

- capture baseline marker before dispatch
- send command text + Enter via `send-keys`
- poll pane capture until next prompt marker appears
- parse exit code from marker
- parse output between start/end markers
- strip echoed command line when present

Timeout path:

- bounded waits return explicit timeout errors with formatted duration.

No-wait path:

- command is dispatched
- tool returns immediate polling guidance to use `capture-pane`

## Tool Availability Gating at Startup

Tool registration in app wiring is conditional:

- `run_shell` when `tools.shell_enabled`
- `fetch_url` when `tools.fetch_enabled`
- `read_file` + `write_file` when `tools.files_enabled`
- `web_search` when `tools.search_enabled`
- `capture-pane` + `send-keys` only when execution context reports capture support
- `time` always registered

## Safety and Policy Notes

- Shell command denylist blocks known dangerous patterns.
- File writes are blocked for sensitive roots by default.
- Fetch URL policy mitigates SSRF classes by default.
- Confirmation flows can be brokered through runtime actor for consistent interactive approval UX.
