# Design

How the agent works, module by module.

## Architecture overview

The agent is a single Rust crate that produces both a library (`lib.rs`) and a binary (`main.rs`). The binary is a thin CLI wrapper; all logic lives in the library modules.

```
main.rs + cli.rs + tui/  CLI entry point, argument parsing, REPL loop + slash autocomplete
        |
        v
    agent.rs              Core orchestration — the agentic loop
   /    |    \
  v     v     v
api/    tools/  tui/      HTTP client, tool dispatch, terminal output
  |       |
  v       v
types.rs  error.rs          Shared data model and error types
  |
  v
config.rs  tokens.rs        Configuration loading, token tracking
```

Every module has a single responsibility. Dependencies flow downward — `agent.rs` depends on everything below it, but `types.rs` and `error.rs` depend on nothing within the crate.

## Features

- OpenAI-compatible model client with per-profile wire protocol:
  - Chat Completions (`POST /chat/completions`)
  - Responses API (`POST /responses`)
- Works with OpenAI, Ollama, OpenRouter, and similar providers.
- Dual runtime modes:
  - Interactive REPL mode.
  - One-shot mode via `buddy exec <prompt>`.
- CLI subcommands:
  - `buddy` (REPL)
  - `buddy exec <prompt>`
  - `buddy resume <session-id>` / `buddy resume --last`
  - `buddy login [model-profile]`
  - `buddy help`
- Configurable model/API settings with precedence:
  - Environment variable overrides.
  - CLI overrides.
  - Local/global TOML config.
  - Built-in defaults.
  - On startup, `~/.config/buddy/buddy.toml` is auto-created (if missing) from a compiled template (`src/templates/buddy.toml`).
  - Built-in template includes OpenAI `responses` profiles for `gpt-codex` (`gpt-5.3-codex`) and `gpt-spark` (`gpt-5.3-codex-spark`), plus OpenRouter examples for DeepSeek V3.2 and GLM, with `gpt-codex` selected by default.
  - Primary naming uses `BUDDY_*` env vars + `buddy.toml`, with legacy `AGENT_*` and `agent.toml` compatibility fallbacks.
- Built-in tool-calling agent loop:
  - Sends tool definitions to the model.
  - Executes returned tool calls.
  - Feeds tool results back until a final assistant response or max-iteration cap.
- Built-in tools:
  - `run_shell` with optional confirmation flow, output truncation, and configurable wait modes (`true`, `false`, or duration timeout).
  - `read_file` and `write_file` with output safety limits.
  - `fetch_url` HTTP GET tool.
  - `web_search` (DuckDuckGo HTML results parsing).
  - `capture-pane` for tmux pane snapshots (supports common `tmux capture-pane` options plus delayed capture for polling); defaults to tmux screenshot behavior (visible pane content) and gracefully falls back when alternate screen is unavailable.
  - `send-keys` for tmux key injection (for example Ctrl-C/Ctrl-Z/Enter/arrows) to control interactive terminal programs.
  - `time` for harness-recorded wall-clock time in multiple common UTC/epoch formats.
- Multi-target execution for shell/file tools:
  - Local host execution by default, tmux-backed when shell/file tools are enabled.
  - Container execution with `--container`.
  - Remote SSH execution with `--ssh`.
  - Tmux-backed execution is the default for local/container/SSH targets (auto session `buddy-xxxx`), with optional override via `--tmux [session]`.
  - On tmux-backed startup, buddy prints friendly attach instructions (local, SSH, or container) including the resolved session name.
  - Managed tmux setup uses a single shared window (`buddy-shared`) with one pane by default.
  - Local tmux startup refuses to run from the managed `buddy-shared` pane to avoid self-injection loops; run buddy from a different terminal/pane.
  - `capture-pane` is only enabled when tmux pane capture is available for the active execution target.
  - In tmux-backed targets, `run_shell` supports non-blocking dispatch (`wait: false`) and timeout-bound waits (`wait: "10m"` style) for interactive workflows.
