# 2026-03-03 Tmux Regression + Recovery Hardening Plan

## Status
- Current: `M6 validation complete (ready to commit)`
- Next: `Commit milestone slices with IDs, then move plan to completed/`
- Priority: eliminate default-pane loss confusion, stop missing-target tool loops, and harden observability for prompt/tool iteration.
- Commit cadence: commit after each milestone with the milestone ID in the commit message; add commit IDs to the execution log.

## Problem Summary
Recent SSH tmux runs showed three regressions:
1. The model ran shell-fragile snippets (`set -e`), then execution state degraded.
2. Default-pane/session recovery was not consistently visible to the operator and model.
3. The agent repeated failing `tmux_send_keys` calls (`tmux target not found`) and spun in a tool-error loop.

These failures indicate gaps in:
- guardrails (prompt-only guidance is insufficient),
- deterministic tmux regression coverage,
- and tooling for rapid prompt iteration against real models.

## Goals
1. Reproduce this failure class in deterministic on-demand tmux UI/integration tests.
2. Add hard safety rails in tool execution so shell-killing behaviors are blocked before they damage the shared pane.
3. Make default-pane/session recovery and managed-target loss explicit in both operator UI and model-visible tool results.
4. Prevent repeated same-error tool loops from consuming turns/tokens.
5. Add a repeatable real-model prompt-eval workflow using tracing artifacts so prompt/tool wording can be tuned empirically.

## Milestones

### M0: Baseline + Failure Characterization
- Capture current behavior with focused failing/near-failing scenarios:
  - fragile command bootstrap (`set -e` in shared shell),
  - explicit missing managed target loop (`tmux_send_keys`/`tmux_capture_pane`),
  - default shared pane loss + recovery visibility.
- Document observed failure signatures and intended behavior in this plan log.
- Status: `Completed`

### M1: Regression Harness Expansion (Fake Models + Deterministic Scripts)
- Extend `tests/ui_tmux_regression.rs` + `tests/ui_tmux/mod.rs` with scripted scenarios:
  - `ui_tmux_default_pane_recovery_notice_flow`
  - `ui_tmux_missing_target_error_no_spin_loop`
  - `ui_tmux_shell_fragile_command_blocked`
- Improve harness fixtures for multi-step scripted tool-call failures/retries.
- Ensure teardown always removes harness and managed sessions/panes.
- Status: `Completed`

### M2: Tool/Backend Safety + Recovery Semantics
- Add run-shell preflight guardrails for shell-killing command patterns in managed shared panes.
  - hard-fail with remediation guidance (use subshell/heredoc; avoid shell-option mutations).
- Harden missing-target errors:
  - structured remediation text that clearly states default behavior and creation flow.
- Ensure default-pane recovery emits durable notices in tool payloads whenever recovery occurs.
- Ensure repaired default pane gets prompt bootstrap exactly once on creation.
- Status: `Completed`

### M3: Loop Suppression + Runtime Recovery UX
- Add repeated-tool-failure suppression (same tool + same args + same error) per task iteration.
- After threshold breach, return a synthesized error to model instructing next valid action and halt repeated execution for that call pattern.
- Surface terminal warnings for:
  - default shared pane/session recreated,
  - non-default managed pane/session missing.
- Add tests for suppression behavior and warning/event rendering.
- Status: `Completed`

### M4: Prompt + Tool Contract Clarification
- Update system prompt tmux section with explicit MUST-NOT rules:
  - do not run `set -e`, `set -o errexit`, `exit`, `logout`, `exec` that can terminate shared shell.
- Tighten tool descriptions/examples so default-target usage is dominant and explicit selectors are exceptional.
- Add targeted tests for prompt template text and tool-definition contract wording.
- Status: `Completed`

### M5: Real-Model Prompt Evals + Tracing Workflow
- Add an on-demand eval harness that runs curated prompts against configured real models/providers.
- Reuse tracing/session artifacts for per-turn analysis:
  - prompt sent,
  - tool selected,
  - error classes,
  - retries/loops.
- Add docs for iterative prompt tuning workflow:
  - run eval set,
  - inspect traces,
  - adjust prompt/tool docs,
  - re-run and compare.
- Status: `Completed` (workflow + tooling landed; live provider smoke is operator-triggered on demand)

### M6: Docs + Validation
- Update docs with tmux safety/recovery semantics and new test workflows:
  - `docs/design/tools.md`
  - `docs/design/prompt.md`
  - `docs/developer/BUILD.md`
  - `docs/tips/tmux.md`
- Validation gates:
  - `cargo fmt --all`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test -q`
  - on-demand tmux regressions for new scenarios
  - on-demand real-model prompt eval smoke pass
- Status: `Completed` (all local/offline + tmux on-demand gates passed; real-model smoke intentionally manual/on-demand)

## Execution Log
- 2026-03-03: Plan created from reported SSH tmux regression (`set -e` + missing-target loop + absent recovery visibility). Starting M0 characterization.
- 2026-03-03: M0 characterization complete.
  - Confirmed current failures: fragile shared-shell commands were not blocked; missing-target explicit calls could repeat; recovery visibility depended on path.
- 2026-03-03: M1 complete.
  - Added tmux UI regression scenarios:
    - `ui_tmux_shell_fragile_command_is_blocked`
    - `ui_tmux_default_pane_recovery_notice_flow`
    - `ui_tmux_missing_target_error_suppressed_after_repeats`
  - Verified full ignored suite passes: `cargo test --test ui_tmux_regression -- --ignored --nocapture` (5/5).
- 2026-03-03: M2 complete.
  - Added managed-tmux `run_shell` guardrails blocking `set -e`/`errexit`, `exit/logout`, and `exec`.
  - Hardened explicit missing-target errors with default-pane recovery guidance in local/container/ssh backends.
- 2026-03-03: M3 complete.
  - Added repeated identical tool-failure suppression in agent loop.
  - Added agent tests for tracker behavior and suppression.
- 2026-03-03: M4 complete.
  - Strengthened system prompt shell safety rules with explicit MUST-NOT guidance.
  - Added prompt-routing fallback so recent `tmux target not found` errors force default-pane snapshot routing.
- 2026-03-03: M5 complete.
  - Added `scripts/prompt-eval.sh` and `make prompt-eval` for trace-backed real-model prompt eval runs.
  - Added docs: `docs/developer/prompt-evals.md` and cross-links from build/developer/tracing docs.
- 2026-03-03: M6 validation complete.
  - `cargo fmt --all`
  - `cargo test -q` (all passing)
  - `cargo clippy --all-targets -- -D warnings` (passing)
  - `cargo test --test ui_tmux_regression -- --ignored --nocapture` (passing)
- Commit IDs: pending (to be filled during commit slicing).
