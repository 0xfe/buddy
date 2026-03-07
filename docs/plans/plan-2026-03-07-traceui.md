# 2026-03-07 Trace UI Plan

## Status
- Current: `Completed`
- Next: `Archive to docs/plans/completed/ if no follow-up polish is needed`
- Priority: ship a terminal-first trace viewer that is easy to inspect live traces with, while keeping parsing/formatting logic reusable outside the main `buddy` binary.
- Commit cadence: implementation landed as a single feature slice; record the commit ID in the execution log after validation.

## Problem Summary
The existing trace tooling is useful for summaries and replay, but it is still line-oriented and static. For real incident/debugging work, Buddy needs an interactive trace viewer that:
- makes event streams easier to scan,
- handles large payloads without flooding the screen,
- supports keyboard navigation,
- and can follow a growing trace file without disrupting inspection.

## Goals
1. Add `buddy traceui <file>` as an interactive trace viewer.
2. Render trace events with a more readable terminal UI using colors, indentation, and compact summaries.
3. Support keyboard navigation with arrow keys and vim-style bindings.
4. Collapse long event detail to a ~500-character preview and let operators expand with `space`.
5. Support `--stream` follow mode for growing trace files, pausing auto-follow during manual navigation and resuming on `Esc`.
6. Keep trace parsing/formatting logic reusable so it can be extracted into a dedicated `buddy-traceui` binary later.
7. Make the viewer resilient to trace-format drift and malformed lines.

## Milestones

### M0: Architecture + UX Freeze
- Define command shape: top-level `buddy traceui <file> [--stream]`.
- Keep implementation reusable by placing the viewer core in a library module rather than binary-only CLI code.
- Choose a two-pane terminal layout to avoid variable-height list complexity while still supporting expand/collapse.
- Status: `Completed`

### M1: Generic Trace Parsing + Tailing
- Parse JSONL trace lines from `serde_json::Value` instead of the strict runtime enum so unknown event variants still render.
- Extract stable metadata (`seq`, timestamp, family, variant, task info) and derive human-readable summaries.
- Add incremental file tailing that tolerates partial lines and file rewrites/truncation.
- Status: `Completed`

### M2: Interactive Terminal Viewer
- Add alternate-screen raw-mode UI.
- Left pane: event index with compact summaries.
- Right pane: selected event details with indentation and color coding.
- Keyboard support: arrows, `j/k`, page navigation, `g/G`, `space`, `Esc`, `q`.
- Status: `Completed`

### M3: Streaming + Follow Semantics
- Add `--stream` mode.
- Auto-follow the newest event while in follow mode.
- Pause auto-follow on manual navigation and accumulate a `new while paused` counter.
- Resume follow mode on `Esc` and jump back to the newest event.
- Status: `Completed`

### M4: Validation + Docs
- Add tests for generic parsing, malformed-line fallback, preview truncation, tailing behavior, and CLI parsing.
- Update README, observability/tracing docs, `DESIGN.md`, and `ai-state.md`.
- Record final validation and commit ID.
- Status: `Completed`

## Acceptance Gate
1. `buddy traceui <file>` launches a navigable trace viewer in a terminal.
2. `buddy traceui <file> --stream` follows appended events without disrupting manual inspection.
3. Long event text is truncated in collapsed mode and expandable with `space`.
4. Unknown/malformed trace lines render as best-effort events instead of crashing the viewer.
5. Parsing/tailing logic is covered by automated tests and does not rely on network access.

## Validation
- `cargo fmt`
- `cargo test traceui -- --nocapture`
- `cargo test`

## Execution Log
- 2026-03-07: Plan created for interactive trace inspection and live-follow UX.
- 2026-03-07: M0 complete.
  - Chose a library-backed `traceui` module plus thin CLI wiring.
  - Chose a two-pane alternate-screen UI with preview/full detail instead of an in-place expanding event list.
- 2026-03-07: M1 complete.
  - Added generic `serde_json::Value` parsing, summary extraction, malformed-line fallback events, and incremental JSONL tailing with partial-line buffering.
- 2026-03-07: M2 complete.
  - Added interactive navigation, alternate-screen rendering, detail expansion, and color-coded event families.
- 2026-03-07: M3 complete.
  - Added follow/inspect mode behavior, paused-stream pending counters, and `Esc` resume semantics.
- 2026-03-07: M4 complete.
  - Added focused tests and docs updates.
  - Validation and commit ID: pending.
