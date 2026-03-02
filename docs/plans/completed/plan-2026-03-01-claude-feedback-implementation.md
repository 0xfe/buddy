# Claude Feedback Implementation Plan (2026-03-01)

## Status

- Program status: Completed
- Scope status: Locked to actionable items from `docs/plans/review-2026-03-01-claude-feedback.md`, sequenced for lowest-risk delivery.
- Current focus: Plan closure and archival.
- Completed so far:
  1. Reviewed `docs/plans/review-2026-03-01-claude-feedback.md` end-to-end.
  2. Mapped priority recommendations into an execution board with validation gates.
  3. Implemented first Milestone 1 slice: `--trace` + `BUDDY_TRACE_FILE`, JSONL runtime-event sink wiring in REPL/exec, and best-effort redaction.
  4. Completed Milestone 1 event completeness: phase durations, task metadata threading (session/iteration/correlation), request/response summaries, and compaction stats in runtime events.
  5. Completed Milestone 2: `-v/-vv/-vvv`, structured logging subscriber wiring, and span instrumentation for runtime commands, turns, model requests/responses, tool calls, and compaction paths.
  6. Completed Milestone 3: static system prompt + request-scoped tmux snapshot context injection with non-default target labeling and prompt-architecture docs updates.
  7. Completed Milestone 4: pair-safe compaction units, orphan tool-call/result repair passes, structured compaction summaries, and failed-tool retention guarantees.
  8. Completed Milestone 5: explicit prompt-priority sectioning, additive operator-instructions conflict policy, planning-before-tools guidance, and structured tool-definition disambiguation metadata.
  9. Completed Milestone 6: explicit provider field support with fallback detection, provider-priority reasoning extraction, and per-model token-estimation calibration.
  10. Completed Milestone 7: trace analysis CLI (`summary`, `replay`, `context-evolution`), pricing-backed cost metrics, and expanded model regression probes for tool-error history compatibility.
- Next steps:
  1. Move this completed plan to `docs/plans/completed/` after commit.

## Goal

Improve model reliability and debuggability by fixing the highest-impact context and compaction issues, while building first-class observability early so model/tool behavior can be understood and replayed.

## Scope

In scope (from the feedback doc):

1. P0 context reliability:
   - move dynamic tmux snapshot out of mutable system prompt,
   - preserve tool-call/result integrity during compaction,
   - improve compaction summary quality.
2. P1 multi-model and operator reliability:
   - prompt structure hardening,
   - stronger tool descriptions and disambiguation,
   - planning-before-action instruction.
3. P1 observability (prioritized early):
   - trace output (`--trace`/env),
   - runtime event completeness,
   - verbosity/logging controls.
4. P2 targeted robustness:
   - explicit provider config with fallback detection,
   - provider-specific reasoning extraction,
   - better context/token estimation calibration,
   - error-preserving compaction behavior.
5. P3 high-value follow-through:
   - trace analysis CLI,
   - cost tracking,
   - expanded eval/regression coverage.

Out of scope for this plan:

1. Large new user-facing features unrelated to feedback remediation.
2. Breaking API changes to public library interfaces unless required for correctness.

## Planning Principles

1. Trace-first: ship observability before major behavior changes.
2. Small vertical slices with hard acceptance gates and rollback-friendly commits.
3. Each milestone must include tests and docs updates in the same change set.
4. Preserve backward compatibility unless a behavior is unsafe or incorrect.
5. Commit between tasks/milestones and record commit IDs in this plan while executing.

## Plan Maintenance Rules

1. Keep this file in `docs/plans/` while active; move to `docs/plans/completed/` when done.
2. Update `## Status`, the board checkboxes, and `## Execution Log` during implementation.
3. Do not mark a task complete until code + tests + docs for that task are complete.
4. If scope changes, add an execution-log entry before implementing the change.

## Integrated Program Board

- [x] Milestone 0: Baseline, Repro Matrix, and Design Freeze
- [x] Milestone 1: Tracing Foundation (`--trace`, JSONL sink, event completeness)
- [x] Milestone 2: Logging/Verbosity and Span Model
- [x] Milestone 3: Prompt Context Architecture (static system + dynamic snapshot as turn context)
- [x] Milestone 4: Compaction Integrity and Error-Preserving Memory
- [x] Milestone 5: Prompt and Tooling Reliability Hardening
- [x] Milestone 6: Provider/Model Compatibility and Token Accuracy
- [x] Milestone 7: Evaluation, Trace Tooling, Cost Visibility, and Closure

