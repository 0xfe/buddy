# Design

Buddy is a Rust terminal agent for OpenAI-compatible APIs. This document is intentionally high-level and human-readable.

Detailed operational and protocol behavior now lives under `design/`.

## Architecture Overview

Buddy is one crate with:

- library surface in `src/lib.rs`
- CLI binary entrypoint in `src/main.rs`

High-level structure:

```text
main.rs + cli.rs + app/                 CLI parsing and mode orchestration
                |
                +--> runtime/ + repl/ + ui/runtime/
                |      Command/event runtime actor + REPL state + event rendering
                |
                +--> agent/ + api/ + auth/
                |      Agentic loop + model transport + login/auth
                |
                +--> tools/ + tools/execution/ + tmux/
                |      Tool registry + execution backends + tmux integration
                |
                +--> config/ + preflight.rs + prompt.rs
                |      Profile/config resolution + readiness checks + prompt templating
                |
                +--> session.rs + tokens.rs + types.rs + error.rs
                       Persistence + token/context tracking + shared data/error primitives
```

## Design Goals

- Keep operator workflows terminal-first and scriptable.
- Separate control-plane state (runtime events/commands) from rendering concerns.
- Support mixed provider ecosystems without tying core logic to one wire protocol.
- Keep tool execution practical and safe by default (policy gates, confirmations, truncation).
- Preserve compatibility with legacy config/session/auth footprints while guiding migration.

## Features

Detailed inventory: [feature-catalog.md](feature-catalog.md).

### User-facing

- CLI modes and commands:
  - REPL (`buddy`)
  - one-shot exec (`buddy exec <prompt>`)
  - session resume (`buddy resume <id|--last>`)
  - setup/auth (`buddy init`, `buddy login`, `buddy logout`)
  - trace analysis (`buddy trace summary|replay|context-evolution`)
  - interactive trace viewer (`buddy traceui <file> [--stream]`)
  - first-run guided init auto-bootstrap when no config exists
- Global targeting and runtime flags:
  - config/model/base-url overrides
  - `--ssh`, `--container`, `--tmux [session]` (`--tmux` optionally sets an explicit managed session name)
  - `--trace <path>` (`BUDDY_TRACE_FILE` fallback) for JSONL runtime event capture
  - `-v/--verbose` (`-vv`, `-vvv`) for structured diagnostics on stderr
  - `--no-color`
  - `--dangerously-auto-approve` for non-interactive exec guardrail override
- Profile-based config with per-profile provider/protocol/auth mode (`provider`; `completions` vs `responses` vs `anthropic`; `api-key` vs `login`) plus optional OpenAI `reasoning_effort`.
- Login auth startup behavior:
  - missing login credentials are surfaced as warnings (non-fatal startup/model-switch),
  - user guidance points to `/login <provider>` and `buddy login <provider>`.
- Built-in tools:
  - `run_shell`, `read_file`, `write_file`, `fetch_url`, `web_search`, `tmux_capture_pane`, `tmux_send_keys`, `time`
  - tmux lifecycle tools: `tmux_create_session`, `tmux_kill_session`, `tmux_create_pane`, `tmux_kill_pane`
  - every tool call requires a concise `why` rationale; non-shell tool calls render that rationale as a plain indented line, while `run_shell` keeps the same justification in its dedicated approval/shell UI to avoid duplicate console output
  - tmux-aware selectors on shell/capture/send tools (`session`, `pane`) with shared-pane defaulting; blank or whitespace selector fields are treated as unset and resolve to the default shared pane/session
  - explicit missing managed targets: `tmux_capture_pane` auto-recovers to default shared pane with a notice; mutating tmux tools stay strict and return remediation errors
- Multi-target execution for shell/file workflows:
  - local tmux-managed session (default when shell/files are enabled)
  - container tmux-managed session
  - SSH with persistent control socket and optional tmux management
- REPL interaction model:
  - slash commands, autocomplete, multiline editing, history persistence
  - `/model` two-step picker for supported OpenAI reasoning profiles (model, then reasoning effort)
  - `/theme` command with interactive picker, persisted selection, and live preview blocks
  - background prompt tasks with `/ps`, `/kill`, `/timeout`
  - interactive approval flow and `/approve` policy modes
  - session control (`/session ...`) and context compaction (`/compact`)
