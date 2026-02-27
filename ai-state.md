# AI State (dense)

- Crate: `buddy` (lib + bin). Entry: `src/main.rs`, core loop: `src/agent.rs`.
- Config load precedence (effective): env vars override TOML; CLI flags override loaded config in `main.rs`.
- Startup now ensures `~/.config/buddy/buddy.toml` exists (materialized from compiled `src/templates/buddy.toml`).
- API protocol: OpenAI-compatible Chat Completions + Responses API (per-profile `api = "completions" | "responses"` in `src/config.rs`; wire handling in `src/api/` modules).
- Tool loop:
  1. push user msg
  2. call API with full history + tool defs
  3. if assistant emits `tool_calls`, execute each via `ToolRegistry`, push tool result messages
  4. repeat until assistant returns final text
- Status/chrome output -> stderr; assistant final answer -> stdout (`src/tui/renderer.rs`, re-exported via `src/render.rs`).

## Important recent changes

- Responses/login/subcommand upgrade:
  - `src/api/` is now split by concern:
    - `src/api/client.rs` (shared auth + dispatch),
    - `src/api/completions.rs` (`/chat/completions`),
    - `src/api/responses.rs` (`/responses` payload/parsing/SSE handling),
    - `src/api/policy.rs` (provider-specific runtime rules).
  - OpenAI login-backed Responses requests now force `store = false` and `stream = true`, then internally consume SSE until `response.completed` so the rest of the agent loop stays non-streaming.
  - Added profile fields in `src/config.rs`:
    - `api` (`completions` / `responses`)
    - `auth` (`api-key` / `login`)
  - Added secure login token storage in `src/auth.rs`:
    - file path: `~/.config/buddy/auth.json`
    - Unix perms: `0600`
    - Provider-scoped storage key (for example `openai`), with legacy profile-scoped token fallback migration.
    - OpenAI device-code login + refresh flow.
  - CLI now uses subcommands (`src/cli.rs`):
    - `buddy` (REPL)
    - `buddy exec <prompt>`
    - `buddy login [model-profile]`
    - `buddy help`
  - Added REPL slash command `/login [name|index]` (`src/tui/commands.rs`, `src/main.rs`).
  - Startup/model-switch auth checks now fail fast with actionable guidance when `auth = "login"` is set but the profile has no saved login.

- Config/template/bootstrap refresh:
  - Config schema migrated from single `[api]` to profile map `[models.<name>]` (also accepts `[model.<name>]` alias), with active profile selected by `[agent].model`.
  - Added runtime model profile switch via REPL slash commands: `/model <name|index>` and `/models`.
  - API key source options are per profile: `api_key`, `api_key_env`, `api_key_file` (`src/config.rs`), with strict mutual exclusivity validation.
  - API key resolution order now:
    1. `BUDDY_API_KEY` / `AGENT_API_KEY`
    2. selected profile `api_key_env` (named env var; empty if unset)
    3. selected profile `api_key_file` (file content, trailing newline trimmed)
    4. selected profile `api_key`
  - Default generated config template now uses `[models.gpt-codex]` (`gpt-5.3-codex`) and `[models.gpt-spark]` (`gpt-5.3-spark`), both with `api = "responses"` and `auth = "login"`.
  - Default active profile is now `agent.model = "gpt-codex"`.
  - `main.rs` now calls `ensure_default_global_config()` before `load_config(...)`.
  - Repository sample config moved from repo root to `src/templates/buddy.toml` and is embedded via `include_str!`.
  - System prompt template moved from `src/prompts/system_prompt.template` to `src/templates/system_prompt.template`; `src/prompts/` removed.
  - `models.toml` moved from repo root to `src/templates/models.toml` and is embedded via `include_str!` in `src/tokens.rs`.
  - Added `Makefile` targets:
    - `make build` -> `cargo build --release`
    - `make install` -> installs `buddy` to `~/.local/bin`

- tmux startup/session UX updates:
  - `ensure_tmux_pane_script()` creates new sessions directly with `buddy-shared` as the initial window (`tmux new-session -d -s "$SESSION" -n "$WINDOW"`), avoiding the extra default window.
  - Missing `buddy-shared` windows are created with `tmux new-window -d -t "$SESSION" -n "$WINDOW"` (no explicit `session:` target), avoiding index-collision errors like `create window failed: index 3 in use`.
  - Managed setup now assumes a single shared pane in `buddy-shared` (operators can add panes/windows manually).
  - `ExecutionContext::tmux_attach_info()` exposes attach metadata; `main.rs` now renders a single concise `execution` section with `attach to session: ...` (no separate tmux section).
  - Prompt-layout initialization (`BUDDY_PROMPT_LAYOUT`, etc.) runs only when the managed pane is newly created; existing panes are reused without re-init.
  - After first-time prompt setup, buddy sends `clear` so first attach lands on a fresh screen.
  - Local `--tmux` still rejects startup when the current pane window name is `buddy-shared`, with guidance to run buddy from a different terminal/pane.

