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
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

**Key design choices:**

- Arguments arrive as a raw JSON string (OpenAI's double-encoding). Tools
  deserialise with `serde_json::from_str` and return a `ToolError::InvalidArguments`
  on parse failure.
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
```

If `definitions()` is called with no tools registered, the agent omits the
`tools` field from the API request entirely (providers reject an empty array).

---

## Built-in Tools

Eight tools ship with the agent. Each is conditionally registered based on
config flags (`[tools].shell_enabled`, `fetch_enabled`, etc.).

---

### 1. `run_shell` — `src/tools/shell.rs`

Run a shell command and capture its output.

**Arguments:**

```json
{
  "command": "du -sh /var",
  "wait": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | required | Shell command, executed via `sh -c` |
| `wait` | bool \| string \| int | `true` | Waiting behaviour (see below) |

**Wait modes:**

| Value | Behaviour |
|-------|-----------|
| `true` (default) | Block until command exits |
| `false` | Fire and forget; requires a tmux-backed target |
| `"30s"`, `"10m"`, `"1h"` | Block up to a timeout, then error |
| `500` (integer) | Block up to N seconds |

**Output format:**

```
exit code: 0
stdout:
512M	/var
stderr:

```

Both stdout and stderr are truncated to 4000 characters each.

**Approval flow:**

When `[tools].shell_confirm = true` in config, the tool pauses before running
and waits for user approval. In interactive mode this goes through the REPL's
inline approval prompt; in one-shot mode it falls back to a simple stdin
prompt (`Run: <cmd> [y/N]`). Denied commands return
`"Command execution denied by user."`.

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
parsed with simple string operations (no external HTML parsing library).

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
| `target` | active pane | tmux pane/session target (`-t` syntax) |
| `start` | tmux default | Start line (`-S`); `"-"` = beginning of history |
| `end` | tmux default | End line (`-E`); `"-"` = end of visible area |
| `join_wrapped_lines` | `true` | tmux `-J` flag — join soft-wrapped lines |
| `preserve_trailing_spaces` | `false` | tmux `-N` flag |
| `include_escape_sequences` | `false` | tmux `-e` flag (ANSI codes) |
| `escape_non_printable` | `false` | tmux `-C` flag (octal encoding) |
| `include_alternate_screen` | `false` | tmux `-a` flag |
| `delay` / `delay_ms` | none | Wait before capturing (for polling) |

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
  "delay_ms": 500
}
```

| Field | Description |
|-------|-------------|
| `target` | tmux pane/session target; defaults to active pane |
| `keys` | tmux key names: `"C-c"`, `"C-z"`, `"Enter"`, `"Up"`, `"Down"`, etc. |
| `literal_text` | Literal text to type (uses `tmux send-keys -l`) |
| `enter` | Press Enter after other keys |
| `delay` / `delay_ms` | Wait before sending |

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

### 8. `time` — `src/tools/time.rs`

Return the current wall-clock time in various formats.

**Arguments:**

```json
{ "format": "iso" }
```

| Format | Example output |
|--------|---------------|
| `epoch` (default) | `1708900000` |
| `epoch_ms` | `1708900000123` |
| `epoch_ns` | `1708900000123456789` |
| `iso` | `2025-02-26T12:34:56.789Z` |
| `rfc2822` | `Wed, 26 Feb 2025 12:34:56 +0000` |
| `date` | `2025-02-26` |
| `time` | `12:34:56` |

This tool reports the harness wall-clock time, not the remote shell's time.
It is useful when the model needs to timestamp actions or calculate durations
without shelling out.

---

## The Execution Backend — `src/tools/execution.rs`

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
use super::Tool;
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

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
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

### Step 3 — Register in main.rs

```rust
// src/main.rs (in the tool registration block)
registry.register(MyTool);
```

### Step 4 — Optionally gate behind a config flag

Add `my_tool_enabled: bool` to `ToolsConfig` in `src/config.rs` and wrap the
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
