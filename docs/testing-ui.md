# UI Regression Testing (Tmux Harness)

This document defines the on-demand UI integration/regression harness for terminal behavior.

Status: planned in `docs/plans/2026-03-01-feature-requests.md` Milestone 1.

## Goals

1. Validate end-to-end REPL rendering and dynamic UI behavior in a real terminal.
2. Catch regressions in prompts, spinners, approval flows, and output formatting.
3. Preserve actionable artifacts on failure (pane captures, streamed logs, recordings).

## Harness Shape

1. Launch buddy in an isolated tmux session/pane.
2. Use deterministic mock/fake model responses.
3. Drive scripted user inputs through tmux pane input.
4. Observe output via:
   - `tmux capture-pane` checkpoint snapshots,
   - `tmux pipe-pane` continuous log stream.
5. Record each scenario with asciinema.

## Planned Scenarios

1. Startup banner and prompt rendering.
2. Spinner lifecycle during in-flight tasks.
3. Approval prompt render + acceptance path.
4. Tool output block formatting and completion lines.
5. Prompt restoration and status updates after task completion.

## Planned Artifact Layout

1. `artifacts/ui-regression/<scenario>/capture-*.txt`
2. `artifacts/ui-regression/<scenario>/pipe.log`
3. `artifacts/ui-regression/<scenario>/session.cast`
4. `artifacts/ui-regression/<scenario>/report.json`

## Invocation Model

The suite is opt-in and should not run under default `cargo test`.

Planned command shape:

```bash
cargo test --test ui_tmux_regression -- --ignored --nocapture
```

An optional make target may wrap the command after implementation.