- Output preview/tint rendering upgraded:
  - Tool output for `run_shell` and `read_file` now renders as clipped snippet blocks (first 10 lines) with `...N more lines...` continuation markers.
  - Snippet backgrounds are now tone-specific:
    - command/file output: greenish
    - reasoning/thinking: blue-gray
    - approval/waiting notices: reddish
  - `read_file` snippets get lightweight syntax highlighting via `syntect` based on file extension/path.
  - Core implementation in:
    - `src/tui/renderer.rs` (snippet block rendering + tone palettes)
    - `src/tui/highlight.rs` (syntect integration)
    - `src/tui/text.rs` (`snippet_preview`)
    - `src/main.rs` (`run_shell`/`read_file` tool-result routing to blocks)
    - `src/tui/settings.rs` (new block/tint constants)
    - `src/tui/prompt.rs` (approval prompt red-tinted)
  - Dependency added: `syntect` (`Cargo.toml`).

- Block rendering behavior refined:
  - Block backgrounds are now rectangular/padded to terminal width (after a fixed two-space left indent), so tints are consistent even on short lines or trailing whitespace.
  - Tool/read/reasoning/approval/final-output blocks now share consistent two-space indentation.
  - Wrapping modes:
    - text/reasoning/read/approval/final assistant output: wrapped to block width
    - shell command stdout/stderr: clipped at block edge (no wrapping)
  - Final assistant output now renders in a dedicated markdown-oriented tinted block (white foreground).
  - System prompt template now explicitly instructs the model to return final user-facing output as valid Markdown.
  - Final assistant markdown is parsed/rendered via `pulldown-cmark` (`src/tui/markdown.rs`) before block display.
  - Block spacing is collapsed with a stream-aware state machine so blocks keep exactly one blank line above/below without stacking extra blank lines.
  - Block width now reserves a 2-char right margin from terminal edge.

- New tools added:
  - `capture-pane` (enabled only when tmux pane capture is available for the active execution context):
    - Exposes common `tmux capture-pane` options (`target`, `-S/-E`, `-J`, `-N`, `-e`, `-C`, `-a`) and supports delayed capture for polling.
    - Tool defaults use tmux screenshot behavior (visible pane content) unless explicit `start`/`end` bounds are provided.
    - If alternate-screen capture is requested but unavailable, it now falls back to main-pane capture with a notice instead of hard failing.
    - Intended for interactive/full-screen apps and stuck-command diagnosis.
    - Implemented in `src/tools/capture_pane.rs`, backed by shared execution API in `src/tools/execution.rs`.
  - `time`:
    - Returns harness-recorded wall-clock time (not remote shell time) in multiple common formats (unix epoch, ISO-8601 UTC, RFC-2822 UTC, date/time UTC).
    - Implemented in `src/tools/time.rs`.
  - `send-keys`:
    - Sends tmux key input directly to a pane (for example `C-c`, `C-z`, `Enter`, arrows, or literal text).
    - Implemented in `src/tools/send_keys.rs`, backed by `ExecutionContext::send_keys(...)`.
  - `main.rs` now always registers `time`, and registers `capture-pane` only when `ExecutionContext::capture_pane_available()` is true.
  - `main.rs` registers `send-keys` alongside `capture-pane` when tmux pane tooling is available.

- `run_shell` wait behavior expanded:
  - New optional `wait` argument:
    - `true` (default): wait for completion.
    - `false`: dispatch command into tmux pane and return immediately (requires tmux-backed target).
    - duration string/integer (e.g. `"10m"`): wait up to timeout, then return timeout error.
  - Recommended interactive flow:
    1. `run_shell` with `wait:false`
    2. `capture-pane` with optional delay polling
    3. `send-keys` for control input if needed

