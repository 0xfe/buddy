# Architecture

This document describes the current module boundaries after the 2026-02-28 modularization refactor (M1-M6), and where to extend behavior safely.

## Runtime layers

1. CLI and orchestration:
   - `src/main.rs`
   - `src/cli.rs`
   - `src/app/*`
2. Runtime command/event pipeline:
   - `src/runtime/schema.rs` (contracts)
   - `src/runtime/mod.rs` + `src/runtime/{approvals,sessions,tasks}.rs` (actor/execution)
   - `src/cli_event_renderer/*` (runtime event to terminal render mapping)
3. Core agent loop:
   - `src/agent/mod.rs`
   - `src/agent/{events,history,normalization,prompt_aug}.rs`
4. Model transport:
   - `src/api/client/{mod,auth,retry,transport}.rs`
   - `src/api/completions.rs`
   - `src/api/responses/{mod,request_builder,response_parser,sse_parser}.rs`
   - `src/api/policy.rs`
5. Config/auth/session primitives:
   - `src/config/*`
   - `src/auth/*`
   - `src/session.rs`
   - `src/tokens.rs`
   - `src/types.rs`
   - `src/error.rs`
6. Tooling and execution:
   - `src/tools/mod.rs` + built-in tool modules
   - `src/tools/execution/*` (local/container/ssh/tmux backends)
7. Terminal UI:
   - `src/tui/*`
   - `src/render.rs` (compat facade for renderer contracts)
   - `src/repl_support/*` (shared REPL/runtime task-state helpers)

## Source tree (high-level)

```text
src/
  main.rs
  cli.rs
  app/
  agent/
  api/
  auth/
  config/
  runtime/
  cli_event_renderer/
  repl_support/
  tools/
    execution/
  tui/
  render.rs
  session.rs
  tokens.rs
  types.rs
  error.rs
  prompt.rs
  preflight.rs
  templates/
```

## Extension points

### Add a new tool

1. Implement `Tool` in a new `src/tools/<name>.rs`.
2. Register it in `main.rs` via `ToolRegistry`.
3. If it needs shell/container/ssh/tmux execution, route through `ExecutionContext` (`src/tools/execution/mod.rs`) instead of direct process calls.
4. Add unit tests in the tool module.

### Add a new model provider behavior

1. Keep provider-agnostic request/response normalization in `src/api/completions.rs` or `src/api/responses/*`.
2. Add provider policy checks to `src/api/policy.rs`.
3. Keep auth resolution in `src/api/client/auth.rs` and provider-login flow in `src/auth/*`.
4. Add regression fixtures in `src/api/mod.rs` tests to preserve normalized internal semantics.

### Add or change REPL/runtime UX behavior

1. Prefer new runtime events in `src/runtime/schema.rs` over direct renderer calls from orchestration.
2. Handle event-to-UI mapping in `src/cli_event_renderer/handlers/*`.
3. Keep text styling/layout details in `src/tui/*`.
4. Keep prompt/task helper state in `src/repl_support/*`.

### Add config fields

1. Define schema in `src/config/types.rs`.
2. Add defaults in `src/config/defaults.rs`.
3. Wire parsing/resolution in `src/config/{loader,resolve,env}.rs`.
4. Update compiled template `src/templates/buddy.toml`.
5. Add/adjust config characterization tests in `src/config/mod.rs`.

## Invariants to preserve

- CLI and config compatibility:
  - Existing subcommands/flags should remain backward compatible unless a migration is documented.
  - Precedence remains: CLI overrides loaded config; env overrides are applied during config resolution.
- Runtime event contract:
  - Runtime actor emits structured task/model/tool/session events consumed by REPL and alternate frontends.
- Tool safety:
  - `run_shell`/`send-keys` require safety metadata (`risk`, `mutation`, `privesc`, `why`).
  - Tool outputs are bounded and wrapped with `harness_timestamp`.
- Tmux behavior:
  - Managed session defaults to `buddy-<agent.name>`.
  - Managed pane/window target is `shared` for execution and capture.

## Testing expectations

- Fast gate for any architectural edit: `cargo test`.
- Parser property coverage when touching parsing code: `cargo test --features fuzz-tests`.
- Live model/protocol smoke checks (explicit, not default): `cargo test --test model_regression -- --ignored --nocapture`.