- Prompt behavior:
  - one template render path with runtime tool/target context
  - static system prompt across turns
  - explicit `--` separators between major prompt sections (system + dynamic request context)
  - explicit system prompt priority sections + end-of-prompt reinforcement
  - structured additive operator-instructions block with conflict policy
  - lightweight planning-before-tools guidance for non-trivial requests
  - request-scoped context annotation before each model request (model metadata + tmux state + annotated history ledger)
  - request-scoped final tail-instruction message appended to every model request (active tmux route, default-vs-explicit pane targeting, shared-shell safety)
  - assistant text that arrives in the same model response as tool calls is streamed to the console instead of being hidden until task completion
  - repeated successful `tmux_capture_pane` calls for the same effective pane/range return an explicit unchanged-state notice instead of re-inserting the same pane snapshot text into context
  - console thinking output ignores intermediate `reasoning_stream` deltas and renders only the final reasoning block to avoid duplicate traces
- Output/rendering behavior:
  - semantic theme-token rendering with built-in `dark`/`light` palettes and optional `[themes.<name>]` overrides
  - startup banner includes build metadata (version, commit hash, build timestamp)
  - assistant response on stdout
  - status/chrome on stderr
  - progress/liveness indicators and structured tool/result rendering
  - `traceui` uses a colorized two-pane alternate-screen viewer with compact event summaries, ~500-character detail previews, `space` expand, arrow/vim navigation, and stream follow/pause behavior
- Token/context behavior:
  - exact usage tracking when provider returns `usage`
  - heuristic context estimation + warnings
  - per-model runtime calibration of token estimates using observed provider usage
  - automatic and manual history compaction
  - context-window lookup from embedded model catalog
- Compatibility behaviors:
  - round-trip provider-specific message extras
  - sanitize malformed/empty assistant turns
  - legacy config/env/session/auth fallback paths with deprecation warnings
  - tool definitions include explicit use/not-use/disambiguation/examples guidance

### Developer-facing

- `ModelClient` trait + `Agent::with_client(...)` for deterministic offline tests.
- `AgentRunner` facade and runtime actor spawn APIs for alternative frontends.
- Typed runtime command/event protocol suitable for non-default UIs.
- Runtime event metadata includes task/session/correlation context plus
  trace-oriented request/response/phase summary events for replay/debugging.
- `RenderSink` abstraction to decouple orchestration from concrete terminal rendering.
- Build/release tooling:
  - compile-time metadata injection via `build.rs`
  - Makefile-first dev/release commands
  - tag-triggered GitHub Actions release artifact publishing

## Detail Docs

- Feature inventory: [feature-catalog.md](feature-catalog.md)
- Current module responsibility map: [module-map.md](module-map.md)
- Runtime actor and API protocol behavior: [runtime-and-protocols.md](runtime-and-protocols.md)
- Prompt architecture: [prompt.md](prompt.md)
- Context management and compaction guarantees: [context-management.md](context-management.md)
- Model/provider behavior matrix: [models.md](models.md)
- Tool contracts and execution backends: [tools-and-execution.md](tools-and-execution.md)
- UI regression harness approach: [../developer/testing-ui.md](../developer/testing-ui.md)
- Observability and trace format: [observability.md](observability.md)

## Near-Term Delivery Track

Current execution plan: [../plans/completed/plan-2026-03-01-feature-requests.md](../plans/completed/plan-2026-03-01-feature-requests.md).

High-level sequence:

1. Freeze architecture/test gates (Milestone 0).
2. Build tmux-based UI regression harness as a prerequisite for terminal UX changes (Milestone 1).
3. Deliver first-class tmux management and targeted tmux routing (Milestone 2).
4. Deliver remaining requested feature slices (build metadata/release flow, init UX, packaging, login soft-fail).

## High-Level Data Flow

1. CLI loads config, applies overrides, and validates active model profile.
2. Execution context is selected (local/container default to managed tmux when shell/files are enabled; SSH uses managed tmux when available, otherwise direct SSH).
3. System prompt is rendered from template with target/tool context.
4. Runtime actor receives prompt commands and drives one active task at a time.
5. Agent loop iterates model calls and tool calls until final assistant message.
6. Runtime events update REPL/UI state, while final assistant text is emitted to stdout.

## Architectural Boundaries

- `Agent` owns the core conversation/tool loop.
- `runtime` owns command/event orchestration, approvals, and session lifecycle commands.
- `tools/execution` owns backend-specific command/file/tmux mechanics.
- `ui`/`tui` own rendering and terminal interaction mechanics.
- `api` owns wire-protocol request/response translation and retry policy.
