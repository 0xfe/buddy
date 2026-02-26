# The Agent Loop

The agent loop is the heart of the system. It drives every conversation: taking
a user message, sending it to the model, handling any tool calls the model
requests, feeding results back, and repeating until the model produces a plain
text response. This document explains that loop end to end.

---

## Overview

At the highest level, the agent is a state machine that runs inside a single
async call:

```
user input
   │
   ▼
append user message to history
   │
   ▼
┌─────────────────────────────────────┐
│  build ChatRequest (full history    │
│  + tool definitions)                │
│            │                        │
│            ▼                        │
│  POST /chat/completions             │ ◄── cancellable
│            │                        │
│            ▼                        │
│  assistant message with tool_calls? │
│     YES ──►execute each tool        │ ◄── cancellable
│            │ push tool results      │
│            └── loop back ──────────►│
│     NO  ──► return text response   │
└─────────────────────────────────────┘
```

The loop is capped at `max_iterations` (default: 20) to prevent runaway
tool-use chains.

---

## Core Structures

### `Agent` — `src/agent.rs`

The agent owns everything needed to run a conversation:

```rust
pub struct Agent {
    client: ApiClient,        // HTTP client for /chat/completions
    config: Config,           // Model name, temperature, limits, ...
    tools: ToolRegistry,      // All registered tools
    messages: Vec<Message>,   // Full conversation history
    tracker: TokenTracker,    // Token accounting
    renderer: Renderer,       // Colorized terminal output

    // Background mode wiring (see below)
    suppress_live_output: bool,
    live_output_sink: Option<(u64, mpsc::UnboundedSender<AgentUiEvent>)>,
    cancellation_rx: Option<watch::Receiver<bool>>,
}
```

`Agent::new()` initialises from a `Config` and a `ToolRegistry`. If the config
includes a system prompt, it is pushed as the first `Message::system` in
history; otherwise the history starts empty.

### `Message` history

The conversation is a flat `Vec<Message>` with four roles:

| Role | When added |
|------|-----------|
| `system` | Once at start, if a system prompt is configured |
| `user` | Each time `send()` is called |
| `assistant` | After every model response (even if it has tool calls) |
| `tool` | After each tool is executed, carrying the result |

