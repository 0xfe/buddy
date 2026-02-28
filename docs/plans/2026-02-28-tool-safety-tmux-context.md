# 2026-02-28 Tool Safety + Tmux Context Plan

## Status
- Current: `Completed`
- Next: none (all scoped items landed and validated).
- Commit cadence: commit after each milestone/task slice; include commit IDs in this plan.

## Goals
1. Add harness timestamp metadata to every tool result and mention that behavior in the system prompt.
2. When Buddy attaches to an existing tmux session/pane, capture pane screenshot text and inject it into system prompt context with guidance about missing shell prompt.
3. Support configurable agent name (`[agent].name`, default `agent-mo`) and derive default tmux session names as `buddy-<name>`.
4. Rename managed tmux window/pane name from `buddy-shared` to `shared` (keep compatibility where needed).
5. Require additional shell/send-keys safety metadata args (`risk`, `mutation`, `privesc`, `why`) and render richer approval prompts.

## Milestones

### M1: Config + tmux naming/context plumbing
- Add `agent.name` to config/default template.
- Thread agent name into execution-context builders.
- Switch default managed tmux session naming to `buddy-<agent-name>`.
- Rename managed window constant to `shared`, keeping compatibility checks for legacy window name where needed.
- Add startup tmux metadata in execution context (whether session/pane was reused + pane id) for prompt injection.
- Tests: config default/parse tests + execution naming/window tests.

### M2: Startup capture-pane prompt context
- On startup, if reusing tmux pane, capture pane text (same semantics as `capture-pane` defaults).
- Extend system prompt templating params to include optional startup pane snapshot block.
- Add explicit instruction: if no visible command prompt, call this out before running commands.
- Tests: prompt rendering unit tests for snapshot block + truncation behavior.

### M3: Tool result timestamp envelope
- Introduce shared tool-result envelope helper with `harness_timestamp` field.
- Update all tools (`run_shell`, `send-keys`, `capture-pane`, `read_file`, `write_file`, `fetch_url`, `web_search`, `time`) to return enveloped JSON string.
- Update tool-result parsing/rendering so CLI keeps human-friendly output (unwrap envelope).
- Tests: parser/render helper tests + per-tool behavior smoke tests.

### M4: Shell/send-keys safety metadata + approval UX
- Update `run_shell` and `send-keys` schemas: require `risk`, `mutation`, `privesc`, `why`.
- Add arg validation and include metadata in shell approval requests/events.
- Update approval rendering block and prompt line formatting:
  - risk level shown and colorized,
  - `why` shown as secondary line,
  - prompt line includes privileged/mutation markers when set.
- Remove noisy transient approval-granted warning display remains suppressed.
- Tests: runtime approval event mapping tests + prompt formatting tests + tool arg validation tests.

### M5: Docs + verification
- Update `README.md`, `DESIGN.md`, `docs/tools.md`, `docs/remote-execution.md`, and `ai-state.md` for new behavior.
- Run full test suite.
- Final pass on UX strings to match requested output shape.

## Validation Gates
- `cargo test -q`
- Focused tests while iterating:
  - `cargo test -q tools::execution`
  - `cargo test -q tools::shell`
  - `cargo test -q tools::send_keys`
  - `cargo test -q prompt`
  - `cargo test -q runtime`

## Execution Log
- 2026-02-28: Created plan and milestones from user requirements. Starting M1 implementation.
- 2026-02-28: M1 complete.
  - Added `[agent].name` config field with default `agent-mo`.
  - Threaded `agent.name` into local/container/ssh tmux context builders.
  - Default managed tmux session now derives from agent name as `buddy-<name>`.
  - Managed tmux window renamed to `shared` with legacy `buddy-shared` compatibility checks.
  - Added execution context support for capturing startup pane state only when reusing existing tmux pane (`capture_startup_existing_tmux_pane`).
  - Updated execution/config tests (`cargo test -q tools::execution`, `cargo test -q config::`).
- 2026-02-28: M2 complete.
  - Startup now attempts tmux pane capture only when attached to an existing managed pane.
  - System prompt templating now accepts optional startup pane snapshot context.
  - Added explicit guidance in prompt template to call out missing command prompt before running commands.
  - Added prompt rendering tests for snapshot inclusion + truncation (`cargo test -q prompt::`).
- 2026-02-28: M3 complete.
  - Added shared tool envelope helper `src/tools/result_envelope.rs`.
  - Updated built-in tools to return `{harness_timestamp, result}` envelopes.
  - Updated CLI/runtime tool-result rendering to unwrap envelope payloads for human-readable output.
  - Added envelope-aware shell-result parsing helpers/tests.
- 2026-02-28: M4 complete.
  - `run_shell` and `send-keys` schemas now require `risk`, `mutation`, `privesc`, `why`.
  - `run_shell` approval requests now carry metadata through runtime `TaskEvent::WaitingApproval`.
  - Approval UI now renders risk + reason lines and dynamic prompt markers for `(privileged)` and `(mutation)`.
  - Added/updated unit tests for required metadata validation and approval metadata propagation.
- 2026-02-28: M5 complete.
  - Updated docs: `README.md`, `DESIGN.md`, `docs/tools.md`, `docs/remote-execution.md`, `ai-state.md`.
  - Verified full suite: `cargo test -q` (all passing).
  - Finalized formatting with `cargo fmt` and re-verified `cargo test -q`.
