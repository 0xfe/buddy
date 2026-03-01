# The Tools System

Tools are the mechanism through which the agent interacts with the outside
world. The model decides which tool to call and with what arguments; the agent
executes it and feeds the result back into the conversation. This document
covers the tool abstraction, the registry, all built-in tools, the shared
execution backend, and how to add new tools.

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                     Agent Loop                       │
│                                                      │
│  model response                                      │
│    └─ tool_calls: [{name, args}, ...]                │
│              │                                       │
│              ▼                                       │
│        ToolRegistry::execute(name, args)             │
│              │                                       │
│              ▼                                       │
│  impl Tool::execute(args) ──► ExecutionContext       │
│                                    │                 │
│                          ┌─────────┴──────────┐     │
│                          │ Local │ Container   │     │
│                          │       │ SSH+tmux    │     │
│                          └─────────────────────┘     │
└──────────────────────────────────────────────────────┘
```

---

## The `Tool` Trait — `src/tools/mod.rs`

Every tool implements three methods:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name the model uses when calling this tool.
    fn name(&self) -> &'static str;

    /// OpenAI-compatible function definition sent to the API in each request.
    fn definition(&self) -> ToolDefinition;

    /// Execute with the JSON argument string the model provided.
    /// Returns a text result that is pushed back into the conversation.
    async fn execute(&self, arguments: &str, context: &ToolContext) -> Result<String, ToolError>;
}
```

**Key design choices:**

