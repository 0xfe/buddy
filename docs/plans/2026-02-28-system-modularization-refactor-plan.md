# System Modularization Refactor Plan (2026-02-28)

## Status

- Program status: Active
- Current milestone: `M1` (CLI/runtime orchestration decomposition)
- Current task: `M1.3` (extract REPL loop dispatch and reduce remaining orchestration in `main.rs`)
- Next steps:
  1. Land `M1.3` by isolating REPL loop command routing into a dedicated module.
  2. Land `M1.4` by migrating/refining moved helper tests into new module-local suites.
  3. Re-run `cargo test` and continue milestone-by-milestone commit discipline.

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

### Execution backend

1. `src/tools/execution/mod.rs` (public facade)
2. `src/tools/execution/types.rs`
3. `src/tools/execution/contracts.rs`
4. `src/tools/execution/process.rs`
5. `src/tools/execution/backend/{local,container,ssh,file_io}.rs`
6. `src/tools/execution/tmux/{pane,prompt,capture,send_keys,run}.rs`

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
- [ ] M1.3 Extract REPL loop dispatch into `src/app/repl_loop.rs`, remove duplicate slash-command routing code paths. — `<commit-id>`
- [ ] M1.4 Migrate/expand unit tests from `main.rs` into new modules and keep binary tests passing. — `<commit-id>`
- Acceptance gate:
  1. `main.rs` reduced to CLI wiring + high-level orchestration only.
  2. All existing main-level behaviors preserved (model/session/login/approval flows).
  3. `cargo test` green.

## M2: Split `repl_support.rs` and `cli_event_renderer.rs`

- [ ] M2.1 Split `repl_support.rs` into `tool_payload`, `task_state`, `policy` submodules with re-export facade. — `<commit-id>`
- [ ] M2.2 Split `cli_event_renderer.rs` into per-event handler modules with single reducer entrypoint. — `<commit-id>`
- [ ] M2.3 Add reducer-focused tests (approval transitions, suppression filters, tool-result rendering branches). — `<commit-id>`
- Acceptance gate:
  1. No logic drift in runtime event rendering.
  2. Stable function boundaries for future frontends/runtime adapters.
  3. `cargo test` green.

## M3: Split `tools/execution.rs` into backend/process/tmux modules

- [ ] M3.1 Convert `src/tools/execution.rs` into `src/tools/execution/mod.rs` facade and extract shared types/contracts. — `<commit-id>`
- [ ] M3.2 Extract process and file I/O helper layers. — `<commit-id>`
- [ ] M3.3 Extract tmux modules (`pane`, `prompt`, `capture`, `send_keys`, `run`) with existing behavior preserved. — `<commit-id>`
- [ ] M3.4 Extract backend modules (`local`, `container`, `ssh`) including lifecycle cleanup logic and test hooks. — `<commit-id>`
- [ ] M3.5 Relocate/augment execution regression tests (tmux creation/reuse, no-wait constraints, ssh cleanup, podman/docker differences). — `<commit-id>`
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

## Feedback Alignment Notes

1. No major disagreement with the remediation priorities; this plan intentionally keeps behavior-stability and test-first sequencing as top constraints.
2. The only explicit policy choice is to preserve currently effective runtime precedence (`CLI > loaded config`) even if comments/docs in older locations implied otherwise.

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
