# Backend Dedup And Modularization Plan (2026-03-02)

## Status

- Program status: Completed
- Current focus: All milestones complete
- Next step: none
- Blockers: none

## Scope

Refactor duplicated code across local/ssh/container execution paths and factor model-specific behavior out of generic app code into cohesive interfaces, without changing Buddy behavior.

## Non-Goals

1. Any user-visible feature changes.
2. Any API behavior changes beyond preserving existing behavior through cleaner structure.
3. Large runtime architecture changes outside dedup/modularization scope.

## Milestone Board

- [x] M1: Shared backend helpers for duplicated tmux execution-path code
- [x] M2: Consolidate tmux capture/wait duplication with generic internal helpers
- [x] M3: Move model-specific built-in-tool policy out of app orchestration into API interface
- [x] M4: Verification, docs sync, and cleanup

## M1: Shared backend helpers for duplicated tmux execution-path code

### Tasks

1. Add `src/tools/execution/backend/common.rs` with:
   - selector builders for `capture-pane`/`send-keys`
   - fallback detection helper (`tmux target not found` parse fallback)
   - shared parser wrappers for create/kill session/pane outputs and send-keys response formatting
2. Update local/ssh/container backend modules to use shared helpers.
3. Preserve all existing error text and behavior.

### Acceptance

1. No behavior changes in backend tests.
2. Duplicate helper code removed from all three backend modules.

### Tests

1. `cargo test`
2. targeted backend tests if needed

## M2: Consolidate tmux capture/wait duplication with generic internal helpers

### Tasks

1. In `src/tmux/capture.rs`, introduce internal reusable async helpers for:
   - run capture with alternate-screen fallback
   - run full-history capture
   - wait for prompt marker polling loop
2. Rewire local/ssh/container capture + wait functions to those helpers.
3. Keep existing public function signatures and error messages unchanged.

### Acceptance

1. Existing capture tests remain green.
2. Duplication in capture/wait flows is reduced significantly.

### Tests

1. `cargo test tmux::capture:: -- --nocapture`
2. `cargo test`

## M3: Move model-specific built-in-tool policy out of app orchestration into API interface

### Tasks

1. Add a public API helper that returns default built-in tool names from API policy inputs.
2. Update app code to call this helper instead of hardcoded OpenAI model-family checks.
3. Keep policy behavior identical to current API request logic.

### Acceptance

1. No duplicated model-family heuristics in `src/app/entry.rs`.
2. Built-in tool enable/disable behavior remains unchanged.

### Tests

1. existing app entry tests for built-in tools
2. `cargo test`

## M4: Verification, docs sync, and cleanup

### Tasks

1. Run full gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
2. Update plan log with completed milestones and verification results.
3. Move plan to `docs/plans/completed/` and update `docs/plans/README.md`.
4. Commit with clear message.

### Acceptance

1. All checks pass.
2. Plan closed and archived.

## Execution Log

- 2026-03-02: Plan created and scoped to dedup/modularization with no behavior changes.
- 2026-03-02: M1 complete. Added `src/tools/execution/backend/common.rs` and rewired local/ssh/container backends to shared selector/fallback/parse helpers.
- 2026-03-02: M2 complete. Refactored `src/tmux/capture.rs` to shared internal capture-with-fallback and prompt-wait helpers across local/ssh/container.
- 2026-03-02: M3 complete. Added `api::default_builtin_tool_names(...)` and removed duplicated model-family heuristic from `src/app/entry.rs`.
- 2026-03-02: M4 verification complete. Passed `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` (438 passed, 0 failed, 4 ignored).
