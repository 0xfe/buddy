# System Modularization Refactor Plan (2026-02-28)

## Status

- Program status: Active
- Current milestone: `M4` (split `config.rs` and `auth.rs`)
- Current task: `M4.1` (move config types/defaults into `config/{types,defaults}.rs`)
- Next steps:
  1. Land `M4.1` with behavior-preserving config type/default extraction.
  2. Continue `M4.2` + `M4.3` with clear module boundaries and stable re-exports.
  3. Keep `cargo test` green after each M4 task slice.

## Maintainer Instructions

1. Keep this **Status** section current.
2. Commit between tasks/milestones; include commit IDs in both the task checklist and execution log.
3. Treat this as a behavior-preserving refactor program. If behavior must change, document why and add explicit regression tests.
4. Run tests after each task slice (`cargo test` minimum; targeted tests while iterating).
5. Append-only execution log; do not delete previous entries.

## Objective

Refactor the entire codebase (not just `main.rs`) into cohesive, composable, and testable modules while preserving current runtime behavior and UX semantics.

## Design Constraints

1. Preserve external behavior and CLI compatibility for existing commands and flags.
2. Preserve config precedence semantics as currently implemented (`CLI overrides loaded config`; env/runtime precedence within config loader).
3. Keep public crate API compatibility unless a milestone explicitly declares a breaking change.
4. Prefer explicit interfaces and narrow modules over broad utility buckets.
5. Maintain or improve test coverage at each extraction step.

## Current Hotspots

1. `src/main.rs` (2593 LOC): bootstrap, REPL routing, session/model/login flows, approval UX, liveness/task orchestration, rendering helpers, and tests.
2. `src/tools/execution.rs` (2750 LOC): local/container/ssh backends, tmux lifecycle, command transport, parsing, and tests.
3. `src/agent.rs` (1799 LOC): agent loop, event plumbing, prompt augmentation, compaction, normalization.
4. `src/runtime.rs` (1690 LOC): runtime schema + actor + approvals + session/model command handling.
5. `src/config.rs` (1561 LOC) and `src/auth.rs` (1090 LOC): large multi-responsibility modules.
6. `src/api/client.rs` + `src/api/responses.rs`: transport/auth/retry/protocol parsing mixed.
7. UI rendering pipeline is split across `src/render.rs`, `src/tui/*`, `src/cli_event_renderer/*`, and `src/repl_support/*`, which leaks presentation/state concerns across too many modules.
8. Tmux lifecycle logic is primarily in `src/tools/execution/tmux/*`, but higher-level runtime/startup and tool-facing call sites still reason about tmux concepts directly instead of using one shared tmux abstraction.

## Target Module Topology

### Binary orchestration (`src/main.rs` + local modules)

1. `src/main.rs`: thin wiring only.
2. `src/app/boot.rs`: startup/bootstrap and execution target wiring.
3. `src/app/commands/`: `/model`, `/session`, `/login`, `/status`, `/context` handlers.
4. `src/app/approval.rs`: approval prompt rendering + decision flow.
5. `src/app/tasks.rs`: background task lifecycle + timeout/cancel logic.
6. `src/app/startup.rs`: startup/session banner rendering.

### REPL/runtime helpers

1. `src/repl_support/tool_payload.rs`
2. `src/repl_support/task_state.rs`
3. `src/repl_support/policy.rs`
4. `src/cli_event_renderer/handlers/*.rs` with one public reducer entrypoint.

### UI and REPL boundaries (new consolidation target)

1. `src/ui/mod.rs` as the single presentation facade for terminal rendering.
2. `src/ui/render_sink.rs` for `RenderSink`, progress handles, and shared rendering contracts.
3. `src/ui/terminal/*` for low-level styling/markdown/spinner/input helpers (current `tui/*` internals).
4. `src/ui/runtime/*` for runtime-event-to-render mapping (current `cli_event_renderer/*` handlers/reducer).
5. `src/repl/mod.rs` for REPL state machine and command dispatch concerns currently spread across `repl_support` and `app/repl_loop`.
6. Keep `src/render.rs` only as a temporary backward-compat shim during migration, then remove it once call sites are moved.

### Execution backend

