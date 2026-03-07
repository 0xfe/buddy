# AI State (quick onboarding)

## 30-second orientation
- Project: `buddy` Rust crate (`src/lib.rs`) + CLI (`src/main.rs`) for OpenAI-compatible agent workflows.
- Core loop ownership: `Agent` orchestrates conversation/tool execution; do not duplicate loop control in other modules.
- Canonical behavior inventory: `docs/design/DESIGN.md`.
- Architecture index: `docs/design/architecture.md`.
- Runtime + REPL usage/operator docs: `docs/design/terminal-repl.md`, `docs/design/tools.md`, `docs/design/remote-execution.md`.
- Observability docs: `docs/design/observability.md` (`--trace`, `BUDDY_TRACE_FILE`, JSONL runtime-event traces, `buddy traceui`).
- Context + compaction behavior: `docs/design/context-management.md`.

## Fast local commands
```bash
cargo build
cargo test
cargo run
cargo run -- exec "prompt"
cargo run -- --trace /tmp/buddy.trace.jsonl
```

## High-value file map
- CLI/startup wiring: `src/main.rs`, `src/cli.rs`.
- Agent/runtime orchestration: `src/agent/`, `src/runtime/`, `src/app/`.
- History compaction + tool-pair repair: `src/agent/history.rs`, `src/agent/normalization.rs`.
- API/auth/config: `src/api/`, `src/auth/`, `src/config/`.
- Tooling: `src/tools/` (+ execution backends under `src/tools/execution/`).
- Terminal UI layers: `src/ui/`, `src/ui/terminal/`, `src/repl/` (`src/tui/` is compatibility re-export only).
- Trace viewer: `src/traceui/` (generic JSONL parsing, incremental tailing, split-pane interactive viewer state/rendering, diff-based repainting).
- Shared tmux domain: `src/tmux/`.
- TTY regression harness: `tests/ui_tmux/`, `tests/ui_tmux_regression.rs`, `tests/traceui_tmux_regression.rs`.

## Invariants to preserve
- Error model: explicit handwritten enums; avoid `anyhow`/`thiserror`.
- Output split: status/chrome on stderr, assistant/user-facing content on stdout.
- Tool outputs must stay truncated to protect context window.
- Config precedence: env > CLI flags > local file > global file > defaults.
- Tests should stay offline and deterministic.

## Current defaults and operator-facing behavior
- Global config lives at `~/.config/buddy/buddy.toml` (bootstrapped when missing).
- Sessions persist in `.buddyx/sessions` (`.agentx` legacy fallback still supported).
- REPL history persists in `~/.config/buddy/history` when enabled.
- Local execution commonly uses tmux-backed contexts; see `docs/tips/tmux.md` + `docs/design/remote-execution.md`.
- Managed tmux selector behavior: blank/whitespace `target`/`session`/`pane` inputs normalize to the default shared pane/session, `tmux_capture_pane` auto-falls back to the default shared pane when an explicit managed target is missing, and mutating tools keep strict missing-target errors.
- All tools now require a concise `why` rationale; non-shell tool calls show that rationale as a plain indented line in the console, `run_shell` keeps justification only in its approval/shell UI, assistant text attached to tool-calling turns is streamed live, repeated unchanged `tmux_capture_pane` snapshots are replaced with an explicit "nothing changed" notice, console reasoning hides `reasoning_stream` duplicates in favor of the final `reasoning` block, and request/session cost estimates are no longer computed or shown.
- `traceui` keeps the right pane always expanded and vertically scrollable, decodes JSON embedded inside string fields where possible, colorizes structured detail values, and repaints only changed rows to reduce flicker during navigation/streaming.
- `make test-ui-regression` now runs both the REPL tmux suite and the dedicated traceui tmux suite.
- Prompt assembly now adds request-scoped context annotation (`--` section separators, tmux snapshot/route state, actor/action history ledger) plus a final tail-instruction message reinforcing default tmux targeting and shared-shell safety (`set -e`/`exit`/`exec` restrictions).

## Docs map for AI agents
- Core docs: `README.md`, `docs/design/DESIGN.md`, `docs/design/architecture.md`.
- Operational references: `docs/design/tools.md`, `docs/design/terminal-repl.md`, `docs/design/remote-execution.md`, `docs/developer/deprecations.md`.
- Tactical guidance: `docs/tips/*.md`.
- Planning/history: `docs/plans/`.

## Update policy for this file
- Keep this file concise and current; replace stale text instead of appending timelines.
- Keep only context needed to start productive work quickly.
- Move tactical playbooks/tips into `docs/tips/`.
