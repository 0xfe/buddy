# Feature Catalog

This document is the detailed feature inventory for Buddy. It is intentionally implementation-oriented.

For the high-level architecture narrative, see [docs/design/DESIGN.md](DESIGN.md).

## CLI Surface

### Top-level commands

- `buddy` (no subcommand): starts interactive REPL mode.
- `buddy init [--force]`: writes `~/.config/buddy/buddy.toml` from the built-in template.
- `buddy exec <prompt>`: executes one prompt and exits.
- `buddy resume <session-id>`: starts REPL after restoring a saved session.
- `buddy resume --last`: starts REPL using the most recent saved session.
- `buddy login [model-profile] [--check] [--reset]`: runs login health/reset/device flow for the selected profile.

### Global CLI flags

- `-c, --config <path>`: explicit config file path.
- `-m, --model <selector>`: model profile key or direct model-id override.
- `--base-url <url>`: API base URL override.
- `--container <name>`: run shell/file tools in a container target.
- `--ssh <user@host>`: run shell/file tools over SSH.
- `--tmux [session]`: opt into tmux-backed execution and optionally set session name.
- `--no-color`: disable colored output.
- `--dangerously-auto-approve`: in `exec` mode, bypass `run_shell` confirmations.

### Exec safety behavior

- `buddy exec` fails closed when `tools.shell_enabled=true` and `tools.shell_confirm=true`.
- `buddy exec --dangerously-auto-approve` disables shell confirmation for that invocation only.

## Configuration and Defaults

### Profile model

- Config is profile-based under `[models.<name>]`.
- Active profile is selected via `agent.model`.
- Per profile:
  - `api = "completions" | "responses"`
  - `auth = "api-key" | "login"`
  - `api_base_url`
  - exactly one key source among `api_key`, `api_key_env`, `api_key_file`
  - optional `model`
  - optional `context_limit`

### Bundled defaults

Built-in profile catalog includes:

- `gpt-spark` (default; OpenAI Responses, login auth)
- `gpt-codex` (OpenAI Responses, login auth)
- `openrouter-deepseek` (OpenRouter Completions, API key)
- `openrouter-glm` (OpenRouter Completions, API key)
- `kimi` (Moonshot Completions, API key)

### Resolution and precedence

Effective behavior is:

- Config file source precedence:
  - explicit `--config`
  - `./buddy.toml`
  - legacy `./agent.toml`
  - `~/.config/buddy/buddy.toml`
  - legacy `~/.config/agent/agent.toml`
  - built-in defaults
- Runtime override precedence:
  - CLI overrides
  - `BUDDY_*` env vars (legacy `AGENT_*` fallback)
  - parsed file values
  - defaults

### Network and display settings

- `[network]`
  - `api_timeout_secs`
  - `fetch_timeout_secs`
- `[display]`
  - `color`
  - `theme`
  - `show_tokens`
  - `show_tool_calls`
  - `persist_history`
- `[themes.<name>]`
  - semantic token overrides (`warning`, `block_assistant_bg`, etc.)
  - supports named terminal colors and `#RRGGBB` values

### Initialization behavior

- Startup ensures `~/.config/buddy/buddy.toml` exists.
- `buddy init --force` writes timestamped backup before overwrite.

## Auth and Login

- `auth = "api-key"`: uses resolved API key from env/file/inline source.
- `auth = "login"`: uses provider-scoped OAuth token store (`~/.config/buddy/auth.json`).
- OpenAI login runtime can rewrite base URL to ChatGPT Codex backend when required.
- Login flow supports:
  - health checks (`--check`)
  - reset (`--reset`)
  - device auth + browser open fallback
- Token storage:
  - encrypted at rest
  - provider-scoped records
  - legacy profile-scoped fallback for backward compatibility
  - near-expiry refresh behavior

## Runtime Modes

- Interactive REPL (`buddy`, `buddy resume ...`)
  - drives prompt execution via runtime commands/events
  - supports background tasks, cancellation, approval mediation
- One-shot exec (`buddy exec ...`)
  - submits one runtime task
  - waits for final message or failure
  - returns exit code based on task result

## Built-in Tools

All tool outputs are wrapped as JSON:

- `harness_timestamp`: `{ source, unix_millis }`
- `result`: tool-specific payload

### Tool list

- `run_shell`
  - required metadata: `risk`, `mutation`, `privesc`, `why`
  - optional `wait`: `true`, `false`, duration (`"10m"`) or integer seconds
  - optional managed tmux selectors: `session`, `pane`
  - denylist enforcement via `tools.shell_denylist`
  - optional confirmation flow (`tools.shell_confirm`)
  - streaming tool events in runtime mode
  - output truncation (4K)
- `read_file`
  - backend-aware file read
  - output truncation (8K)
- `write_file`
  - backend-aware write
  - sensitive-path blocking plus optional allowlist (`tools.files_allowed_paths`)
- `fetch_url`
  - HTTP(S) GET with timeout
  - default SSRF protections (localhost/private/link-local blocking)
  - optional allow/deny domain policy
  - optional confirmation (`tools.fetch_confirm`)
  - output truncation (8K)
- `web_search`
  - DuckDuckGo HTML scraping via CSS selectors
  - parser-break fallback diagnostics
  - max 8 results
- `capture-pane`
  - tmux pane snapshot tool with delay and capture options
  - optional managed tmux selectors: `session`, `pane`
  - defaults to visible screenshot behavior
  - alternate-screen fallback when unavailable
  - output tail truncation
- `send-keys`
  - tmux key injection (`keys`, `literal_text`, `enter`, delay)
  - optional managed tmux selectors: `session`, `pane`
  - required metadata: `risk`, `mutation`, `privesc`, `why`
- `tmux-create-session`
  - create/reuse buddy-managed tmux session and shared pane
  - enforced managed ownership + `tmux.max_sessions` limit
  - required metadata: `risk`, `mutation`, `privesc`, `why`
- `tmux-kill-session`
  - kill one buddy-managed tmux session (default shared session protected)
  - required metadata: `risk`, `mutation`, `privesc`, `why`
- `tmux-create-pane`
  - create/reuse buddy-managed pane in managed session
  - optional `session` selector, required `pane`
  - enforced managed ownership + `tmux.max_panes` limit
  - required metadata: `risk`, `mutation`, `privesc`, `why`
- `tmux-kill-pane`
  - kill one buddy-managed pane (default shared pane protected)
  - optional `session` selector, required `pane`
  - required metadata: `risk`, `mutation`, `privesc`, `why`
- `time`
  - harness wall-clock snapshot in unix and UTC text formats

## Execution Targets and Backends

### Target modes

- local (non-tmux backend)
- local tmux-backed managed session
- container backend
- container tmux-backed managed session
- SSH backend with persistent control socket
- SSH + tmux managed session (auto-enabled when available)

### Target selection behavior

- If shell/files tools are disabled, Buddy uses local non-tmux execution context.
- If shell/files tools are enabled and no remote target is specified, Buddy initializes local tmux-managed execution.
- `--container` and `--ssh` select corresponding remote backends.
- `--tmux` can provide explicit session name; otherwise defaults to `buddy-<agent.name>`.

### Managed tmux behavior

- Buddy manages one shared tmux window (`shared`) and pane title (`shared`).
- It refuses to launch from the managed local shared pane to avoid self-injection loops.
- Startup prints attach instructions for local/SSH/container tmux targets.
- Prompt markers are installed and parsed for deterministic command completion in tmux-backed `run_shell`.
- Managed ownership is enforced via tmux options (`@buddy_managed`, `@buddy_owner`).
- Managed naming is canonicalized to the buddy owner prefix (`buddy-<agent.name>-...`).
- Configurable tmux limits:
  - `[tmux].max_sessions` (default `1`)
  - `[tmux].max_panes` (default `5`, per managed session, including shared pane)

## REPL UX

### Input and editing

- Slash command parsing with autocomplete.
- History navigation (`Up/Down`, `Ctrl-P/N`).
- Multiline input (`Alt+Enter`).
- Standard line-edit shortcuts (`Ctrl-A/E/B/F/K/U/W`, arrows, home/end, delete/backspace).
- Optional persistent history file: `~/.config/buddy/history`.

### Slash commands