## Milestone 0: Baseline, Repro Matrix, and Design Freeze

### Scope

Create deterministic repro coverage for each targeted issue and freeze implementation boundaries before code changes.

### Tasks

1. Build an issue matrix mapping feedback IDs to code locations and expected behaviors.
2. Add/refresh failing tests or scripted repros for:
   - mutable system prompt per turn,
   - compaction tool-pair breakage,
   - summary quality and error preservation,
   - dead/incomplete runtime events.
3. Capture baseline metrics from a short scripted session:
   - token estimate behavior,
   - compaction trigger behavior,
   - tool error visibility.
4. Confirm module boundaries for upcoming work (`runtime`, `api`, `prompt`, `history`, `tracing`).

### Acceptance Gate

1. Every in-scope feedback item has either a failing automated test or a documented manual repro.
2. Baseline tests pass before remediation starts.
3. Boundary decisions are documented in this plan.

### Tests

1. `cargo test`
2. `cargo test --test ui_tmux_regression -- --ignored --nocapture`
3. `cargo test --test model_regression -- --ignored --nocapture` (smoke subset where applicable)

### Docs

1. Update this plan with baseline findings.
2. Add/update any required test runbooks under `docs/developer/`.

### Commit Slices

1. `test(plan): add baseline repro coverage for claude-feedback remediation`

## Milestone 1: Tracing Foundation (`--trace`, JSONL sink, event completeness)

### Scope

Ship immediate observability so every model turn can be inspected, replayed, and debugged.

### Tasks

1. Add trace configuration surface:
   - CLI flag `--trace <path>`,
   - env override `BUDDY_TRACE_FILE`.
2. Implement JSONL trace sink that fans out `RuntimeEventEnvelope` to file.
3. Emit missing/underused events:
   - `PhaseDuration`,
   - `ToolEvent::Result`,
   - `MessageFinal` from the agent loop path.
4. Thread metadata fields end-to-end:
   - `TaskRef.session_id`,
   - `TaskRef.iteration`,
   - stable per-request correlation ID.
5. Capture model-interaction artifacts in trace:
   - normalized request payload summary (with safe redaction),
   - response payload summary (`usage`, finish reason, tool calls),
   - compaction events (pre/post counts and token estimates).
6. Ensure trace writes are non-fatal (best-effort) and do not break REPL/exec.

### Acceptance Gate

1. A full turn produces a readable JSONL sequence with enough data for replay/debug.
2. No secrets are written in clear text in trace output.
3. Trace-disabled behavior remains unchanged.

### Tests

1. Unit tests for trace sink writer and redaction rules.
2. Runtime tests verifying required events are emitted in expected order.
3. Golden tests for trace envelopes of one prompt+tool+response turn.
4. `cargo test`

### Docs

1. New `docs/design/observability.md` (trace format, fields, redaction policy).
2. Update `README.md` and `docs/DEVELOPER.md` with trace usage.
3. Update `ai-state.md` with tracing workflow pointers.

### Commit Slices

1. `feat(trace): add trace config and JSONL runtime-event sink`
2. `feat(trace): emit complete runtime metadata and model/compaction trace events`
3. `docs(trace): document trace file format and operator workflow`

## Milestone 2: Logging/Verbosity and Span Model

### Scope

Introduce structured logging and a stable internal span model for turn/LLM/tool observability.

### Tasks

1. Add `--verbose` (`-v`, `-vv`, `-vvv`) output levels.
2. Integrate `tracing` + subscriber with level mapping and component filters.
3. Add spans/events for:
   - session/turn,
   - llm request/response,
   - tool execution,
   - compaction.
4. Align span fields with GenAI semantic conventions where practical.
5. Keep JSONL trace sink available and compatible with verbosity levels.

### Acceptance Gate

1. `-v/-vv/-vvv` expose progressively richer diagnostics.
2. Span/log output captures retries, durations, and payload summaries without leaking secrets.
3. No measurable regressions in normal non-verbose REPL UX.

### Tests

1. CLI tests for verbose flag parsing and level behavior.
2. Runtime tests for span/phase duration emission around LLM and tool calls.
3. `cargo test`

### Docs

1. `docs/design/observability.md` (verbosity + spans).
2. `docs/DEVELOPER.md` debug recipes (`RUST_LOG` and trace combos).

### Commit Slices

1. `feat(logging): add verbosity levels and tracing subscriber wiring`
2. `feat(trace): add turn/llm/tool span instrumentation`

## Milestone 3: Prompt Context Architecture (static system + dynamic snapshot as turn context)

### Scope