1. `src/tools/execution/mod.rs` (public facade)
2. `src/tools/execution/types.rs`
3. `src/tools/execution/contracts.rs`
4. `src/tools/execution/process.rs`
5. `src/tools/execution/backend/{local,container,ssh,file_io}.rs`
6. `src/tools/execution/tmux/{pane,prompt,capture,send_keys,run}.rs`

### Shared tmux infrastructure (new consolidation target)

1. `src/tmux/mod.rs` as a neutral tmux domain API (session/pane discovery, ensure/create, capture, send-keys, prompt readiness).
2. `src/tmux/types.rs` for tmux target/session/pane identifiers and errors.
3. `src/tmux/driver.rs` for command transport adapters (local/container/ssh).
4. `src/tmux/workflow.rs` for behavior-level flows (create-or-reuse `shared` pane, attach metadata, busy-pane checks, startup capture).
5. `src/tools/execution/tmux/*` becomes a thin adapter layer (or is folded into `src/tmux/*`) so tmux behavior is defined once and reused by tools and startup/runtime flows.

### Config/auth

1. `src/config/mod.rs` + submodules (`types`, `loader`, `resolve`, `env`, `sources`, `init`, `defaults`, `diagnostics`, `selector`).
2. `src/auth/mod.rs` + submodules (`types`, `provider`, `openai`, `store`, `crypto`, `browser`, `error`).
3. Shared path resolution extracted into `src/paths.rs` (re-exported where needed).

### Agent/runtime/api

1. `src/agent/{core.rs,events.rs,prompt_aug.rs,history.rs,reasoning.rs,message_norm.rs}`.
2. `src/runtime/{schema.rs,actor.rs,approvals.rs,sessions.rs,tasks.rs}`.
3. `src/api/{transport.rs,auth.rs,retry.rs,adapters/{completions,responses},responses/{request_builder,sse_parser,response_parser}}`.

## Milestones and Gates

## M0: Baseline Characterization (safety net)

- [ ] M0.1 Add/expand characterization tests for fragile behavior seams:
  - task/event terminal semantics;
  - approval policy parsing and timeout behavior;
  - config/auth precedence and migration edge cases.
- [ ] M0.2 Capture current module line counts + coupling notes in `ai-state.md`.
- [ ] M0.3 Confirm baseline `cargo test` green before structural moves.
- Acceptance gate:
  1. No refactor starts without baseline green tests and documented behavior assumptions.
- Planned commit:
  1. `test(refactor): add characterization coverage for modularization baseline` — `<commit-id>`

## M1: Decompose `main.rs` into app modules

- [x] M1.1 Extract model/session/startup helper functions into `src/app/commands/{model,session}.rs` and `src/app/startup.rs` (no behavior change). — `84af672`
- [x] M1.2 Extract approval and task lifecycle helpers into `src/app/{approval,tasks}.rs`; keep runtime command semantics unchanged. — `84af672`
- [x] M1.3 Extract REPL loop dispatch into `src/app/repl_loop.rs`, remove duplicate slash-command routing code paths. — `5cceba8`
- [x] M1.4 Migrate/expand unit tests from `main.rs` into new modules and keep binary tests passing. — `5cceba8`
- Acceptance gate:
  1. `main.rs` reduced to CLI wiring + high-level orchestration only.
  2. All existing main-level behaviors preserved (model/session/login/approval flows).
  3. `cargo test` green.

## M2: Split `repl_support.rs` and `cli_event_renderer.rs`

- [x] M2.1 Split `repl_support.rs` into `tool_payload`, `task_state`, `policy` submodules with re-export facade. — `d3be74e`
- [x] M2.2 Split `cli_event_renderer.rs` into per-event handler modules with single reducer entrypoint. — `794741f`
- [x] M2.3 Add reducer-focused tests (approval transitions, suppression filters, tool-result rendering branches). — `abcf21c`
- Acceptance gate:
  1. No logic drift in runtime event rendering.
  2. Stable function boundaries for future frontends/runtime adapters.
  3. `cargo test` green.

## M3: Split `tools/execution.rs` into backend/process/tmux modules

