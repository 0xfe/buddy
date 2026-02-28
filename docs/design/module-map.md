# Module Map

This is the current architectural map for `src/`, grouped by responsibility.

For a high-level overview, see [DESIGN.md](../../DESIGN.md).

## Entrypoints and App Wiring

- `src/main.rs`
  - binary entrypoint (`tokio::main`)
  - parses CLI and delegates to app runner
- `src/cli.rs`
  - clap argument model (global flags + subcommands)
- `src/app/`
  - top-level flow orchestration (`entry.rs`)
  - mode-specific loops (`exec_mode.rs`, `repl_mode.rs`)
  - shared REPL command/task/approval/startup helpers

## Core Agent and Runtime

- `src/agent/`
  - `Agent` core loop, tool turn handling, cancellation, session snapshotting
  - history compaction and context-budget enforcement
  - provider message normalization and reasoning extraction
  - runtime/UI event emission bridges
- `src/runtime/`
  - typed runtime command/event schema
  - runtime actor (single active prompt task orchestration)
  - approval mediation, session commands, task spawning helpers
- `src/repl/`
  - reusable REPL policy/task/tool-payload utilities shared by app/runtime UI layers

## API and Protocol Layer

- `src/api/`
  - `ApiClient` orchestration facade (`client/`)
  - protocol modules:
    - `completions.rs` (`/chat/completions`)
    - `responses/` (`/responses` request build + parse + SSE handling)
  - policy module for provider/runtime protocol toggles
  - retry/backoff and diagnostic hinting

## Auth and Identity

- `src/auth/`
  - OpenAI device auth flow + token refresh
  - provider detection and runtime base URL helpers
  - encrypted token store with provider-scoped records and legacy fallback
  - browser-open helper for login UX

## Config and Preflight

- `src/config/`
  - config schema types
  - source discovery (`buddy.toml` + legacy fallbacks)
  - profile resolution and API key source validation
  - env overrides and deprecation diagnostics
  - default config initialization (`buddy init`)
  - model profile selection helpers
- `src/preflight.rs`
  - active profile readiness checks (URL/model/auth sanity)

## Tools and Execution Stack

- `src/tools/mod.rs`
  - async `Tool` trait and `ToolRegistry`
  - `ToolContext` stream events
- Built-in tool modules:
  - `shell.rs`, `files.rs`, `fetch.rs`, `search.rs`
  - `capture_pane.rs`, `send_keys.rs`, `time.rs`
  - `result_envelope.rs` shared JSON wrapper
- `src/tools/execution/`
  - backend-neutral execution context
  - local/container/ssh backend implementations
  - file I/O and process helpers
- `src/tmux/`
  - managed pane/session setup
  - capture/send/run prompt-marker plumbing

## UI and Terminal Rendering

- `src/ui/`
  - stable rendering traits and terminal facade re-exports
  - runtime event rendering adapter and handlers
- `src/tui/`
  - concrete terminal renderer
  - interactive editor/input loop
  - slash-command metadata and autocomplete
  - progress indicators, markdown/highlight formatting, UI constants

## Shared Domain Primitives

- `src/types.rs`
  - chat request/response model and tool call definitions
- `src/tokens.rs`
  - token counters, estimation heuristics, context catalog lookup
- `src/session.rs`
  - persistent session store under `.buddyx` (legacy `.agentx` fallback)
- `src/prompt.rs`
  - system prompt templating
- `src/error.rs`
  - hand-written error enums and conversion hierarchy
- `src/textutil.rs`
  - shared truncation/preview helpers
- `src/lib.rs`
  - crate public module surface

## Templates and Embedded Data

- `src/templates/buddy.toml`
  - default global config written by `buddy init`
- `src/templates/system_prompt.template`
  - built-in system prompt template
- `src/templates/models.toml`
  - model context-window rule catalog
