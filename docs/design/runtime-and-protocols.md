# Runtime and Protocols

This document captures detailed runtime control-plane behavior and API protocol specifics.

For the high-level architectural summary, see [DESIGN.md](../../DESIGN.md).

## Runtime Actor Model

Buddy runtime is channel-driven and frontend-agnostic:

- command input: `RuntimeCommand`
- event output: `RuntimeEventEnvelope`
- one actor loop serializes command handling, task lifecycle, approvals, and event fanout
- one active prompt task at a time

Entry points:

- `spawn_runtime(...)`
- `spawn_runtime_with_agent(...)`
- `spawn_runtime_with_shared_agent(...)`

## Runtime Commands

`RuntimeCommand` supports:

- prompt submission: `SubmitPrompt`
- approval response: `Approve`
- cancellation: `CancelTask`
- policy updates: `SetApprovalPolicy`
- model switching: `SwitchModel`
- session operations:
  - `SessionNew`
  - `SessionResume`
  - `SessionResumeLast`
  - `SessionCompact`
- shutdown: `Shutdown`

### Command semantics

- `SubmitPrompt` is rejected if a prompt task is already active.
- `CancelTask` only applies to the currently active task id.
- `SwitchModel` is rejected while a task is running.
- `Shutdown` denies pending approvals and signals cancellation for active work.

## Runtime Events

All events are wrapped in `RuntimeEventEnvelope { seq, ts_unix_ms, event }`.

Event families:

- `Lifecycle`
  - runtime start/stop/config-loaded milestones
- `Session`
  - created/resumed/saved/compacted
- `Task`
  - queued/started/waiting-approval/cancelling/completed/failed
- `Model`
  - profile switched, request started, reasoning deltas, final message
- `Tool`
  - call requested, stream updates, completed, final result
- `Metrics`
  - token usage, context usage, optional phase durations
- `Warning`
- `Error`

## Prompt Task Execution Path

`spawn_prompt_task(...)` configures the shared `Agent` for runtime-stream mode:

- suppress direct live stderr output
- attach runtime event sink with task id
- wire cancellation receiver
- call `Agent::send(...)`
- restore baseline agent state on completion

Task completion is reported via `TaskDone` channel and reconciled in the actor.

## Approval Flow

Tool-level shell/fetch confirmation requests arrive via `ShellApprovalRequest` channel.

Runtime behavior:

- if policy is auto-resolving (`all`, `none`, or active `until`), request is resolved immediately
- if policy is `ask`, runtime emits `TaskEvent::WaitingApproval` and tracks a pending approval id
- frontend sends `RuntimeCommand::Approve { approval_id, decision }`
- runtime resolves the pending request and emits warning event for decision outcome

Safety default:

- if an approval request arrives with no active task context, runtime denies it.

## Session Lifecycle in Runtime

Runtime session helpers persist/restore `AgentSessionSnapshot` via `SessionStore`.

- `SessionNew`
  - persists current active snapshot
  - resets in-memory agent session
  - creates and activates new session id
- `SessionResume`
  - persists current active snapshot
  - loads requested snapshot into agent
  - refresh-saves resumed snapshot
- `SessionCompact`
  - invokes `agent.compact_history()`
  - persists compacted snapshot
  - emits summary warning
- after each task completion, runtime attempts to save active session snapshot

## Agent Loop Details

Core loop (`Agent::send`) behavior:

1. sanitize existing conversation history
2. append user message
3. enforce context budget (warning, auto-compaction, hard-limit error)
4. refresh dynamic tmux snapshot section in system prompt (when available)
5. build `ChatRequest` with message history + tool definitions
6. call model client
7. record usage metrics if provided
8. normalize assistant message and reasoning traces
9. if tool calls exist:
   - emit tool call event
   - execute tool with `ToolContext` stream channel
   - emit streamed tool events + final result event
   - append `Message::tool_result(...)`
   - continue loop
10. if no tool calls, return final content

Guardrails in loop:

- max-iteration cap (`agent.max_iterations`)
- cancellation short-circuit for model calls and tool calls
- cancellation still appends synthetic tool results for remaining tool-call ids to keep provider bookkeeping valid

## API Client Protocol Routing

The API layer normalizes both wire protocols to one internal `ChatResponse` shape.

### `/chat/completions`

- endpoint: `{base_url}/chat/completions`
- request body: internal `ChatRequest`
- response: direct parse to `ChatResponse`

### `/responses`

- endpoint: `{base_url}/responses`
- request translation:
  - system messages -> `instructions`
  - other turns -> `input` items
  - tool definitions -> responses function-tool shape
- response normalization:
  - parse output text
  - parse function calls into internal `tool_calls`
  - map usage fields (`input_tokens`, `output_tokens`, `total_tokens`)
  - preserve reasoning payloads in message `extra`

### Streaming Responses (SSE)

For streaming responses mode:

- parse multiline SSE `data:` event blocks
- collect deltas (`output_text`, reasoning summary/details)
- use `response.completed`/`response.done` payload when present
- fallback to delta-only message if completed block is missing but text exists
- error on empty stream or `response.failed`

## Auth-Driven Transport Policy

Per-profile auth/protocol can change runtime transport behavior.

- API key mode: bearer from resolved key
- Login mode:
  - load provider tokens
  - refresh near-expiry tokens
  - retry once on 401 with forced refresh
  - return login-required guidance on persistent failure

For OpenAI login-backed Responses requests:

- runtime base URL may be rewritten to ChatGPT Codex backend
- request options include `store=false` and `stream=true`

## Retry and Diagnostics

Retry policy (bounded exponential backoff):

- retries on:
  - timeout/connectivity errors
  - HTTP 429
  - HTTP 5xx
- respects `Retry-After` when available

Diagnostic hints:

- 404 responses add protocol mismatch hints:
  - `responses` mode suggests trying `api = "completions"`
  - `completions` mode suggests trying `api = "responses"`