- System prompt templating:
  - Full built-in prompt compiled from one template file (`src/templates/system_prompt.template`).
  - Prompt rendered in one code path with runtime placeholders (enabled tools, execution target, optional operator instructions from config).
  - Remote-execution note injected by template parameters when `--ssh` or `--container` is used.
- REPL ergonomics:
  - Prompt format:
    - local: `> `
    - ssh target: `(ssh user@host)> `
  - Slash-command autocomplete and built-in slash commands (`/status`, `/context`, `/ps`, `/kill`, `/timeout`, `/approve`, `/session`, `/model`, `/login`, `/help`, `/quit`, `/exit`, `/q`).
  - Command history navigation (`Up/Down`, `Ctrl-P/N`).
  - Multiline editing with `Alt+Enter`.
  - Common cursor/edit shortcuts (`Ctrl-A/E/B/F/K/U/W`, arrows, home/end, delete/backspace).
  - Background prompt execution with task IDs, `/ps` listing, cooperative `/kill <id>` cancellation, per-task `/timeout <duration> [id]`, and command gating while tasks are active.
  - Session approval policy control via `/approve ask|all|none|<duration>`, including expiring auto-approve windows.
  - Background shell-confirmation handoff: when `run_shell` needs approval, the REPL input is interrupted and approval is rendered in the foreground.
  - One-line approval prompt format (`user@host$ <command> -- approve?`) with colorized actor/command/action segments.
  - Inline liveness line while background tasks run (task runtime/state), rendered above the input prompt.
  - Persistent session IDs under `.buddyx/sessions` with `/session` list + resume/create flows (`/session resume <session-id|last>`, `/session new`) and CLI resume (`buddy resume <session-id>`, `buddy resume --last`), ordered by last use, with legacy `.agentx` auto-reuse when present.
- Terminal UX and observability:
  - Colorized status output.
  - Strict stdout/stderr separation (assistant response on stdout, status/chrome on stderr).
  - Live TTY spinners for model/tool work.
  - Interactive background mode uses REPL-integrated liveness indicators (instead of thread-written spinners) to avoid input corruption.
  - Tool-call/result previews and status sections.
  - Reasoning/thinking trace rendering when providers return reasoning fields, including foreground-rendered forwarding from background tasks.
- Context and token awareness:
  - Usage accounting from API responses when available.
  - Pre-flight token estimation with context-limit warning.
  - Model context-window lookup from shipped `src/templates/models.toml` rules (with fallback heuristic).
- Provider compatibility hardening:
  - Unknown message fields are preserved and round-tripped for providers that require extra metadata during tool-use turns.
  - Assistant messages with tool calls preserve compatible null content behavior.

## Module details

### `types.rs` — Data model

Defines the complete request/response types for the OpenAI Chat Completions API. These types serialize directly to and from JSON using serde, so there's no translation layer between internal representations and wire format.

Key types:

- **`Role`** — enum with `System`, `User`, `Assistant`, `Tool` variants. Serialized as lowercase strings.
- **`Message`** — the core conversation unit. Has a `role`, `content`, optional `tool_calls` (when the assistant wants to invoke tools), and optional `tool_call_id` (when responding with tool results). It also stores unknown provider-specific fields via `#[serde(flatten)]` so they round-trip back to the API unchanged (important for reasoning-capable providers that require metadata like `reasoning_content` across tool-call turns).
- **`ToolCall` / `FunctionCall`** — nested structs representing a model's request to invoke a tool. The `arguments` field is a JSON-encoded string (this is how the OpenAI API works — arguments are double-encoded).
- **`ToolDefinition` / `FunctionDefinition`** — the schema sent in requests so the model knows what tools are available. Parameters use a raw `serde_json::Value` to hold the JSON Schema.
- **`ChatRequest`** — the POST body. Includes model name, messages, optional tool definitions, optional temperature/top_p.
- **`ChatResponse` / `Choice` / `Usage`** — the API response. `usage` is optional because some providers (older Ollama versions) don't include it.