- [x] M3.1 Convert `src/tools/execution.rs` into `src/tools/execution/mod.rs` facade and extract shared types/contracts. — `b923755`
- [x] M3.2 Extract process and file I/O helper layers. — `2a46328`
- [x] M3.3 Extract tmux modules (`pane`, `prompt`, `capture`, `send_keys`, `run`) with existing behavior preserved. — `93fb3f2`
- [x] M3.4 Extract backend modules (`local`, `container`, `ssh`) including lifecycle cleanup logic and test hooks. — `bfb153c`
- [x] M3.5 Relocate/augment execution regression tests (tmux creation/reuse, no-wait constraints, ssh cleanup, podman/docker differences). — `21a7a6a`
- Acceptance gate:
  1. Public `ExecutionContext` API unchanged.
  2. Tmux/session/pane behaviors unchanged across local/container/ssh.
  3. `cargo test` green.

## M4: Split `config.rs` and `auth.rs`

- [ ] M4.1 Move config types/defaults into `config/{types,defaults}.rs`; keep loader behavior unchanged. — `<commit-id>`
- [ ] M4.2 Extract config source/env/resolve/init/selector logic into dedicated modules; keep public re-exports. — `<commit-id>`
- [ ] M4.3 Split auth provider/openai/store/crypto/browser/error modules with same public API. — `<commit-id>`
- [ ] M4.4 Add precedence and migration characterization tests (including legacy compatibility paths). — `<commit-id>`
- Acceptance gate:
  1. Config/auth behavior and precedence preserved.
  2. Login/token flow and encrypted storage behavior preserved.
  3. `cargo test` green.

## M5: Split `agent.rs` and `runtime.rs`

- [ ] M5.1 Extract agent event plumbing and prompt augmentation modules. — `<commit-id>`
- [ ] M5.2 Extract agent history/compaction and normalization modules. — `<commit-id>`
- [ ] M5.3 Split runtime schema from actor implementation and isolate approvals/sessions/task handlers. — `<commit-id>`
- [ ] M5.4 Resolve known runtime-event contract risks (duplicate failure/start events) with explicit tests. — `<commit-id>`
- Acceptance gate:
  1. Cleaner event contracts for alternate frontends.
  2. No runtime loop deadlocks from agent lock contention.
  3. `cargo test` green.

## M6: Split API transport and protocol adapters

- [ ] M6.1 Extract HTTP transport + retry + auth resolution from `api/client.rs`. — `<commit-id>`
- [ ] M6.2 Split Responses API request builder and parsers (`sse_parser`, `response_parser`). — `<commit-id>`
- [ ] M6.3 Add protocol fixture tests ensuring completions/responses normalize to equivalent internal structures where expected. — `<commit-id>`
- Acceptance gate:
  1. Provider compatibility and retry behavior preserved.
  2. SSE parsing behavior unchanged or improved with explicit regression fixtures.
  3. `cargo test` green.

## M7: Documentation and architectural clean-up

- [ ] M7.1 Update `DESIGN.md` module map + features for final structure. — `<commit-id>`
- [ ] M7.2 Update `README.md`, `docs/architecture.md`, and `ai-state.md` with new layout and extension points. — `<commit-id>`
- [ ] M7.3 Add `docs/refactor-playbook.md` summarizing module boundaries and refactor rationale. — `<commit-id>`
- Acceptance gate:
  1. Docs match code structure.
  2. New contributors can navigate module boundaries without reverse-engineering.

## M8: UI/REPL/Tmux Consolidation

- [ ] M8.1 Introduce `src/ui` facade and migrate `RenderSink` + renderer contracts from `src/render.rs`; keep `render.rs` as compat shim during transition. — `<commit-id>`
- [ ] M8.2 Move runtime-event rendering reducer/handlers (`cli_event_renderer`) under `src/ui/runtime/*` and isolate pure reducer tests from terminal styling tests. — `<commit-id>`
- [ ] M8.3 Move REPL state/policy/task helpers (`repl_support`) into `src/repl/*` with explicit interfaces consumed by `app/repl_loop`. — `<commit-id>`
- [ ] M8.4 Extract shared tmux domain module (`src/tmux/*`) and route startup/runtime/tool tmux operations through one API surface. — `<commit-id>`
- [ ] M8.5 Remove migration shims (`src/render.rs` and legacy module aliases) once all internal call sites are on the new boundaries. — `<commit-id>`
- Acceptance gate:
  1. `main.rs` no longer imports mixed rendering/state helper modules directly (`render` + `tui` + `cli_event_renderer` + `repl_support`); it depends on `ui` and `repl` facades only.
  2. Tmux behavior (session reuse/create, shared pane targeting, attach metadata, capture/send-keys semantics) is defined once and reused uniformly.
  3. Existing rendering/approval/task UX behavior remains stable with regression coverage.
  4. `cargo test` green.