- Background-task REPL mode added:
  - User prompts now run as background tasks (numbered IDs) so the prompt remains available while model/tool work is in progress.
  - New slash commands:
    - `/ps` lists currently running background tasks and elapsed runtime.
    - `/kill <id>` cancels a running background task by ID.
  - While any background task is active, only `/ps`, `/kill <id>`, `/timeout <dur> [id]`, `/approve <mode>`, `/status`, and `/context` are accepted; all other prompts/commands return a friendly warning.
  - Interactive mode uses REPL-integrated liveness lines (spinner + runtime/state) while tasks are active, so input remains readable.
  - Shell confirmations are now first-class in background mode:
    - `run_shell` no longer reads stdin directly when brokered.
    - Approval requests are sent over a channel and brought to compact one-line foreground UI:
      - `<actor>$ <command> -- approve?`
    - REPL input is interruptible, so approval can preempt typing cleanly.
    - `/ps` now shows task state (`running` vs `waiting approval ...`).
    - Approval prompt actor is target-aware (`ssh user@host`, `container:name`, or `local`) and colorized for readability.
  - `/kill <id>` is now cooperative cancellation (not hard `JoinHandle::abort()`):
    - Each background task carries a cancellation signal.
    - `Agent` listens for cancellation during API calls and tool execution using `tokio::select!`.
    - On cancellation during tool-call turns, the agent now appends tool results with exact text `operation cancelled by user` for outstanding tool call IDs before returning.
    - This prevents invalid conversation history (`assistant.tool_calls` without matching `tool` messages) and fixes follow-up `400` errors after kill.
    - Task state now includes `cancelling`, and `/ps`/liveness lines reflect that state until completion.
  - Task controls expanded:
    - `/timeout <dur> [id]` sets per-task timeout (supports `ms`, `s`, `m`, `h`, `d`; bare number = seconds).
    - If task id omitted, timeout applies only when exactly one background task exists; otherwise an explicit id is required.
    - Expired timeouts trigger cooperative cancellation and deny pending approvals for that task.
  - Approval policy controls expanded:
    - `/approve ask` (default), `/approve all`, `/approve none`, `/approve <dur>`.
    - `all` auto-approves shell confirmations, `none` auto-denies, `<dur>` auto-approves until expiry, then falls back to `ask`.
    - Policy is surfaced in `/status` and applied both during normal prompt flow and approval UI flow.

- Documentation contract tightened:
  - `DESIGN.md` now has a dedicated `## Features` section intended as the canonical current feature inventory.
  - `AGENTS.md` now explicitly requires updating that section whenever features/behavior change.

- System prompt templating added:
  - New module: `src/prompt.rs`.
  - Prompt text now compiles from a single file: `src/templates/system_prompt.template`.
  - The full prompt is rendered in one place from runtime parameters (`enabled tools`, `execution target`, optional operator instructions).
  - `[agent].system_prompt` now acts as optional additional operator instructions (default empty), appended into the rendered template.
  - Remote target instructions are parameterized by template render (instead of separate text-file append logic).

- Session persistence added:
  - New module: `src/session.rs` storing named sessions under `.buddyx/sessions`.
  - Agent state snapshot/restore support lives in `src/agent.rs` (`snapshot_session`, `restore_session`, `reset_session`).
  - Interactive mode auto-loads/saves `default` session and persists on prompt completion.
  - New slash command flow:
    - `/session` lists sessions ordered by last use
    - `/session resume <name|last>` resumes a specific or most-recent session
    - `/session new <name>` creates/switches to a fresh named session
  - Test isolation fix:
    - `src/session.rs` tests now use an atomic suffix for temp session roots to avoid occasional collisions when tests execute quickly/parallel.

- Shell spinner + prompt UX updates:
  - `run_shell` now always shows a spinner while the command itself is executing (in `src/tools/shell.rs`), even when shell confirmation is enabled.
  - Generic tool spinner in `src/agent.rs` now skips `run_shell` to avoid spinner/confirmation prompt collisions.
  - Background sends now forward tool-call/reasoning/token events to the foreground REPL loop via channel, so activity remains visible while preserving stable input editing.
  - REPL prompt now shows:
    - local: `> `
    - SSH target: `(ssh user@host)> `
  - Prompt marker `>` is rendered bright/bold when color is enabled.