- `/status`
- `/context`
- `/ps`
- `/kill <id>`
- `/timeout <duration> [id]`
- `/approve ask|all|none|<duration>`
- `/session [list|resume <id|last>|new]`
- `/compact`
- `/model [name|index]`
- `/theme [name|index]`
- `/login [name|index]`
- `/help`
- `/quit`, `/exit`, `/q`

### Background task model

- REPL submits prompts as runtime tasks.
- One active runtime prompt task is enforced.
- Liveness line shows running/waiting/cancelling state.
- `/kill` and timeout enforcement send runtime cancellation commands.
- During background-task activity, only a restricted slash-command subset is accepted.

### Approval UX

- Runtime emits `TaskEvent::WaitingApproval` with command + metadata.
- REPL enters approval mode with dedicated prompt and shell snippet block.
- Decision input supports `y/yes`, `n/no`, and selected slash commands.
- Approval policy supports ask/all/none/expiring auto-approve windows.

### Session UX

- Sessions persisted under `.buddyx/sessions` (`.agentx` fallback).
- `/session` lists by recency.
- `/session resume <id|last>` and `/session new` supported.
- CLI `buddy resume ...` paths map to same store behavior.

## Prompt Behavior

- System prompt rendered from one built-in template.
- Template variables include:
  - enabled tools
  - execution target note (local/container/ssh)
  - optional custom operator instructions
- When `capture-pane` is available, Buddy refreshes a tmux snapshot block before each model request.
- Snapshot text is explicitly labeled as the default shared-pane screenshot.
- When the most recent tmux tool call targets a non-default session/pane, default-pane snapshot injection is skipped for that request.
- Snapshot block is replaced in-place in the primary system message (not accumulated across turns).

## Rendering and Output Behavior

- Assistant final content is rendered to stdout.
- Status/chrome/progress/tool traces are rendered to stderr.
- Renderer abstraction (`RenderSink`) decouples orchestration from concrete terminal implementation.
- Runtime event renderer updates task/session/context state from typed runtime events.
- Tool/result snippets and reasoning blocks use formatted terminal blocks.
- Progress handling is centralized and can be disabled while background tasks run.

## Token and Context Behavior

- Exact token accounting from response `usage` when provided.
- Session totals and last-call counters tracked with saturating updates.
- Heuristic preflight estimate drives warnings and hard-limit checks.
- Hard-limit guard attempts automatic compaction before failing.
- Manual `/compact` triggers stronger compaction target.
- Context limits come from embedded `templates/models.toml` rule catalog with legacy fallback heuristics.

## API Protocol and Compatibility Behaviors

- Per-profile protocol selection:
  - `/chat/completions`
  - `/responses`
- `/responses` path includes request translation and response normalization back to internal chat/tool-call shape.
- OpenAI login-backed Responses requests can force `store=false` and `stream=true` with SSE parsing.
- Retry policy covers timeouts/connectivity/429/5xx with `Retry-After` support.
- 404 errors include protocol mismatch hints.

Conversation compatibility behaviors:

- Unknown message fields are preserved via `Message.extra` round-trip.
- Empty/null assistant turns are sanitized from history.
- Assistant messages are normalized before persistence/reuse.
- Provider reasoning payloads are extracted and rendered when textual.

Migration compatibility behaviors:

- Legacy config/env aliases still work with deprecation warnings:
  - `AGENT_*` env vars
  - `agent.toml`
  - legacy `[api]` table
- Legacy session root `.agentx` is reused when `.buddyx` is absent.
- Legacy auth profile records are still readable with migration warnings.

## Developer-Facing Interfaces

- `ModelClient` trait enables mock/offline model clients.
- `Agent::with_client(...)` supports deterministic injection.
- `AgentRunner` provides stream-capable runner facade over `Agent`.
- Runtime spawn entry points:
  - `spawn_runtime(...)`
  - `spawn_runtime_with_agent(...)`
  - `spawn_runtime_with_shared_agent(...)`
- Runtime command/event protocol provides frontend-neutral control/data plane.
- `examples/alternate_frontend.rs` demonstrates non-default frontend integration over runtime channels.
- Optional parser property tests are available via `cargo test --features fuzz-tests`.