`Message` has convenience constructors — `Message::system()`, `Message::user()`, `Message::tool_result()` — that set the right role and fields without boilerplate.

### `config.rs` — Configuration

Configuration is defined with model profiles and runtime resolution:

- **`ModelConfig`** — profile fields under `[models.<name>]`: `api_base_url`, `api` (`completions|responses`), `auth` (`api-key|login`), `api_key`, `api_key_env`, `api_key_file`, optional `model`, optional `context_limit`
- **`ApiConfig`** — resolved active runtime API settings (`base_url`, resolved `api_key`, concrete `model`, resolved `protocol`, resolved `auth`, active `profile`, optional `context_limit`)
- **`AgentConfig`** — `model` (active profile key), `system_prompt`, `max_iterations`, optional `temperature`/`top_p`
- **`ToolsConfig`** — boolean flags for each built-in tool, plus `shell_confirm`
- **`DisplayConfig`** — `color`, `show_tokens`, `show_tool_calls`

Every field has a default, so a completely empty config file (or no config file at all) produces a working configuration. The `load_config()` function searches for config in this order:

1. Explicit path from `--config` flag (fail if missing)
2. `./buddy.toml` in the working directory
3. legacy `./agent.toml` fallback
4. `$XDG_CONFIG_HOME/buddy/buddy.toml` (or `~/.config/buddy/buddy.toml`)
5. legacy `~/.config/agent/agent.toml` fallback

Startup ensures `~/.config/buddy/buddy.toml` exists by writing the compiled default template when the file is missing. Login auth stores provider-scoped tokens in `~/.config/buddy/auth.json` (mode `0600` on Unix), so one login is reused across model profiles that target the same provider.

API key resolution supports exactly one configured source per model profile: `api_key`, `api_key_env`, or `api_key_file`. The key source order is:
1. `BUDDY_API_KEY` (legacy `AGENT_API_KEY` fallback)
2. `models.<name>.api_key_env` (named environment variable; empty if unset)
3. `models.<name>.api_key_file` (file contents, with trailing newline trimmed)
4. `models.<name>.api_key` literal

`BUDDY_BASE_URL` and `BUDDY_MODEL` (legacy `AGENT_*` fallback) continue to override parsed config values.

### `api/` — HTTP client

A thin wrapper around `reqwest::Client`. `ApiClient` lives in `src/api/client.rs`, with protocol-specific modules split into `src/api/completions.rs` and `src/api/responses.rs`, plus provider policy rules in `src/api/policy.rs`.

The public method `chat()`:
1. Resolves auth header:
   - API key when provided (`Authorization: Bearer ...`)
   - Login token from `~/.config/buddy/auth.json` when `auth = "login"` (refreshes tokens when near expiry)
2. Chooses endpoint protocol per profile:
   - `api = "completions"` -> `{base_url}/chat/completions`
   - `api = "responses"` -> `{base_url}/responses`
3. For OpenAI login-backed Responses requests (`auth = "login"` without API key), sends `store = false` and `stream = true` to match ChatGPT Codex backend requirements.
4. Normalizes Responses API output back into the internal chat/tool-call structures used by the agent loop. For mandatory streaming paths, it consumes SSE and resolves the final `response.completed` payload.
5. Captures non-2xx responses as `ApiError::Status`.

### `tools/` — Tool system

The tool system is built around one trait and one registry.

**The `Tool` trait** (`tools/mod.rs`):

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

- `name()` returns the string the model uses to invoke the tool — it must match the `name` in the tool definition.
- `definition()` returns the OpenAI-format tool definition including the JSON Schema for parameters.
- `execute()` receives the raw JSON arguments string from the model and returns a text result.

The trait uses `async_trait` because dyn dispatch with native async fn in traits requires boxing the future, and `async_trait` handles this cleanly. All tools are `Send + Sync` so they work in async contexts.

**`ToolRegistry`** holds a `Vec<Box<dyn Tool>>` and provides:
- `register()` — add a tool
- `definitions()` — collect all tool definitions for API requests
- `execute(name, args)` — find the tool by name and call it