- Added execution-target routing for `run_shell` + file tools:
  - New CLI flags:
    - `--container <id/name>`: execute `run_shell`, `read_file`, `write_file` inside container.
    - `--ssh user@host`: execute those tools on remote host via persistent SSH ControlMaster session.
    - `--tmux [session]`: tmux-backed execution (default auto session `buddy-xxxx`) local by default, or on `--ssh` / `--container` target.
  - New backend module: `src/tools/execution.rs`.
    - Supports `local`, `container`, and `ssh` execution contexts.
    - Detects whether `docker` frontend is actually podman-compatible by probing version output.
    - Initializes SSH master connection at startup and keeps it alive for the app lifetime.
    - Local `--tmux` creates/reuses a persistent local session (default `buddy-xxxx`) with the same shared-pane prompt-marker protocol used by SSH+tmux.
    - `--container ... --tmux` creates/reuses a persistent tmux session inside the container and enables the same `wait=false`/`capture-pane`/`send-keys` workflow.
    - If remote `tmux` exists, SSH auto-creates/reuses a stable per-target session (or uses `--tmux` name).
    - Default tmux session naming is short (`buddy-xxxx`, 4-hex suffix) for local, container, and SSH targets.
    - All SSH tool commands run inside that tmux session; if session is deleted, it is recreated on next command.
    - SSH tmux now uses a stable shared window (`buddy-shared`) and reuses the same pane across commands.
    - Commands are injected into that persistent pane (`tmux send-keys`) so humans and the agent can observe the same live terminal state.
    - Prompt customization is now applied once per pane lifecycle (during SSH/tmux setup, and again only if tmux recreates a different pane), not before every command.
    - Prompt setup is versioned (`BUDDY_PROMPT_LAYOUT=v3`) so existing panes with old prompt config are upgraded on reconnect.
    - On first shared-pane setup, the shell prompt is reconfigured to include marker + exit status:
      - `[buddy <command-id>: <last-exit>] ...original prompt...`
    - Per command:
      - send the exact command text directly (no wrapper subshell/parens)
      - prompt displays ID via `$(__agent_prompt_id)`; incrementation happens in shell prompt hooks (`PROMPT_COMMAND` for bash / `precmd_functions` for zsh), avoiding subshell side-effect loss.
      - parser takes the baseline prompt ID `N` before sending command and expects completion at prompt `N+1`
      - parser ignores repeated markers `<= N` and uses the first marker `> N` as completion candidate.
      - parser boundaries now come directly from prompt IDs in full `capture-pane` history: extract output strictly between marker `N` and completion marker `N+1` (no baseline overlap slicing), preventing old setup text bleed-through.
      - parse command output by diffing `capture-pane` content against a pre-command baseline (suffix overlap), so completion still works even if tmux history size stays constant while scrolling.
      - exit code comes from the trailing completion prompt marker.
    - Uses `tmux capture-pane -p -J -S - -E -` for shared-pane parsing.
  - `src/tools/shell.rs` and `src/tools/files.rs` now delegate execution through shared `ExecutionContext`.
  - `main.rs` now validates target flags early:
    - if `--container`/`--ssh` is set while both shell/files tools are disabled, exits with a friendly error.

  - Added modular TUI implementation under `src/tui/`:
  - `input.rs` for interruptible raw-mode editing loop
  - `commands.rs` for slash-command parsing/autocomplete
  - `input_buffer.rs` + `input_layout.rs` for editing/layout primitives
  - `prompt.rs` for prompt rendering and mode-specific prompt text
  - `settings.rs` as the single place for hardcoded UI constants (colors, glyphs, prompt strings/formats, indentation, spinner behavior)
  - Prompt is `> ` locally, or `(ssh user@host)> ` when `--ssh` is set.
  - Raw-mode line editor with:
    - cursor/edit keys: arrows, backspace/delete/home/end
    - ctrl shortcuts: `Ctrl-A/E/B/F/K/U/W`
    - history navigation: `Up/Down` and `Ctrl-P/N`
    - multiline composition: `Alt+Enter` inserts newline without submit
  - Slash autocomplete dropdown appears when input begins with `/`.
  - Commands:
    - `/status`: model/base_url/tools/session counters
    - `/context`: estimated context usage + last/session token counts
    - `/ps`: show running background tasks
    - `/kill <id>`: cancel a background task
    - `/timeout <dur> [id]`: set timeout for running background tasks
    - `/approve ask|all|none|<dur>`: set shell-approval policy
    - `/session`: list/resume/create named local sessions
    - `/help`: list commands (blocked while tasks run)
    - `/quit`, `/exit`, `/q`: exit (blocked while tasks run)
  - `main.rs` now routes REPL input through `buddy::tui::read_repl_line_with_interrupt(...)` and handles slash commands before `agent.send(...)`.
  - `ReplState` persists history across turns.
  - Input now uses `trim_end()` (not full trim) so multiline commands preserve leading indentation.
  - `Ctrl-C` now exits interactive mode instead of only clearing the current input line.

