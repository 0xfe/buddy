# Streaming Runtime Architecture Plan (2026-02-27)

## Status

- Current milestone: `S3` (tool context + streaming tool outputs)
- Current status: `S2` complete (integrated with `docs/plans/2026-02-27-claude-feedback-remediation-plan.md`)
- Next steps:
  1. Introduce `ToolContext` and compatibility adapter path.
  2. Start incremental tool streaming with `run_shell` as first tool.
  3. Keep renderer parity while moving to event-driven incremental updates.
  4. Feed `S3` checkpoints back into remediation Milestone 6 gates.

## Task Board

- [x] S0: Event schema and adapter shims
- [x] S1: Stream-capable agent core
- [x] S2: Runtime actor + command plane
- [ ] S3: Tool context and streaming tool outputs
- [ ] S4: Renderer decoupling and alternate frontend parity
- [ ] S5: Documentation and stabilization

## Remediation Linkage

1. `S1` satisfies remediation Milestone 5 task for API abstraction (`MockApiClient`/test seam).
2. `S2` + `S4` satisfy remediation Milestone 5 tasks for REPL extraction and renderer boundary.
3. `S3` + `S4` satisfy remediation Milestone 6 streaming UX/runtime API tasks.
4. `S5` documentation output is a required input for remediation Milestone 6 docs gate.

## Maintainer Instructions

1. Keep this **Status** section at the top and update it as work progresses.
2. Check off milestones/tasks immediately when complete.
3. Append a new entry to **Execution Log** for each meaningful implementation step (what changed, tests run, results, next step).
4. Commit between tasks/milestones before marking any task complete; include the commit ID in the corresponding log entry.
5. If scope changes, record the rationale in the log and adjust the Task Board/next steps.
6. Do not delete historical log entries; append only.

## Objective

Redesign Buddy’s library/runtime interface so a caller can:

1. submit prompts and control execution via commands,
2. receive a typed stream of events for model/tool/metrics/session lifecycle,
3. implement a full-featured alternate CLI (or GUI) with parity to the built-in CLI behavior.

## Current Gaps

1. `Agent::send()` is request/response and returns only final text.
2. Live activity events exist (`AgentUiEvent`) but are a UI-specific side channel wired to `main.rs`, not a stable public runtime API.
3. Tool execution is pull-based (`Tool::execute -> String`) with no streaming tool-level events.
4. `main.rs` owns orchestration logic (approval flow, task lifecycle, slash command behavior), making alternate frontends difficult to reproduce.
5. Progress/metrics rendering is terminal-coupled (`Renderer`/spinner) rather than model/runtime events.

## Design Options

## Option A: Minimal extension around current Agent

Expose a `send_with_events` callback/stream while leaving orchestration in `main.rs`.

Pros:
1. low risk, fast to implement.

Cons:
1. does not fully decouple CLI orchestration;
2. still hard to replicate approvals/background-task semantics in alternate clients.

## Option B (Recommended): Event-sourced runtime actor

Introduce a runtime actor with command input and event output as first-class API.

Pros:
1. clean front-end/backend separation;
2. supports alternate CLI/TUI/GUI with parity;
3. enables deterministic tests over event timelines.

Cons:
1. medium migration effort;
2. needs careful compatibility shims.

## Option C: Full plugin/reactive graph redesign

Large rework of tool/model/runtime as composable graph stages.

Pros:
1. maximal flexibility.

Cons:
1. highest risk/effort;
2. overkill before stabilizing core runtime contracts.

Recommendation: choose **Option B**.

## Recommended Architecture (Option B)

## 1) Public runtime API

```rust
pub struct BuddyRuntimeHandle {
    pub commands: tokio::sync::mpsc::Sender<RuntimeCommand>,
}

pub fn spawn_runtime(config: RuntimeConfig) -> (BuddyRuntimeHandle, RuntimeEventStream);
```

Where:

1. `RuntimeCommand` is the control plane (submit prompt, approve, cancel, switch model, session ops).
2. `RuntimeEventStream` is `impl Stream<Item = RuntimeEvent>` (or channel receiver wrapper).

## 2) Typed event envelope

```rust
pub struct RuntimeEventEnvelope {
    pub seq: u64,
    pub ts_unix_ms: u64,
    pub event: RuntimeEvent,
}
```

Event families:

1. `RuntimeEvent::Lifecycle` (runtime started/stopped, config loaded)
2. `RuntimeEvent::Session` (new/resume/saved/compacted)
3. `RuntimeEvent::Task` (queued/started/cancelled/completed/failed)
4. `RuntimeEvent::Model`:
   - request started/finished
   - text delta chunks
   - reasoning delta chunks
   - final assistant message