## Feedback Alignment Notes

1. No major disagreement with the remediation priorities; this plan intentionally keeps behavior-stability and test-first sequencing as top constraints.
2. The only explicit policy choice is to preserve currently effective runtime precedence (`CLI > loaded config`) even if comments/docs in older locations implied otherwise.
3. Additional architecture feedback accepted: renderer/TUI/REPL and tmux concerns will be consolidated under explicit facades (`ui`, `repl`, `tmux`) in `M8` after core decomposition milestones.

## Test Strategy

1. Fast inner loop:
  - targeted module tests during extraction.
2. Merge gate per task:
  - `cargo test`.
3. Critical path checks per milestone:
  - interactive runtime event tests,
  - execution backend tests (tmux/ssh/container),
  - config/auth precedence and migration tests.

## Execution Log

- 2026-02-28: Created system-wide modularization plan after auditing `main`, `execution`, `agent`, `runtime`, `config`, `auth`, and `api` modules. M1 started. Commit: `84af672`.
- 2026-02-28: Completed `M1.1` + `M1.2` in one behavior-preserving slice: extracted `src/app/commands/{model,session}.rs`, `src/app/startup.rs`, `src/app/approval.rs`, and `src/app/tasks.rs`; rewired `main.rs` to use new modules. `main.rs` reduced from 2593 LOC to 1825 LOC. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `84af672`.
- 2026-02-28: Completed `M1.3` + `M1.4`: added `src/app/repl_loop.rs` to centralize shared slash-command dispatch for normal/approval REPL paths (`/ps`, `/kill`, `/timeout`, `/approve`), removed duplicated routing logic in `main.rs`, and migrated command/helper tests from `main.rs` into `src/app/{approval,startup,commands/*}.rs`. `main.rs` reduced further from 1825 LOC to 1624 LOC. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `5cceba8`.
- 2026-02-28: Completed `M2.1`: split `repl_support` into `policy`, `task_state`, and `tool_payload` modules with a re-exporting facade to preserve call sites. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `d3be74e`.
- 2026-02-28: Completed `M2.2`: converted `cli_event_renderer` into a reducer + per-event handlers (`warning`, `session`, `task`, `model`, `tool`, `metrics`) with unchanged runtime event semantics. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `794741f`.
- 2026-02-28: Completed `M2.3`: added reducer-focused tests for approval state transitions, transient warning suppression, and tool-result branch rendering (including run_shell suppression behavior). Validation: `cargo fmt`, `cargo test -q` (green). Commit: `abcf21c`.
- 2026-02-28: Completed `M3.1`: converted execution into `src/tools/execution/mod.rs` with extracted `types.rs` and `contracts.rs` facade boundaries. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `b923755`.
- 2026-02-28: Completed `M3.2`: extracted `process.rs` and `file_io.rs` for command execution/wait behavior and command-backed file operations. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `2a46328`.
- 2026-02-28: Completed `M3.3`: moved tmux logic into `tmux/{pane,prompt,capture,send_keys,run}.rs` and kept behavior stable across local/container/ssh flows. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `93fb3f2`.
- 2026-02-28: Completed `M3.4`: moved backend implementations to `backend/{local,container,ssh}.rs`, including ssh lifecycle cleanup/test hooks and local tmux safety helpers. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `bfb153c`.
- 2026-02-28: Completed `M3.5`: relocated execution regression tests into process/backend/tmux modules and added explicit no-wait constraint tests for non-tmux container/ssh backends. Validation: `cargo fmt`, `cargo test -q` (green). Commit: `21a7a6a`.
- 2026-02-28: Incorporated architecture feedback into this plan: documented UI/REPL boundary smell (`render`/`tui`/`cli_event_renderer`/`repl_support`) and cross-cutting tmux abstraction needs, then added `M8` with concrete extraction gates (`ui`, `repl`, `tmux`). Validation: doc-only planning update. Commit: `21f1fcb`.
