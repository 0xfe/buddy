# UI Regression Testing (Tmux Harness)

This document describes the on-demand terminal UI integration suite used to validate
real REPL rendering behavior in tmux.

## Purpose

1. Validate end-to-end UI behavior (startup banner, prompt, spinner, approvals, tool output).
2. Catch regressions that unit tests cannot see (line redraw, status updates, terminal formatting).
3. Preserve enough artifacts to make failures debuggable without rerunning immediately.

## Current Suite

Integration test entrypoint:

- `tests/ui_tmux_regression.rs`

Harness utilities:

- `tests/ui_tmux/mod.rs`

Current scenarios:

1. Baseline shell approval/rendering flow:
   - Start buddy inside an isolated tmux pane through asciinema.
   - Run one prompt that produces a deterministic `run_shell` tool call via a fake model server.
   - Approve the command.
   - Verify spinner/liveness lines, approval formatting, command output, and final assistant reply.
   - Exit cleanly and assert expected mock request count.
2. Managed tmux pane + targeted shell flow:
   - Run scripted tool calls that create a managed pane (`tmux-create-pane`) and then run `run_shell` targeted to that pane.
   - Approve both operations.
   - Verify targeted approval and output rendering.
   - Assert expected mock request count and clean shutdown.

## Runtime Dependencies

The suite requires these commands in `PATH`:

1. `tmux`
2. `asciinema`

If either is missing, the ignored test fails with an actionable prerequisite message.

## Artifact Model

Each run writes under:

- `artifacts/ui-regression/<scenario>-<pid>-<timestamp>/`

Artifacts include:

1. `session.cast`:
   - full asciinema recording.
2. `pipe.log`:
   - continuous `tmux pipe-pane` output stream.
3. `snapshots/*.txt`:
   - checkpoint captures from `tmux capture-pane` (plain + ANSI).
4. `report.json`:
   - structured assertion report with `matched=true/false` and artifact paths.

Artifacts are intentionally preserved for both pass and fail runs.

Tmux cleanup behavior:

1. Harness always kills its own detached session on teardown.
2. Harness also kills the buddy-managed tmux session derived from the scenario session name to prevent session leaks across runs.
3. Regression scenarios explicitly assert that the derived buddy-managed session does not exist after teardown.

## Commands

Opt-in direct cargo command:

```bash
cargo test --test ui_tmux_regression -- --ignored --nocapture
```

Makefile wrapper:

```bash
make test-ui-regression
```

## Determinism Strategy

1. The integration test starts a local scripted fake model HTTP server.
2. The fake server returns:
   - tool-call response on request #1,
   - final assistant text on request #2.
3. Responses include short delays to exercise spinner/liveness UI paths.
4. The test writes and uses an isolated `buddy.toml` profile that targets the fake server.

## Extension Guidance

When adding scenarios:

1. Keep each scenario deterministic and minimal.
2. Add explicit expected substrings for each UI element being validated.
3. Persist all relevant captures and update `report.json` schema only additively.
4. Keep tests `#[ignore]` unless intentionally moving them into default CI coverage.