Preserve cache-friendly static system prompt and eliminate screenshot prompt-injection risks.

### Tasks

1. Keep system prompt static across turns.
2. Move tmux snapshot to a synthetic per-turn context message (user-context style), not system message.
3. Ensure only current snapshot is injected (replace old snapshot context every turn).
4. Add explicit context labeling:
   - clearly mark default pane snapshot,
   - clearly indicate when snapshot came from non-default target.
5. Harden snapshot content handling:
   - delimiter-safe formatting,
   - size limits and truncation notes,
   - basic injection-resistant framing.
6. Preserve guidance for “do not execute if no shell prompt is visible.”

### Acceptance Gate

1. System prompt text is byte-for-byte stable between turns.
2. Snapshot context updates every turn without accumulating stale snapshots.
3. Model behavior in tmux remains correct for default and non-default panes.

### Tests

1. Unit tests for prompt assembly and snapshot replacement semantics.
2. Integration tests for default-pane vs explicit-pane snapshot behavior.
3. `cargo test`
4. `cargo test --test ui_tmux_regression -- --ignored --nocapture`

### Docs

1. `docs/design/prompt.md` or equivalent prompt architecture doc.
2. `DESIGN.md` features section update for snapshot injection behavior.

### Commit Slices

1. `fix(prompt): make system prompt static and move tmux snapshot to turn context`
2. `test(prompt): cover snapshot rotation and target labeling`

## Milestone 4: Compaction Integrity and Error-Preserving Memory

### Scope

Prevent tool-history corruption and improve compacted memory quality.

### Tasks

1. Implement atomic compaction units that preserve assistant tool-calls with matching tool results.
2. Add post-compaction validator to remove/repair orphaned tool messages.
3. Replace plain-text compaction summary with structured summary format:
   - operation,
   - success/failure,
   - key outcome/error.
4. Always retain last N failed tool operations verbatim.
5. Emit compaction trace events with pre/post token and message counts.

### Acceptance Gate

1. Compaction never produces invalid tool-call/result history.
2. Summaries preserve failure context and key outcomes.
3. API calls after compaction remain valid across providers.

### Tests

1. Property/fixture tests for compaction invariants.
2. Regression tests for orphaned tool-call/result scenarios.
3. Model-facing tests validating post-compaction continuation quality.
4. `cargo test`

### Docs

1. `docs/design/context-management.md` compaction algorithm and guarantees.
2. Update `ai-state.md` compaction notes.

### Commit Slices

1. `fix(history): preserve tool-call/result pairs during compaction`
2. `feat(history): structured compaction summaries with failure retention`

## Milestone 5: Prompt and Tooling Reliability Hardening

### Scope

Improve cross-model instruction following and tool selection accuracy.

### Tasks

1. Restructure system prompt with explicit sections and clear rule priority.
2. Move critical behavioral rules to top and reinforce at end.
3. Wrap operator custom instructions in explicit structured block with conflict policy.
4. Add lightweight planning instruction before tool actions.
5. Expand tool definitions with:
   - when to use,
   - when not to use,
   - disambiguation between similar tools,
   - short invocation examples.
6. Ensure tmux execution model guidance is explicit and default-target-first.

### Acceptance Gate

1. Prompt template has deterministic section ordering and explicit priorities.
2. Tool-selection and parameter-format errors decrease in regression scenarios.
3. Added guidance does not break existing working model flows.

### Tests

1. Prompt template snapshot tests.
2. Tool schema/definition tests (required fields, examples, disambiguation text).
3. Scenario tests for `run_shell` vs `send_keys` decisions.
4. `cargo test`

### Docs

1. `docs/design/prompt.md` and `docs/design/tools.md` updates.
2. `DESIGN.md` feature updates for tool guidance behavior.

### Commit Slices

1. `feat(prompt): restructure system prompt with explicit priority sections`
2. `feat(tools): enrich tool definitions with disambiguation and examples`

## Milestone 6: Provider/Model Compatibility and Token Accuracy

### Scope

Reduce provider-specific failures and improve context-budget decisions.

### Tasks

1. Add explicit `provider` field to model profiles with URL-based fallback.
2. Introduce provider-specific reasoning extractors with generic fallback.
3. Improve token estimation using live usage calibration per model.
4. Keep reasoning output normalization robust (`null`/empty suppression remains provider-aware).
5. Update model compatibility docs and default profile examples.

### Acceptance Gate

1. Provider behavior no longer depends solely on base-url heuristics.
2. Reasoning extraction works for supported providers without showing JSON structures/noise.
3. Token estimation error improves and is tracked per model.