**Built-in tools:**

**`shell.rs` — `run_shell`**: Executes commands via `tokio::process::Command` with `sh -c`. If `confirm` is true, the tool either prompts directly (one-shot / non-brokered mode) or sends a foreground approval request through a channel consumed by the interactive REPL loop. Output (stdout + stderr + exit code) is truncated to 4000 characters to prevent blowing up the context window.

**`fetch.rs` — `fetch_url`**: Async HTTP GET via reqwest. Returns the response body as text, truncated to 8000 characters.

**`files.rs` — `read_file` / `write_file`**: Two tool structs in one module. `ReadFileTool` reads via `tokio::fs::read_to_string`, truncating to 8000 chars. `WriteFileTool` writes via `tokio::fs::write` and returns a confirmation message with the byte count.

**`search.rs` — `web_search`**: Searches DuckDuckGo's HTML endpoint (`html.duckduckgo.com/html/?q=...`) which requires no API key. The response HTML is parsed with a crude string-based scraper that extracts result titles, URLs, and snippets. This is intentionally simple — no HTML parser dependency. Results are capped at 8 entries.

All tools truncate their output. This is a deliberate design decision: tool output goes into the conversation history, so unbounded output can exhaust the context window in a single call.

### `agent.rs` — The agentic loop

This is the central module. The `Agent` struct owns everything:

- `client: ApiClient` — for API calls
- `config: Config` — full configuration
- `tools: ToolRegistry` — registered tools
- `messages: Vec<Message>` — conversation history
- `tracker: TokenTracker` — token accounting
- `renderer: Renderer` — terminal output

**Construction**: `Agent::new(config, tools)` initializes the client, sets up the token tracker with the context limit (from config or auto-detected from the model name), and prepends the system prompt to the message history.

**The `send()` method** is where the agentic loop lives:

```
1. Push user message to history
2. Check if approaching context limit (warn at 80%)
3. Loop:
   a. Build ChatRequest with full message history + tool definitions
   b. POST to the API
   c. Record token usage from response
   d. Extract the first choice's message
   e. Push assistant message to history
   f. If the message has tool_calls:
      - For each tool call:
        - Render the tool invocation (name + args preview)
        - Execute the tool via the registry
        - Render the tool result (truncated preview)
        - Push a Message::tool_result to history
      - Continue the loop (go back to step a)
   g. If no tool_calls: return the text content (done)
   h. If iterations > max_iterations: return error
```

The loop handles the case where the model makes multiple tool calls in a single response (parallel tool use). Each tool result gets its own message in the history, associated with the tool call by ID.

Tool execution errors don't crash the loop — they're formatted as error strings and sent back to the model, which can then decide how to proceed.

### `tokens.rs` — Token tracking

Two functions:

**Exact tracking**: The `record()` method captures `prompt_tokens` and `completion_tokens` from the API's `usage` field. These are accumulated into session totals. Some providers don't include usage data, so this is optional (the `usage` field is `Option` in the response).

**Estimation**: `estimate_messages()` provides a crude pre-flight check before sending a request. The heuristic is ~1 token per 4 characters, plus 16 characters (~4 tokens) of overhead per message for role and framing. This is intentionally rough — it's only used to trigger a warning when approaching the context limit, not for precise accounting.