- Arguments arrive as a raw JSON string (OpenAI's double-encoding). Tools
  deserialise with `serde_json::from_str` and return a `ToolError::InvalidArguments`
  on parse failure.
- `ToolContext` provides an optional stream sink for incremental events
  (`started`, `stdout`, `stderr`, `info`, `completed`) consumed by the runtime UI.
- The return type is always `String`. If execution fails and the error is not
  fatal, formatting the error as a string and returning it lets the model read
  the failure and decide what to do next. The agent loop formats hard errors as
  `"Tool error: {e}"` and continues the conversation.
- Tools are `Send + Sync` so they can be shared across async tasks.

---

## `ToolRegistry` — `src/tools/mod.rs`

The registry is a simple `Vec<Box<dyn Tool>>` with three operations:

```rust
registry.register(tool);           // add a tool
registry.definitions();            // collect ToolDefinition for the API request
registry.execute(name, args).await // dispatch by name
registry.execute_with_context(name, args, &ctx).await // dispatch with stream sink
```

If `definitions()` is called with no tools registered, the agent omits the
`tools` field from the API request entirely (providers reject an empty array).

---

## Built-in Tools

Twelve tools ship with the agent. Each is conditionally registered based on
config flags (`[tools].shell_enabled`, `fetch_enabled`, etc.).

---

### 1. `run_shell` — `src/tools/shell.rs`

Run a shell command and capture its output.

**Arguments:**

```json
{
  "command": "du -sh /var",
  "session": "ops",
  "pane": "worker",
  "risk": "low",
  "mutation": false,
  "privesc": false,
  "why": "Inspect disk usage before cleanup",
  "wait": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | required | Shell command, executed via `sh -c` |
| `risk` | string | required | Estimated command risk: `low`, `medium`, `high` |
| `mutation` | bool | required | Whether command mutates system state |
| `privesc` | bool | required | Whether command uses privilege escalation |
| `why` | string | required | Short reason and risk justification |
| `session` | string | shared default session | Optional managed tmux session selector |
| `pane` | string | `shared` | Optional managed tmux pane selector |
| `wait` | bool \| string \| int | `true` | Waiting behaviour (see below) |

**Wait modes:**

| Value | Behaviour |
|-------|-----------|
| `true` (default) | Block until command exits |
| `false` | Fire and forget; requires a tmux-backed target |
| `"30s"`, `"10m"`, `"1h"` | Block up to a timeout, then error |
| `500` (integer) | Block up to N seconds |

**Output format:**

All tools return a JSON envelope:

```json
{
  "harness_timestamp": { "source": "harness", "unix_millis": 1772283708794 },
  "result": {
    "exit_code": 0,
    "stdout": "512M\t/var",
    "stderr": ""
  }
}
```

`run_shell` truncates both stdout and stderr to 4000 characters.

**Approval flow:**

When `[tools].shell_confirm = true` in config, the tool pauses before running
and waits for user approval. In interactive mode this goes through the REPL's
inline approval prompt; in one-shot mode it falls back to a simple stdin
prompt (`Run: <cmd> [y/N]`). Denied commands return
`{"result":"Command execution denied by user.", ...}`.

**Spinner:** `run_shell` manages its own spinner so that it can appear after
the approval prompt, not before.

---

### 2. `read_file` — `src/tools/files.rs`

Read the contents of a file.

**Arguments:**

```json
{ "path": "/etc/hostname" }
```

Returns the file's text content, truncated to 8000 characters with
`...[truncated]` appended if it exceeds the limit.

In container or SSH mode the read is performed via `cat -- <path>` on the
remote target.

---

### 3. `write_file` — `src/tools/files.rs`

Write content to a file, creating it if needed and overwriting if it exists.

**Arguments:**

```json
{
  "path": "/tmp/output.txt",
  "content": "hello world\n"
}
```

Returns `"Wrote N bytes to /path"` on success.

In container or SSH mode, the content is piped via stdin to `cat > <path>`.

---

### 4. `fetch_url` — `src/tools/fetch.rs`

Perform an HTTP GET and return the response body.

**Arguments:**

```json
{ "url": "https://example.com/api/data" }
```

Uses `reqwest` under the hood. No authentication is supported. Response body
is truncated to 8000 characters.

Useful for downloading configuration files, checking APIs, or fetching
documentation pages.

---

### 5. `web_search` — `src/tools/search.rs`

Search the web via DuckDuckGo's HTML endpoint. No API key is required.

**Arguments:**

```json
{ "query": "rust tokio tutorial" }
```

Returns up to 8 results, each with title, URL, and snippet. The HTML is
parsed with `scraper` selectors, with a fallback extractor when the primary
result container layout changes.

**Example output:**

```
1. Tokio - An asynchronous Rust runtime
   https://tokio.rs
   Tokio is an event-driven, non-blocking I/O platform...

2. Tutorial | Tokio - An asynchronous Rust runtime
   https://tokio.rs/tokio/tutorial
   ...
```

---

### 6. `capture-pane` — `src/tools/capture_pane.rs`

Capture a snapshot of a tmux pane's visible output. This tool is only
registered when a tmux pane is available (either locally via `$TMUX_PANE`, or
in a local `--tmux` session, in a `--container ... --tmux` session, or on an
SSH target with a tmux session).

**Arguments:**

```json
{
  "delay": "2s",
  "start": "-",
  "end": "-",
  "join_wrapped_lines": true
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `target` | active pane | Legacy raw tmux pane/session target (`-t` syntax) |
| `session` | shared default session | Optional managed tmux session selector |
| `pane` | `shared` | Optional managed tmux pane selector |
| `start` | tmux default | Start line (`-S`); `"-"` = beginning of history |
| `end` | tmux default | End line (`-E`); `"-"` = end of visible area |
| `join_wrapped_lines` | `true` | tmux `-J` flag — join soft-wrapped lines |
| `preserve_trailing_spaces` | `false` | tmux `-N` flag |
| `include_escape_sequences` | `false` | tmux `-e` flag (ANSI codes) |
| `escape_non_printable` | `false` | tmux `-C` flag (octal encoding) |
| `include_alternate_screen` | `false` | tmux `-a` flag |
| `delay` | none | Wait before capturing (for polling) |

Output is truncated to 8000 characters, keeping the **tail** (most recent
content), with `[truncated N chars from start]` prepended if clipped.

If `include_alternate_screen` is requested but no alternate screen is active,
the tool silently falls back to the main pane and appends a notice.

**Common pattern — polling a background command:**

```
run_shell({"command": "npm run build", "wait": false})
  → "command dispatched to tmux pane %1"

capture-pane({"delay": "5s"})
  → "[...build output so far...]"
```

---

### 7. `send-keys` — `src/tools/send_keys.rs`

Inject keystrokes into a tmux pane. Only available with a tmux backend.

**Arguments:**

```json
{
  "keys": ["C-c"],
  "literal_text": "yes\n",
  "enter": true,
  "delay": "500ms",
  "risk": "low",
  "mutation": false,
  "privesc": false,
  "why": "Send Ctrl-C to cancel a hung command"
}
```

| Field | Description |
|-------|-------------|
| `target` | Legacy raw tmux pane/session target; defaults to active pane |
| `session` | Optional managed tmux session selector |
| `pane` | Optional managed tmux pane selector |
| `keys` | tmux key names: `"C-c"`, `"C-z"`, `"Enter"`, `"Up"`, `"Down"`, etc. |
| `literal_text` | Literal text to type (uses `tmux send-keys -l`) |
| `enter` | Press Enter after other keys |
| `delay` | Wait before sending |
| `risk` | Required risk label: `low`, `medium`, `high` |
| `mutation` | Required mutation flag |
| `privesc` | Required privilege-escalation flag |
| `why` | Required short justification |

Keys are sent in order: `literal_text` first, then named `keys`, then `Enter`
if requested.

**Common patterns:**

```json
// Cancel a stuck command
{"keys": ["C-c"]}

// Respond to an interactive prompt
{"literal_text": "yes", "enter": true}

// Navigate a menu
{"keys": ["Down", "Down", "Enter"]}
```

---

### 8. `tmux-create-session` — `src/tools/tmux_manage.rs`

Create or reuse a buddy-managed tmux session and ensure its shared pane is
ready.

Required fields: `session`, `risk`, `mutation`, `privesc`, `why`.

Session names are canonicalized to the buddy owner prefix
(`buddy-<agent.name>-...`) and are bounded by `[tmux].max_sessions`.

---

### 9. `tmux-kill-session` — `src/tools/tmux_manage.rs`

Kill one buddy-managed tmux session.

- Cannot kill the default shared session.
- Fails for unmanaged sessions.

Required fields: `session`, `risk`, `mutation`, `privesc`, `why`.

---

### 10. `tmux-create-pane` — `src/tools/tmux_manage.rs`

Create or reuse a buddy-managed pane in a managed session.

Required fields: `pane`, `risk`, `mutation`, `privesc`, `why`.
Optional: `session`.

Pane names are canonicalized to buddy-managed names (except reserved
`shared`) and are bounded by `[tmux].max_panes`.

---

### 11. `tmux-kill-pane` — `src/tools/tmux_manage.rs`

Kill one buddy-managed pane in a managed session.

- Default shared pane is protected from deletion.
- Fails for unmanaged panes.

Required fields: `pane`, `risk`, `mutation`, `privesc`, `why`.
Optional: `session`.

---

### 12. `time` — `src/tools/time.rs`

Return the current wall-clock time snapshot from the harness.

**Arguments:**

```json
{}
```

`result` includes common UTC/epoch fields (for example `unix_millis`,
`iso_8601_utc`, and `rfc_2822_utc`) wrapped in the standard envelope with
`harness_timestamp`.

This tool reports the harness wall-clock time, not the remote shell's time.
It is useful when the model needs to timestamp actions or calculate durations
without shelling out.

---

## The Execution Backend — `src/tools/execution/mod.rs`

`run_shell`, `read_file`, `write_file`, `capture-pane`, and `send-keys` all
delegate to an `ExecutionContext` rather than running commands directly. This
single abstraction supports multiple execution backends transparently.

### Backends

`ExecutionContext` now stores an internal trait object:

```rust
Arc<dyn ExecutionBackendOps>
```

Concrete backend implementations currently include:

- `LocalBackend`
- `LocalTmuxContext`
- `ContainerContext` (docker/podman exec)
- `ContainerTmuxContext` (container exec + tmux)
- `SshContext` (SSH ControlMaster + tmux)

Shared command-oriented behavior is factored through a `CommandBackend` trait so
`read_file`/`write_file` and shell command execution paths are not duplicated
per backend.

All tools accept an `ExecutionContext` at construction time. The REPL
constructs the context based on CLI flags (`--container`, `--ssh`, `--tmux`) and passes
it to every tool.

### Local Backend

Commands run via `tokio::process::Command` directly on the host. For
`wait=false` shell commands, the command is dispatched to the current tmux
pane via `tmux send-keys`.

File reads use `tokio::fs::read_to_string`; writes use `tokio::fs::write`.

### Container Backend

Commands run via `docker exec` or `podman exec`. The engine is auto-detected
at startup by probing `docker --version` and `podman --version`.

```
docker exec <container> sh -lc '<command>'
```

For commands that need stdin (e.g., `write_file`), the interactive flag is
added: `-i` for Docker, `--interactive` for Podman.

When `--tmux` is also set, the container backend becomes tmux-backed:
- a session is created/reused inside the container,
- commands are dispatched with `tmux send-keys`,
- `wait=false`, `capture-pane`, and `send-keys` become available.

### SSH+Tmux Backend

The most sophisticated mode. See [Remote Execution](./remote-execution.md)
for the full design. In brief:

- An SSH ControlMaster socket is established at startup and reused for all
  subsequent commands.
- Commands are executed inside a persistent tmux pane rather than fresh SSH
  processes, so the operator can attach and observe what the agent is doing.
- Output is collected via a prompt-marker system that lets the agent reliably
  extract the output of each command from the tmux scrollback buffer.

---

## Adding a Custom Tool

### Step 1 — Create the tool file

```rust
// src/tools/my_tool.rs

use async_trait::async_trait;
use serde::Deserialize;
use super::{Tool, ToolContext};
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

pub struct MyTool;

#[derive(Deserialize)]
struct Args {
    message: String,
}

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &'static str {
        "my_tool"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Does something useful.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "The message to process"
                        }
                    },
                    "required": ["message"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        Ok(format!("processed: {}", args.message))
    }
}
```

### Step 2 — Export from the tools module

```rust
// src/tools/mod.rs
pub mod my_tool;
```

### Step 3 — Register in `src/app/entry.rs`

```rust
// src/app/entry.rs (inside build_tools)
registry.register(MyTool);
```

### Step 4 — Optionally gate behind a config flag

Add `my_tool_enabled: bool` to `ToolsConfig` in `src/config/types.rs` and wrap the
registration:

```rust
if config.tools.my_tool_enabled {
    registry.register(MyTool);
}
```

---

## Output Truncation Summary

Tool output is truncated before being stored in conversation history. Keeping
results small prevents accidental context exhaustion.

| Tool | Limit | Truncation style |
|------|-------|-----------------|
| `run_shell` stdout | 4000 chars | head (appends `...[truncated]`) |
| `run_shell` stderr | 4000 chars | head |
| `read_file` | 8000 chars | head |
| `fetch_url` | 8000 chars | head |
| `capture-pane` | 8000 chars | tail (prepends `[truncated N chars from start]`) |

`capture-pane` truncates from the **tail** instead of the head because the
most recent screen content is more relevant than old scrollback.

---

## Error Propagation

Tools can return two kinds of errors:

```rust
enum ToolError {
    InvalidArguments(String),  // bad JSON from model
    ExecutionFailed(String),   // runtime failure
}
```

Neither variant aborts the agent loop. The agent formats errors as
`"Tool error: {e}"` and pushes the string as a `tool_result` message. The
model can then read the error and decide whether to retry, ask for help, or
give up. This makes the system resilient to transient tool failures.