5. `RuntimeEvent::Tool`:
   - tool call requested
   - approval required/resolved
   - execution started/stdout chunk/stderr chunk/completed
6. `RuntimeEvent::Metrics`:
   - token usage updates
   - context usage updates
   - timing/latency samples
7. `RuntimeEvent::Warning` and `RuntimeEvent::Error` (typed, user-display-safe)

Every task-scoped event carries `session_id`, `task_id`, and `iteration` (if model loop step).

## 3) Command plane

```rust
pub enum RuntimeCommand {
    SubmitPrompt { prompt: String, metadata: PromptMetadata },
    Approve { approval_id: String, decision: ApprovalDecision },
    CancelTask { task_id: u64 },
    SetApprovalPolicy { policy: ApprovalPolicy },
    SwitchModel { profile: String },
    SessionNew,
    SessionResume { id: String },
    SessionResumeLast,
    Shutdown,
}
```

## 4) Model backend abstraction with streaming contract

Introduce backend trait that emits normalized model events:

```rust
#[async_trait]
pub trait ModelBackend {
    async fn run_turn(
        &self,
        req: ModelTurnRequest,
        sink: &mut dyn RuntimeEventSink,
        cancel: &CancellationToken,
    ) -> Result<ModelTurnResult, ApiError>;
}
```

Notes:

1. Responses API streams deltas naturally.
2. Completions API can emit synthetic single-chunk events if provider lacks streaming.
3. Runtime always receives normalized deltas + final message, independent of provider.

## 5) Tool runtime contract

Evolve tool interface from `String` output to structured + stream-capable output:

```rust
pub enum ToolRunOutcome {
    Final(ToolResult),
    Streaming(Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send>>),
}
```

`ToolContext` passed to tools should include:

1. cancellation token,
2. runtime event emitter,
3. execution target metadata (local/ssh/container/tmux),
4. shared HTTP client and policy handles.

Compatibility path:

1. adapt existing tools (`String` output) into `ToolRunOutcome::Final`.
2. migrate high-value tools (`run_shell`, `fetch_url`) to emit chunk/progress events.

## 6) Frontend adapter layer

Keep terminal renderer out of core runtime:

1. `runtime-core` emits typed events only.
2. `runtime-cli-adapter` translates events into current terminal presentation.
3. alternate CLI can consume same event stream and render differently with feature parity.

## 7) Persistable event timeline (optional but useful)

Add optional event log capture (`jsonl`) for:

1. reproducing bugs,
2. deterministic playback tests,
3. alternate frontend development.

## Incremental Migration Plan

## Milestone S0: Event schema and adapter shims

1. Define `RuntimeEvent`, `RuntimeCommand`, envelopes, IDs.
2. Add adapter from existing `AgentUiEvent` to new `RuntimeEvent`.
3. Keep current CLI behavior unchanged.

Gate:

1. all current tests pass,
2. event schema tests validate serialization and required IDs.

Commit:

1. `feat(runtime): add typed event schema and command contract`

## Milestone S1: Stream-capable agent core

1. Add `AgentRunner` that drives one prompt as event stream.
2. Keep `Agent::send()` as compatibility wrapper that consumes stream and returns final text.
3. Emit model/tool/token/context events through sink.

Gate:

1. deterministic integration tests assert event order and final outputs.

Commit:

1. `refactor(agent): introduce stream-capable runner with send compatibility wrapper`

## Milestone S2: Runtime actor + command plane

1. Implement `spawn_runtime` actor.
2. Move approval/task/session orchestration from `main.rs` into runtime actor.
3. Implement command handling for submit/cancel/approve/policy/session/model switch.

Gate:

1. current CLI rebuilt on runtime actor with no feature regression.

Commit:

1. `feat(runtime): add actor-based command/event runtime`

## Milestone S3: Tool context and streaming tool outputs

1. Introduce `ToolContext`.
2. Add compatibility adapter for existing tools.
3. Upgrade `run_shell` first to emit start/stdout/stderr/exit events.

Gate:

1. approval flow and cancellation still correct;
2. shell output appears incrementally in CLI adapter.

Commit:

1. `refactor(tools): add ToolContext and streaming tool outcomes`

## Milestone S4: Renderer decoupling and alternate frontend parity

1. Build `cli_event_renderer` consuming `RuntimeEvent`.
2. Move spinner/metrics to event-driven UI state.
3. Provide a minimal example alternate CLI binary in `examples/`.

Gate:

1. baseline CLI parity checklist passes;
2. example alternate CLI can run prompt, approve tools, cancel tasks, switch models.

Commit:

1. `refactor(cli): render from runtime events and add alternate frontend example`

## Milestone S5: Documentation and stabilization