### Tests

1. Provider compatibility unit tests.
2. Reasoning extraction fixtures per provider family.
3. Calibration tests for estimate adjustment logic.
4. `cargo test`
5. `cargo test --test model_regression -- --ignored --nocapture`

### Docs

1. Update `docs/design/models.md` with provider-specific behavior.
2. Update config reference docs for `provider` field.

### Commit Slices

1. `feat(models): add explicit provider config with compatibility fallback`
2. `fix(reasoning): add provider-specific reasoning extraction pipelines`
3. `feat(tokens): calibrate token estimates from usage telemetry`

## Milestone 7: Evaluation, Trace Tooling, Cost Visibility, and Closure

### Scope

Close the loop with analysis tooling, cost metrics, and regression confidence.

### Tasks

1. Add `buddy trace` subcommands:
   - `summary`,
   - `replay --turn`,
   - `context-evolution`.
2. Add per-model pricing fields and per-turn/session cost estimates.
3. Expand eval/regression suite for multi-model tool behavior and error recovery.
4. Final hardening pass for docs and deprecation notes.

### Acceptance Gate

1. Operators can explain any turn using trace replay + context evolution.
2. Session-level token/cost summaries are available and accurate enough for operations.
3. Regression suite covers core multi-model behavior changes introduced in this plan.

### Tests

1. CLI integration tests for `buddy trace` commands.
2. Cost calculation tests.
3. Extended model regression suite (ignored by default).
4. `cargo test`

### Docs

1. `docs/developer/tracing-cli.md` (or merged into existing observability docs).
2. README pointers to trace/eval/cost docs.
3. Plan completion summary and archival.

### Commit Slices

1. `feat(trace): add trace analysis CLI commands`
2. `feat(metrics): add model pricing and cost tracking`
3. `test(eval): expand multi-model behavior regression suite`
4. `docs(plan): close claude-feedback implementation plan`

## Module Boundary Freeze (Milestone 0)

1. `src/runtime/*`: event emission, task lifecycle metadata, turn/session correlation IDs.
2. `src/app/trace.rs`: JSONL sink, redaction, and trace path resolution (`--trace`, `BUDDY_TRACE_FILE`).
3. `src/prompt/*`: static system prompt assembly + dynamic per-turn context injection.
4. `src/history/*` or existing compaction modules: pair-safe compaction and structured summaries.
5. `src/api/*`: provider-aware reasoning extractors and usage-calibration hooks.

## Risks and Mitigations

1. Risk: tracing captures secrets accidentally.
   - Mitigation: strict redaction helper, dedicated tests, and no raw headers by default.
2. Risk: prompt restructuring regresses strong models while helping weaker ones.
   - Mitigation: model regression suite across providers before merge.
3. Risk: compaction changes break tool-call protocol on one API.
   - Mitigation: invariant tests + provider-specific protocol validation.
4. Risk: instrumentation overhead degrades UX.
   - Mitigation: trace/logging opt-in defaults and lightweight hot-path checks.

## Execution Log

1. 2026-03-01: Created this execution plan from `docs/plans/review-2026-03-01-claude-feedback.md`; tracing milestone intentionally prioritized immediately after baseline.
2. 2026-03-01: Started Milestone 1 implementation slice:
   - added `--trace <path>` and `BUDDY_TRACE_FILE` support,
   - added best-effort JSONL runtime-event sink in REPL and `buddy exec`,
   - added heuristic trace redaction for obvious secret-bearing fields/content,
   - added tests in `src/app/trace.rs` and `src/cli.rs`,
   - validated with `cargo test` (full suite).
   - commit: `f9983ad`.
3. 2026-03-01: Finished Milestone 1:
   - emitted `Metrics.PhaseDuration` for model requests and tool execution,
   - threaded `TaskRef` metadata (`session_id`, `iteration`, `correlation_id`) across runtime + agent events,
   - added `Model.RequestSummary` and `Model.ResponseSummary` events for trace-friendly request/response artifacts,
   - enriched `Session.Compacted` with pre/post token estimates and removal counts,
   - expanded runtime/agent tests to verify new event ordering and metadata propagation,
   - validated with `cargo fmt` and `cargo test` (full suite).
   - commit: `911a305`.