**Context limits**: `default_context_limit()` now reads rules from a shipped `src/templates/models.toml` catalog (compiled into the binary). Rules support exact, prefix, and contains matching and are evaluated in file order. The catalog is a local snapshot of common models (sourced from OpenRouter's models API `context_length` field), and unknown models fall back to a conservative 8192. The config file can override this with an explicit `context_limit` value.

The `is_approaching_limit()` method warns when estimated usage exceeds 80% of the context window. This is a heuristic safety net, not a hard limit.

### `tui/renderer.rs` (+ `render.rs` compatibility shim) — Terminal output

All rendering is handled by the `Renderer` struct, which takes a `color: bool` flag. When color is disabled, output is plain text.

**Key design decision**: Status/chrome output (prompts, tool calls, token usage, warnings, errors) goes to **stderr** via `eprintln!`. The assistant's final response goes to **stdout** via `println!`. This means in one-shot mode, you can pipe just the response:

```bash
buddy exec "Generate a UUID" | pbcopy
```

Colors use crossterm's `Stylize` trait:
- Green bold: prompt (`>`), header (`buddy`)
- Yellow: tool call names
- Dark yellow: tool call indicator glyph
- Dark grey: tool arguments, tool results, token labels
- Cyan/dark cyan: token counts
- Red bold: errors
- Yellow bold: warnings

Tool call display shows the function name and a truncated preview of the arguments. Tool results show a truncated single-line preview. Both truncate at reasonable lengths (80 chars for args, 120 for results) and replace newlines with spaces.

`Renderer` also provides a TTY-only live progress spinner (`progress()`), used by the agent loop for long model/tool operations. The spinner line includes an in-progress label and elapsed time and is cleared automatically via an RAII handle when the operation completes. Interactive background-task mode keeps prompt stability by using REPL-rendered liveness lines above the editor rather than direct spinner thread output.

When a provider includes reasoning/thinking fields on assistant messages (for example `reasoning_content`), the agent renders those traces to stderr before continuing tool execution/final output.

### `error.rs` — Error handling

Four error enums in a hierarchy:

- **`ToolError`** — `InvalidArguments` (bad JSON from model) or `ExecutionFailed` (tool runtime error)
- **`ConfigError`** — `Io` (file not found) or `Toml` (parse error)
- **`ApiError`** — network/status errors plus login-required and invalid-response cases
- **`AgentError`** — wraps all three above, plus `EmptyResponse` and `MaxIterationsReached`

All error types implement `Display` and `Error` manually (no `thiserror`). `From` conversions allow `?` propagation from inner errors to `AgentError`. The binary (`main.rs`) is the only place that calls `process::exit()`.

### `cli.rs` — Argument parsing

Uses clap with derive macros and subcommands:
- `buddy` -> interactive REPL
- `buddy exec <prompt>` -> one-shot mode (send, print response, exit)
- `buddy login [profile]` -> provider login flow for configured model profile

CLI flags override config file values. This happens in `main.rs` after config loading.

### `main.rs` — Binary entry point

Wires everything together:

1. Parse CLI args
2. Load config (with CLI overrides)
3. Validate minimum config (base_url must be set)
4. Build the tool registry based on config flags
5. Create the agent
6. Branch: login / one-shot / interactive

The interactive REPL reads input via `tui/input.rs`, which runs in raw mode and supports:
- prompt `> ` (or `(ssh user@host)> ` when using `--ssh`)
- slash-command autocomplete when input starts with `/`
- command history navigation (`↑`/`↓`, `Ctrl-P`/`Ctrl-N`)
- multiline entry with `Alt+Enter`
- common line editing shortcuts (`Ctrl-A`, `Ctrl-E`, `Ctrl-B`, `Ctrl-F`, `Ctrl-K`, `Ctrl-U`, `Ctrl-W`)
- background prompt execution with task tracking (`/ps`), cooperative cancellation (`/kill <id>`), and per-task timeout control (`/timeout <duration> [id]`)
- command gating while background tasks run (only `/ps`, `/kill <id>`, `/timeout <duration> [id]`, `/approve <mode>`, `/status`, `/context` are accepted)
- foreground shell approval when background tasks hit a `run_shell` confirmation point (`y/yes` approve; `n/no` deny), rendered as an inline one-line approval prompt
- approval policy controls for shell confirmations (`/approve ask|all|none|<duration>`)
- persistent ID-based sessions stored locally under `.buddyx/` (`/session`, `/session resume <session-id|last>`, `/session new`, and CLI `buddy resume <session-id>|--last`)
- model-profile switching from config via `/model [name|index]` (no-arg opens arrow-key picker)

Supported slash commands:
- `/status` — current model, endpoint, enabled tools, and session counters
- `/model [name|index]` — switch the active configured model profile (`/model` opens picker)
- `/context` — estimated context window usage + token stats
- `/ps` — list running background tasks
- `/kill <id>` — cancel a running background task by task ID
- `/timeout <duration> [id]` — set timeout for a running background task (id optional only when one task exists)
- `/approve ask|all|none|<duration>` — configure shell approval policy for this session
- `/session` — list sessions ordered by last use
- `/session resume <session-id|last>` — save current session and resume by id / most-recent
- `/session new` — start and switch to a fresh generated session id
- `/help` — slash command reference (only when no background tasks are running)
- `/quit`, `/exit`, `/q` — exit (only when no background tasks are running)

Ctrl-D (EOF) also exits cleanly.

## Design decisions

**Why mostly non-streaming?** Tool call handling is simpler when each turn resolves to one final response object. Buddy keeps that architecture, but it also supports provider-mandated streaming transport for OpenAI login-backed Codex endpoints by consuming SSE internally and using the final `response.completed` object.

**Why async tools?** Tools like `fetch_url` and `web_search` are naturally async (network I/O). Making the trait async avoids blocking the tokio runtime. The shell tool uses `tokio::process::Command` and the file tools use `tokio::fs`, keeping everything non-blocking.

**Why truncate tool output?** Tool results go into the conversation history. A single `cat` of a large file could exhaust the entire context window. Truncation is a crude but effective safety measure. The limits (4K for shell, 8K for files/fetch) are configurable-by-code — a future version could make them configurable via TOML.

**Why DuckDuckGo for search?** It requires no API key, no account, and no rate limit management. The HTML endpoint is stable and parseable with simple string operations. This keeps the tool self-contained with zero setup.

**Why hand-written errors?** The error surface is small (4 enums, ~10 variants total). Manual `Display` and `From` impls take about 50 lines and add zero dependencies. `thiserror` would save a few lines but add a proc-macro dependency.

**Why `crossterm` instead of raw ANSI?** It's a small crate (~50KB compiled) that handles Windows compatibility. Raw ANSI codes would also work fine on macOS/Linux but would need manual handling for terminals that don't support them.

**Why stderr for chrome?** It enables clean piping in one-shot mode. The assistant's response is the "data" (stdout), everything else is "status" (stderr). This follows Unix conventions and makes the tool composable.

## Data flow

A typical multi-turn tool-use interaction:

```
User types: "What's in /tmp?"
     |
     v
main.rs: agent.send("What's in /tmp?")
     |
     v
agent.rs: push Message::user("What's in /tmp?")
agent.rs: check context limit — ok
agent.rs: build ChatRequest { messages: [system, user], tools: [run_shell, ...] }
     |
     v
api/client.rs: POST /responses (or /chat/completions) → 200 OK
     |
     v
agent.rs: response has tool_calls: [{ id: "call_1", function: { name: "run_shell", arguments: '{"command":"ls /tmp"}' } }]
agent.rs: push assistant message (with tool_calls) to history
     |
     v
tui/renderer.rs: ▶ run_shell({"command":"ls /tmp"})
     |
     v
tools/shell.rs: prompt user "Run: ls /tmp [y/N]"
user types: y
tools/shell.rs: tokio::process::Command::new("sh").arg("-c").arg("ls /tmp").output()
tools/shell.rs: return "exit code: 0\nstdout:\nfile1.txt\nfile2.log\n..."
     |
     v
tui/renderer.rs: ← exit code: 0 stdout: file1.txt file2.log...
agent.rs: push Message::tool_result("call_1", "exit code: 0\n...")
     |
     v
agent.rs: loop continues — build new ChatRequest { messages: [system, user, assistant+tool_calls, tool_result] }
     |
     v
api/client.rs: POST /responses (or /chat/completions) → 200 OK
     |
     v
agent.rs: response has content: "The /tmp directory contains: file1.txt, file2.log, ..."
agent.rs: no tool_calls — loop exits
     |
     v
main.rs: print response to stdout
```