- Token display default changed:
  - `display.show_tokens` default is now `false` in `Config::default`.
  - template `src/templates/buddy.toml` also sets `show_tokens = false`.
  - use `/context` and `/status` for token/session visibility instead of per-turn spam.

- Context-limit estimation upgraded:
  - Added shipped `src/templates/models.toml` catalog (compiled into binary).
  - `src/tokens.rs::default_context_limit()` now loads catalog rules (exact/prefix/contains) via `OnceLock`.
  - Rule matching normalizes model IDs and strips OpenRouter variants like `:free`.
  - Falls back to conservative built-in heuristics only if catalog parsing fails.
  - Catalog snapshot source: `https://openrouter.ai/api/v1/models` (`context_length` field), fetched 2026-02-18.

- Live progress UI added:
  - `src/tui/progress.rs` provides spinner primitives and RAII `ProgressHandle`.
  - `src/tui/renderer.rs` exposes `Renderer::progress(label)` and `Renderer::progress_with_metrics(...)`.
  - Spinner is TTY-only, updates in place with elapsed time, and clears when dropped.
  - `src/agent.rs` now wraps model calls in progress status (`calling model ...`).
  - Tool execution now shows progress (`running tool ...`) except `run_shell` (which handles its own progress to keep confirmation UX stable).
  - `Renderer` now supports global progress suppression used by interactive background-task mode.

- Latest TUI styling/markdown updates:
  - Final assistant output markdown rendering now uses `termimad` (`MadSkin::no_style`) via `src/tui/markdown.rs`, then passes through tinted block layout in `renderer.rs`.
  - Shared green-tint block background was darkened again for subtler contrast; approvals remain red-tinted.
  - `run_shell` result rendering no longer prints a separate `stdout:` label before the output block (`src/main.rs`).
  - Normal prompt now includes context usage estimate, e.g. `(ssh user@host) (4% used)>`.
  - Approval prompt is now two lines:
    - `  $ <command>`
    - `  • run on <target>? `
  - Startup session line is now a single bullet line:
    - `• using existing session "default"` or `• creating new session "default"`.
  - Reasoning/thinking blocks now render full text (no 10-line snippet truncation), with a slightly darker green tint and grayer text.
  - Activity lines for task/prompt lifecycle (for example `prompt #N processed...`, `task #N ...`) now use bold gray styling via `Renderer::activity(...)`.
  - `capture-pane` tool results now render captured text in the same non-wrapping command-output block style used for shell output.
  - Approval input flow now inserts a blank line after the entered approval response for cleaner separation from following output.

- Reasoning trace display added:
  - `src/agent.rs` now detects provider fields containing `reasoning` / `thinking` / `thought` in assistant message `extra`.
  - Matching fields are rendered to stderr via `Renderer::reasoning_trace(...)`.
  - Works for string and JSON reasoning payloads (JSON shown pretty-printed).

- REPL wrapping fix:
  - `src/tui/input.rs` + `src/tui/input_layout.rs` track visual rows (soft wraps + explicit newlines), not just newline count.
  - Cursor placement and clear/redraw now use terminal width-aware layout computation.
  - This fixes long-line typing at screen edge where prompt lines were previously duplicated instead of wrapping cleanly.
  - Added comprehensive pseudo-terminal layout tests for:
    - hard/soft wrap boundaries
    - narrow terminal prompt + continuation wrapping
    - cursor-at-offset placement within wrapped buffers
    - autocomplete suggestion row accounting and redraw move-up math

- Provider-compatibility fix for reasoning/tool-call models:
  - `Message` now preserves unknown fields via `#[serde(flatten)] extra`.
  - This prevents dropping provider metadata (e.g. `reasoning_content`) between turns.
  - `content` is serialized as `null` when absent (instead of omitted), improving compatibility for assistant tool-call messages.
  - Token estimator now includes preserved extra JSON fields (`src/tokens.rs`).

## Where to edit next

- REPL UX behavior: `src/tui/input.rs`
- Slash commands: `src/tui/commands.rs`
- Prompt formatting + approval prompt modes: `src/tui/prompt.rs`
- Status/chrome rendering: `src/tui/renderer.rs` (compat shim: `src/render.rs`)
- API wire compatibility/types: `src/types.rs`
- Context calculations: `src/tokens.rs`

## Verification baseline

- `cargo fmt`
- `cargo test` (currently passing all unit + doc tests)