4. 2026-03-01: Finished Milestone 2:
   - added `-v/-vv/-vvv` CLI verbosity levels and a `tracing-subscriber` bootstrap path in `main`,
   - added `BUDDY_LOG`/`RUST_LOG` filter support with component noise guards (`reqwest/hyper/h2/rustls`),
   - introduced span instrumentation for runtime commands, session operations, prompt turns, model request/response, tool calls, and history/session compaction,
   - kept JSONL runtime-event tracing independent and compatible with verbose logging,
   - expanded tests for CLI verbosity parsing and logging filter resolution,
   - updated observability/developer/reference docs for verbosity and tracing usage,
   - validated with `cargo fmt` and `cargo test` (full suite).
   - commit: `d680cdc`.
5. 2026-03-01: Finished Milestone 3:
   - removed dynamic system-message mutation for tmux snapshots and kept system prompt static across turns,
   - introduced request-scoped dynamic context injection for default shared-pane screenshots,
   - added non-default tmux-target context labeling when recent tool calls explicitly route away from default shared pane,
   - hardened snapshot framing with explicit context markers and truncation notes,
   - updated prompt architecture documentation (`docs/design/prompt.md`) and related design docs,
   - validated with `cargo fmt`, `cargo test`, and `cargo test --test ui_tmux_regression -- --ignored --nocapture`.
   - commit: `d00838d`.
6. 2026-03-01: Finished Milestone 4:
   - compaction now operates on atomic units that keep assistant tool-call/result pairs together,
   - added pre/post compaction tool-history repair to remove orphan tool results and unmatched assistant tool calls,
   - replaced free-form compaction prose with structured summary entries (`op`, `status`, `detail`) including tool success/failure outcomes,
   - preserved last three failed tool operations verbatim across compaction passes for debugging continuity,
   - added regression coverage in `src/agent/history.rs` + `src/agent/normalization.rs` for pair integrity, failure retention, structured summaries, and orphan repair,
   - added `docs/design/context-management.md` and refreshed design/feature docs for new guarantees,
   - validated with `cargo fmt` and `cargo test` (full suite).
   - commit: `37a4d82`.
7. 2026-03-02: Finished Milestone 5:
   - restructured `src/templates/system_prompt.template` into explicit priority-ordered sections with rule reinforcement and a deterministic final checklist,
   - added additive operator-instructions block framing with explicit conflict policy in prompt rendering (`src/prompt.rs`),
   - added explicit planning-before-tools guidance and run_shell-vs-send-keys-vs-capture-pane decision guidance in the system prompt,
   - enriched built-in tool definitions with structured `When to use` / `When NOT to use` / `Disambiguation` / `Examples` guidance across shell, tmux, file, fetch, search, time, and capture/send tools,
   - added prompt snapshot/ordering tests and tool-definition guidance/schema regression tests across tool modules,
   - updated prompt/tools/design docs to match the new behavior,
   - validated with `cargo fmt` and `cargo test` (full suite).
   - commit: `b625cba`.
8. 2026-03-02: Finished Milestone 6:
   - added explicit model-profile `provider` support (`auto|openai|openrouter|moonshot|other`) and resolved-runtime provider plumbing in config/API/auth/preflight paths,
   - switched provider compatibility behavior (responses reasoning config, completions reasoning overrides, login-support checks) to resolved provider selection with `auto` base-url fallback,
   - added provider-priority reasoning extraction with generic fallback and placeholder/noise suppression retention,
   - added per-model runtime token-estimation calibration based on observed `usage.prompt_tokens` with bounded smoothing,
   - updated bundled `buddy.toml` template and model/config design docs for provider semantics and calibration behavior,
   - added/updated provider and reasoning regression tests plus token calibration unit coverage,
   - validated with `cargo fmt`, `cargo test` (full suite), and `cargo test --test model_regression -- --ignored --nocapture` (all default template profiles passing).
   - commit: `82255ae`.
9. 2026-03-02: Finished Milestone 7:
   - added `buddy trace` subcommands (`summary`, `replay --turn`, `context-evolution`) with strict JSONL parsing and turn/timeline reconstruction in `src/app/trace_cli.rs`,
   - added pricing-aware request/session cost metrics (`Metrics.Cost`) and runtime emission from agent usage telemetry when `templates/models.toml` provides pricing metadata,
   - added baseline pricing metadata for `gpt-5*` rules in `src/templates/models.toml`,
   - expanded ignored model regression coverage with a provider-compatibility probe that injects tool-call + tool-error history and verifies follow-up response continuity across default profiles,
   - updated observability/reference/model docs and added `docs/developer/tracing-cli.md`,
   - validated with `cargo fmt`, `cargo test -q`, and `cargo test --test model_regression -- --ignored --nocapture` (both ignored model-regression tests passing).
   - commit: `f3bf480`.