1. Document runtime API, event taxonomy, and integration guide.
2. Add compatibility guarantees/versioning policy for event schema.
3. Mark older internal-only hooks as deprecated.

Gate:

1. docs complete and all offline tests pass.

Commit:

1. `docs(runtime): add streaming integration guide and event schema contract`

## Testing Strategy

## Core deterministic tests (offline, required)

1. Event ordering invariants:
   - task started before model/tool events,
   - tool approval required before tool execute,
   - completion/failure terminal event exactly once.
2. Cancellation invariants:
   - cancelled task emits cancellation terminal event,
   - no orphan tool-call transcripts.
3. Backpressure behavior:
   - bounded channels; clear behavior when consumer is slow.
4. Session invariants:
   - resume/new/switch events correlate to persisted snapshots.

## Protocol tests

1. Responses streaming emits chunk events and final aggregation.
2. Completions path emits at least synthesized chunk + final event.
3. Mixed tool-call loops preserve event iteration IDs.

## CLI parity tests

1. approval flow;
2. `/model` switching;
3. `/session` operations;
4. task cancellation/timeouts;
5. context/token metric updates.

## Risks and Mitigations

1. Event schema churn:
   - mitigation: version event envelope and document compatibility policy.
2. Performance overhead from high-frequency events:
   - mitigation: bounded channels + coalescing for verbose streams.
3. Migration regressions in `main.rs`:
   - mitigation: keep compatibility wrapper and move one subsystem at a time.

## Success Criteria

1. Library consumers can drive Buddy entirely via commands and typed event stream.
2. Built-in CLI is just one frontend on top of runtime events.
3. Alternate CLI can replicate full behavior without calling internal/private APIs.
4. Existing non-streaming callers using `Agent::send()` continue to work during migration.

## Execution Log

- 2026-02-27: Plan document created with architecture options and recommended Option B runtime design.
- 2026-02-27: Added top-level status/task-board/log maintenance instructions and began S0 implementation.
- 2026-02-27: Implemented `S0` scaffolding in `src/runtime.rs`:
  - Added public `RuntimeCommand`, `RuntimeEvent`, `RuntimeEventEnvelope`, and event family enums/structs.
  - Added adapter helpers: `runtime_event_from_agent_ui` and `runtime_envelope_from_agent_ui`.
  - Exported runtime module from `src/lib.rs`.
  - Added unit tests for JSON round-trip/shape and `AgentUiEvent` adapter mapping.
- 2026-02-27: Validation:
  - Ran `cargo fmt --all`.
  - Ran `cargo test` (all passing: lib/main/doc tests pass; network regression test remains ignored by default).
- 2026-02-27: Marked `S0` complete; set next active work to `S1`.
- 2026-02-27: Started `S1` implementation:
  - Extended `Agent` with `set_runtime_event_sink(...)` and sequence tracking.
  - Forwarded suppressed live events (`warning`, `token usage`, `reasoning`, tool call/result) to runtime envelopes via `runtime_envelope_from_agent_ui`.
  - Added `agent` unit test: `suppressed_warning_is_forwarded_to_runtime_sink`.
- 2026-02-27: Validation after `S1` partial changes:
  - Ran `cargo fmt --all`.
  - Ran `cargo test` (all passing: 201 lib tests, 31 main tests, doc tests pass; network regression ignored by default).
- 2026-02-27: Completed `S1`:
  - Added `ModelClient` trait in `src/api/mod.rs` and implementation for `ApiClient` (network client now mockable).
  - Added `Agent::with_client(...)` and `AgentRunner` facade for stream-capable runner entry point.
  - Extended `Agent::send(...)` to emit direct runtime events:
    - task lifecycle (`Started`, `Completed`, `Failed`),
    - model lifecycle (`RequestStarted`, `MessageFinal`),
    - tool lifecycle (`CallRequested`, `Result`),
    - metrics (`TokenUsage`) independent of display flags.
  - Kept CLI behavior unchanged; runtime sink is additive.
  - Added deterministic test `runtime_stream_emits_ordered_events_for_tool_round_trip` using mock model client + tool.
- 2026-02-27: Validation after `S1` completion:
  - Ran `cargo fmt --all`.
  - Ran `cargo test` (all passing: 202 lib tests, 31 main tests, doc tests pass; network regression ignored by default).