The history grows monotonically. Old messages are never pruned; context-window
management is handled by warning the user when they approach the limit (see
[Token Tracking](#token-tracking)).

---

## The Loop in Detail — `Agent::send`

```rust
pub async fn send(&mut self, user_input: &str) -> Result<String, AgentError>
```

### Step 1 — Push the user message

```rust
self.messages.push(Message::user(user_input));
```

### Step 2 — Pre-flight context check

Before entering the loop, the token tracker estimates how full the context
window is. If it is at or above 80% it emits a warning.

### Step 3 — Build the request

Each iteration constructs a fresh `ChatRequest`:

```rust
ChatRequest {
    model: "gpt-5.2-codex",
    messages: self.messages.clone(),   // full history
    tools: Some(self.tools.definitions()),
    temperature: ...,
    top_p: ...,
}
```

If no tools are registered, the `tools` field is omitted so providers that
reject an empty array are not confused.

### Step 4 — Call the API

```rust
let response = self.client.chat(&request).await?;
```

While waiting for the API, a progress spinner is shown. If cancellation is
active, the call races against the cancellation signal:

```rust
tokio::select! {
    _ = wait_for_cancellation(&mut cancel_rx) => { return Ok("cancelled"); }
    response = self.client.chat(&request) => response?,
}
```

### Step 5 — Record token usage

If the API response includes a `usage` field (not all providers include it),
the token tracker is updated and optionally printed to the terminal.

### Step 6 — Handle reasoning traces

Some providers (e.g., DeepSeek, o1-style models) include reasoning or
thinking fields alongside the main message content in non-standard extra
fields. The agent detects any field whose name contains `reasoning`,
`thinking`, or `thought`, and renders it before continuing:

```rust
fn reasoning_traces(message: &Message) -> Vec<(String, String)> {
    message.extra.iter()
        .filter_map(|(key, value)| {
            is_reasoning_key(key).then(|| (key, reasoning_value_to_text(value)))
        })
        .collect()
}
```

Traces are rendered to stderr (or forwarded as `AgentUiEvent::ReasoningTrace`
in background mode) so they don't pollute stdout.

### Step 7 — Branch on tool calls

**If the assistant message has tool calls:**

Each `ToolCall` in the response is executed in order:

```rust
for tc in tool_calls.iter() {
    // Show what's about to run
    self.tool_call_live(&tc.function.name, &tc.function.arguments);

    // Execute (also cancellable)
    let result = self.tools.execute(&tc.function.name, &tc.function.arguments).await;

    // Push the result into history
    self.messages.push(Message::tool_result(&tc.id, &result));
}
// Loop back to step 3
continue;
```

The `tool_call_id` in the result must match the `id` from the tool call;
providers use this to correlate pairs. This pairing is preserved even
when execution is cancelled mid-batch (see [Cancellation](#cancellation)).

**If the assistant message has no tool calls:**

The `content` string is returned as the final answer:

```rust
return Ok(assistant_msg.content.unwrap_or_default());
```

### Step 8 — Iteration cap

If `iterations > max_iterations`, the loop returns `AgentError::MaxIterationsReached`
rather than running forever.

---

## Cancellation

The agent supports cooperative cancellation through a Tokio `watch` channel.
The REPL injects a `watch::Sender<bool>` when spawning a background task; the
agent holds the corresponding `Receiver`.

Cancellation is checked at three points:

1. **Before the loop starts** — fast path if already cancelled.
2. **During the API call** — via `tokio::select!`.
3. **During each tool execution** — via `tokio::select!`.

When cancellation fires mid-tool-batch, the agent must still emit a
`tool_result` message for every outstanding `tool_call_id` in order to keep
the conversation history valid for any future session resume. It fills these
with `"operation cancelled by user"`:

```rust
for remaining_tc in tool_calls.iter().skip(idx + 1) {
    self.messages.push(Message::tool_result(
        &remaining_tc.id,
        "operation cancelled by user",
    ));
}
return Ok("operation cancelled by user");
```

---

## Token Tracking

`TokenTracker` in `src/tokens.rs` keeps running totals:

```
total_prompt_tokens     — sum across all API calls this session
total_completion_tokens — sum across all API calls this session
last_prompt_tokens      — from the most recent call
last_completion_tokens  — from the most recent call
context_limit           — looked up from models.toml or config
```

When the API does not return a `usage` field, the tracker falls back to a
simple heuristic: ~1 token per 4 characters, plus 16 characters per message
for role/framing overhead.

Context limits are resolved in order:

1. Explicit `[api].context_limit` in `buddy.toml`
2. `src/templates/models.toml` catalog (exact name, prefix, or substring match)
3. Conservative fallback of 8192 tokens

---

## Session Snapshots

The agent's full conversation state can be serialised and restored:

```rust
// Save
let snapshot: AgentSessionSnapshot = agent.snapshot_session();
// Restore
agent.restore_session(snapshot);
// Clear
agent.reset_session();
```

`AgentSessionSnapshot` holds the `Vec<Message>` history plus the token
tracker's accumulated counts. The REPL saves snapshots to `.buddyx/sessions/`
after each completed prompt, and reloads them when the user runs
`/session resume`.

---

## Background Mode

When the REPL spawns a background task, it configures the agent in a special
mode where live output is routed through a channel instead of printed directly:

```rust
agent.set_live_output_suppressed(true);
agent.set_live_output_sink(Some((task_id, event_tx)));
agent.set_cancellation_receiver(Some(cancel_rx));
```

All rendering calls (`warn_live`, `token_usage_live`, `tool_call_live`, etc.)
check `suppress_live_output`. If set, they emit an `AgentUiEvent` variant
instead of writing to stderr:

```rust
enum AgentUiEvent {
    Warning         { task_id, message },
    TokenUsage      { task_id, prompt_tokens, completion_tokens, session_total },
    ReasoningTrace  { task_id, field, trace },
    ToolCall        { task_id, name, args },
    ToolResult      { task_id, name, args, result },
}
```

The foreground REPL loop reads these events and renders them in the main
thread, allowing multiple background tasks to run concurrently without
interleaving their output.

---

## Error Handling

`Agent::send` returns `Result<String, AgentError>`:

```rust
enum AgentError {
    Config(ConfigError),       // bad configuration
    Api(ApiError),             // HTTP error or bad status
    Tool(ToolError),           // tool execution failure
    EmptyResponse,             // no choices in API response
    MaxIterationsReached,      // loop cap hit
}
```

Tool errors are formatted as `"Tool error: {e}"` and pushed into the
conversation history as tool results so the model can read them and decide
what to do next. This means a failing tool call does not abort the loop;
the model might retry with different arguments or explain the failure.

---

## Example Trace

```
User: "What's the disk usage of /var?"

→ send("What's the disk usage of /var?")
  Push: user("What's the disk usage of /var?")
  context check: 3% used, OK

  Iteration 1:
    POST /chat/completions
    ← assistant { tool_calls: [{ id: "tc1", name: "run_shell",
                                  args: {"command":"du -sh /var"} }] }
    Push: assistant (with tool_call)
    execute run_shell: "du -sh /var"
    → "512M\t/var"
    Push: tool_result(id="tc1", "exit code: 0\nstdout:\n512M\t/var\n...")

  Iteration 2:
    POST /chat/completions  (history: system, user, assistant, tool)
    ← assistant { content: "The disk usage of /var is 512 MB." }
    Push: assistant (final)

→ return "The disk usage of /var is 512 MB."
```
