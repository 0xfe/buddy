# Refactor Playbook

This playbook captures the modularization strategy used in the 2026-02-28 refactor program (M1-M6) and defines how to keep the architecture coherent as the codebase evolves.

## Goals

1. Keep behavior stable while improving readability and testability.
2. Keep module responsibilities narrow and explicit.
3. Preserve runtime event and CLI behavior contracts for existing users.
4. Make it practical to add alternate frontends by consuming runtime events.

## Refactor rationale

The old layout concentrated too much behavior in large single files (`main.rs`, `agent.rs`, `runtime.rs`, `config.rs`, `auth.rs`, `tools/execution.rs`, `api/client.rs`, `api/responses.rs`), which increased risk when changing unrelated concerns.

The modularization split those files into cohesive submodules:

- Orchestration helpers moved under `src/app/*`.
- Runtime contracts and actor concerns split under `src/runtime/*`.
- Agent loop concerns split under `src/agent/*`.
- Config/auth split under `src/config/*` and `src/auth/*`.
- Execution backends split under `src/tools/execution/*`.
- API transport/auth/retry/protocol parsing split under `src/api/*`.

## Boundary rules

### 1) Orchestration (`src/main.rs`, `src/app/*`)

- `main.rs` should stay focused on CLI wiring and high-level startup branch logic.
- Place command-specific behavior in `src/app/commands/*`.
- Place long-running task/approval orchestration in `src/app/{tasks,approval}.rs`.
- Do not embed tool-implementation or provider-protocol logic in `app` modules.

### 2) Runtime/event surface (`src/runtime/*`, `src/ui/runtime/*`, `src/repl/*`)

- Add new runtime semantics via `src/runtime/schema.rs` first.
- Keep actor flow in `src/runtime/mod.rs`; helper logic belongs in focused submodules (`approvals`, `sessions`, `tasks`).
- Convert runtime events to terminal output in `src/ui/runtime/*` only.
- Keep task-state/policy parsing helpers in `src/repl/*` so they are reusable from REPL and renderer layers.

### 3) Agent and model transport (`src/agent/*`, `src/api/*`)

- `src/agent/mod.rs` is orchestration; state transforms belong in `history`, `normalization`, `prompt_aug`, and `events`.
- Keep transport/auth/retry concerns in `src/api/client/*`.
- Keep protocol-specific payload/parsing logic in `src/api/completions.rs` and `src/api/responses/*`.
- Provider quirks belong in `src/api/policy.rs` (not in generic parsers).

### 4) Config/auth (`src/config/*`, `src/auth/*`)

- Types/defaults belong in `types.rs`/`defaults.rs`; resolution and I/O in dedicated loader modules.
- Keep explicit precedence behavior stable:
  - env overrides in config loader;
  - CLI overrides in `src/app/entry.rs` (`apply_cli_overrides`).
- Keep auth storage/provider logic provider-scoped and profile-agnostic where possible.

### 5) Tools/execution (`src/tools/*`, `src/tools/execution/*`)

- Keep JSON schema + argument validation close to each tool.
- Route shell/files target execution through `ExecutionContext` rather than ad-hoc process spawning.
- Tmux/local/container/ssh behavior changes must include execution tests covering all affected targets.

## Change workflow

1. Characterize current behavior with tests before moving code.
2. Move one cohesive slice at a time (single responsibility).
3. Keep old public facades while call sites migrate.
4. Run `cargo test` after each slice.
5. Commit each slice separately with a behavior-preserving message.
6. Update docs (`DESIGN.md`, `README.md`, `docs/architecture.md`, `ai-state.md`) in the same milestone.

## Review checklist for refactor PRs

- Does each new module own one clear concern?
- Are there any new cross-layer dependencies that violate boundaries?
- Did runtime events or CLI output semantics change?
- Are tests updated to cover moved behavior?
- Are user-facing docs updated for path/name changes?

## Known follow-up work

- Continue tightening the `ui::terminal` facade so more `tui/*` internals become implementation-only details.
