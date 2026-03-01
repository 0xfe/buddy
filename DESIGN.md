# Design

Buddy is a Rust terminal agent for OpenAI-compatible APIs. This document is intentionally high-level and human-readable.

Detailed operational and protocol behavior now lives under `docs/design/`.

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

Detailed inventory: [docs/design/feature-catalog.md](docs/design/feature-catalog.md).

### User-facing

- CLI modes and commands:
  - REPL (`buddy`)
  - one-shot exec (`buddy exec <prompt>`)
  - session resume (`buddy resume <id|--last>`)
  - setup/login (`buddy init`, `buddy login`)
- Global targeting and runtime flags:
  - config/model/base-url overrides
  - `--ssh`, `--container`, `--tmux [session]`
  - `--no-color`
  - `--dangerously-auto-approve` for non-interactive exec guardrail override
- Profile-based config with per-profile protocol/auth mode (`completions` vs `responses`; `api-key` vs `login`).
- Built-in tools:
  - `run_shell`, `read_file`, `write_file`, `fetch_url`, `web_search`, `capture-pane`, `send-keys`, `time`
  - tmux lifecycle tools: `tmux-create-session`, `tmux-kill-session`, `tmux-create-pane`, `tmux-kill-pane`
  - tmux-aware selectors on shell/capture/send-keys (`session`, `pane`) with shared-pane defaulting
- Multi-target execution for shell/file workflows:
  - local
  - local tmux-managed session
  - container
  - container tmux-managed session
  - SSH with persistent control socket and optional tmux management
- REPL interaction model:
  - slash commands, autocomplete, multiline editing, history persistence
  - `/theme` command with interactive picker, persisted selection, and live preview blocks
  - background prompt tasks with `/ps`, `/kill`, `/timeout`
  - interactive approval flow and `/approve` policy modes
  - session control (`/session ...`) and context compaction (`/compact`)
- Prompt behavior:
  - one template render path with runtime tool/target context
  - dynamic tmux screenshot block refresh before each model request when available
- Output/rendering behavior:
  - semantic theme-token rendering with built-in `dark`/`light` palettes and optional `[themes.<name>]` overrides
  - assistant response on stdout
  - status/chrome on stderr
  - progress/liveness indicators and structured tool/result rendering
- Token/context behavior:
  - exact usage tracking when provider returns `usage`
  - heuristic context estimation + warnings
  - automatic and manual history compaction
  - context-window lookup from embedded model catalog
- Compatibility behaviors:
  - round-trip provider-specific message extras
  - sanitize malformed/empty assistant turns
  - legacy config/env/session/auth fallback paths with deprecation warnings

### Developer-facing

- `ModelClient` trait + `Agent::with_client(...)` for deterministic offline tests.
- `AgentRunner` facade and runtime actor spawn APIs for alternative frontends.
- Typed runtime command/event protocol suitable for non-default UIs.
- `RenderSink` abstraction to decouple orchestration from concrete terminal rendering.

## Detail Docs

- Feature inventory: [docs/design/feature-catalog.md](docs/design/feature-catalog.md)
- Current module responsibility map: [docs/design/module-map.md](docs/design/module-map.md)
- Runtime actor and API protocol behavior: [docs/design/runtime-and-protocols.md](docs/design/runtime-and-protocols.md)
- Tool contracts and execution backends: [docs/design/tools-and-execution.md](docs/design/tools-and-execution.md)
- UI regression harness approach: [docs/testing-ui.md](docs/testing-ui.md)

## Near-Term Delivery Track

Current execution plan: [docs/plans/2026-03-01-feature-requests.md](docs/plans/2026-03-01-feature-requests.md).

High-level sequence:

1. Freeze architecture/test gates (Milestone 0).
2. Build tmux-based UI regression harness as a prerequisite for terminal UX changes (Milestone 1).
3. Deliver first-class tmux management and targeted tmux routing (Milestone 2).
4. Deliver remaining requested feature slices (build metadata/release flow, init UX, packaging, login soft-fail).

## High-Level Data Flow

1. CLI loads config, applies overrides, and validates active model profile.
2. Execution context is selected (local/ssh/container, optionally tmux-backed).
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