- 2026-02-27: Marked `S1` complete; next active work is `S2`.
- 2026-02-27: Started `S2` implementation (actor core + commands):
  - Added runtime actor API in `src/runtime.rs`:
    - `BuddyRuntimeHandle`, `RuntimeEventStream`,
    - `RuntimeSpawnConfig`, `spawn_runtime(...)`, `spawn_runtime_with_agent(...)`.
  - Added command handling for:
    - `SubmitPrompt`, `CancelTask`, `SetApprovalPolicy`, `SwitchModel`,
    - `SessionNew`, `SessionResume`, `SessionResumeLast`, `Shutdown`.
  - Added runtime session persistence integration (optional `SessionStore`) and model-switch auth checks.
  - Added internal task orchestration with cancellation channel and event forwarding.
  - Added `ModelEvent::ProfileSwitched`.
- 2026-02-27: Added S2 tests:
  - `runtime_actor_submit_prompt_emits_expected_events`
  - `runtime_actor_cancel_task_emits_cancelling`
  - `runtime_actor_switch_model_emits_profile_switched`
- 2026-02-27: Validation after S2 actor-core changes:
  - Ran `cargo fmt --all`.
  - Ran `cargo test` (all passing: 205 lib tests, 31 main tests, doc tests pass; network regression ignored by default).
- 2026-02-27: S2 is partially complete. Remaining work is CLI migration to consume runtime actor events and final approval-command wiring.
- 2026-02-27: Continued S2 CLI migration:
  - One-shot `buddy exec` path in `main.rs` now runs through `spawn_runtime_with_agent(...)` and consumes runtime events (`MessageFinal`, `TaskFailed`, `TaskCompleted`) instead of calling `Agent::send(...)` directly.
  - This starts real runtime-backed CLI usage while keeping interactive REPL on legacy flow for now.
- 2026-02-27: Validation after `exec` runtime migration:
  - Ran `cargo fmt --all`.
  - Ran `cargo test` (all passing: 205 lib tests, 31 main tests, doc tests pass; network regression ignored by default).
- 2026-02-27: Cross-plan integration update:
  - Synchronized this plan with `2026-02-27-claude-feedback-remediation-plan.md`.
  - Added explicit remediation linkage for `S1`-`S5`.
  - Updated next steps to include remediation gate handoff once `S2` interactive migration completes.
- 2026-02-27: Remediation Milestone 0 completed in parallel:
  - Added shared parser/auth/execution test fixtures in `src/testsupport.rs`.
  - Added remediation runbook (`docs/playbook-remediation.md`) and ai-state issue tracking.
  - Maintains `S2` as active milestone for interactive runtime migration.
- 2026-02-27: Parallel remediation Milestone 1 slice `B1` completed:
  - Added shared UTF-8-safe truncation helpers and migrated truncation call sites.
  - Added UTF-8 regression tests for parser/tool/render truncation paths.
  - Runtime milestone status unchanged: `S2` remains active next step.
- 2026-02-27: Parallel remediation Milestone 1 slice `R1` completed:
  - Added centralized timeout policy for API/fetch via `[network]` config.
  - Added timeout regression tests using local hanging socket fixtures.
  - Runtime milestone status unchanged: `S2` remains active next step.
- 2026-02-27: Checkpoint commit created after integrated runtime+remediation baseline work.
  - commit: `ace8000`
- 2026-02-27: Parallel remediation Milestone 1 slice `S2` completed:
  - Added fetch SSRF protections and domain allow/deny policy controls.
  - Added optional fetch confirmation flow with shared approval broker wiring.
  - Validation: `cargo test` passed.
  - commit: `26e389d`
- 2026-02-27: Parallel remediation Milestone 1 slice `S3` completed:
  - Added `write_file` path policy controls (`tools.files_allowed_paths` + sensitive directory guardrails).
  - Validation: `cargo test` passed.
  - commit: `81325b0`
- 2026-02-27: Parallel remediation Milestone 1 slice `S1` completed:
  - Added shell denylist policy (`tools.shell_denylist`) and denylist enforcement in `run_shell`.
  - Added one-shot `buddy exec` fail-closed guard when shell confirmation is enabled, plus `--dangerously-auto-approve` override.
  - Validation: `cargo test` passed.
  - commit: `e5ad7ee`
- 2026-02-27: Completed streaming/runtime milestone `S2`:
  - Migrated interactive REPL prompt/task orchestration to runtime command/event flow.
  - Wired explicit runtime approval flow (`RuntimeCommand::Approve`) and runtime-owned approval queue handling.
  - Added runtime approval command regression test and runtime-context metric emission.
  - Validation: `cargo test` passed.
  - commit: `84724e3`
- 2026-02-27: Parallel remediation Milestone 3 completed before `S3` start:
  - Landed SSE parser hardening, transient API retry/backoff with `Retry-After`, and shared auth/search HTTP client reuse.
  - Validation: `cargo test` passed; model regression suite executed with one expected missing-key failure for `kimi` (`MOONSHOT_API_KEY` not set).
  - commit: `e4cf33c`
