# Buddy Agent — Architecture Review & Recommendations

**Date:** 2026-03-01
**Reviewer:** Claude Opus 4.6
**Scope:** Agent loop, tool system, prompt engineering, context management, multi-model support, observability & tracing

---

## Executive Summary

Buddy is well-architected for a single-model CLI agent. The core loop is sound, error handling is thoughtful, and the tmux-centric design is unique and useful. However, several areas need attention to achieve reliable multi-model operation and prevent context-related failures at scale. The issues below are sorted by impact on the stated goal of "model never confused, does the right thing the vast majority of the time."

---

## P0 — Critical (directly causes model confusion or failure)

### 1. Tmux snapshot injection replaces the system prompt every turn

**Problem:** `refresh_dynamic_tmux_snapshot_prompt()` concatenates the tmux pane screenshot into the system message and *replaces* it on every iteration of the agent loop. This means:

- The system prompt content changes on every turn, **destroying KV cache** for the entire system prefix. Research from Manus and Anthropic shows cached tokens are 10x cheaper and significantly faster. Every turn pays full price for re-processing the entire system prompt.
- The pane content is user-controlled terminal output injected directly into the system message — a **prompt injection surface**. A malicious command output could contain instructions that override agent behavior.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Move snapshot to user message | Inject the snapshot as a synthetic user message (or append to the real user message) before each model call. System prompt stays static. | Best for KV cache. Slightly changes message structure. |
| B. Use a dedicated `context` role | Some providers support injecting context outside the main message flow. Not portable. | Provider-dependent. |
| C. Keep current approach but add injection filtering | Sanitize pane content to remove potential prompt injection patterns. | Doesn't fix cache. |

**Recommendation:** Option A. This is the single highest-impact change. Anthropic, Augment Code, and Manus all converge on: **static system prompt prefix + dynamic content in user/assistant messages**.

---

### 2. No tool-call/result pairing protection during compaction

**Problem:** `compact_history_with_budget()` removes "turns" starting at user messages, but tool-call/result pairs can span message boundaries. If compaction removes an assistant message with `tool_calls` but keeps the corresponding `tool` result messages (or vice versa), the next API call will fail or confuse the model. The Strands Agents framework documentation and Anthropic's guidelines explicitly state: **never break tool call/result pairs**.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Group by tool-call pairs | Treat assistant+tool_results as an atomic unit during compaction. Never remove one without the other. | Slightly coarser granularity. |
| B. Validate after compaction | Post-compaction pass that removes orphaned tool results or tool calls. | More complex, but handles edge cases. |
| C. Both A + B | Belt and suspenders. | Most robust. |

**Recommendation:** Option A, with a validation pass (B) as a safety net.

---

### 3. Compaction summary is text-only — model loses structured context

**Problem:** When older turns are compacted, the summary is plain text with one-line descriptions like `"[tool] run_shell: {\"command\":\"ls\"}"`. The model receives this as a system message. Several issues:

- The model can't distinguish between "I ran this command" and "a summary says a command was run" — it may try to parse or re-execute summarized commands.
- Summary is capped at 24 lines regardless of information density.
- No distinction between successful and failed operations in summaries.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. LLM-based summarization | Use a fast/cheap model to summarize older turns into a coherent narrative. | Adds latency and cost. Much better quality. |
| B. Structured summary format | Use a structured format that preserves key outcomes (success/failure, key outputs) rather than raw message dumps. | Cheaper than A, better than current. |
| C. Scratchpad pattern | Maintain a persistent `todo.md`-style scratchpad that the agent updates, serving as external memory. Compaction becomes less critical. | Requires a new tool. Proven effective at Manus. |

**Recommendation:** Option B short-term (structured summaries with outcome labels), Option C medium-term (scratchpad for goal persistence).

---

## P1 — High (causes problems with some models or degrades reliability)

### 4. System prompt is not structured for multi-model consumption

**Problem:** The system prompt uses informal language and assumes a specific model's comprehension style. Key issues for multi-model support:

- **No clear section delimiters.** Research shows XML tags or markdown headers significantly improve instruction following across models. The current prompt is mostly flowing prose.
- **Critical rules buried in the middle.** The tmux execution model (6-point list) is mid-prompt. Models attend most strongly to the beginning and end of system prompts ("primacy and recency bias"). Instructions in the middle are the most likely to be ignored.
- **Mixed concerns.** Tool usage rules, behavioral guidelines, output format, and domain knowledge are interspersed rather than in dedicated sections.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Restructure with XML tags | Wrap sections in `<role>`, `<tools>`, `<rules>`, `<output_format>` tags. Place most critical rules first and last. | Works well across all models. Slightly more verbose. |
| B. Restructure with markdown headers | Use `## Role`, `## Tools`, `## Rules`, etc. | Slightly less precise than XML for some models. |
| C. Hybrid | XML for structural delineation, markdown within sections for readability. | Best of both worlds. |

**Recommendation:** Option A or C. Put the most critical behavioral rules (output format, tool usage rules) at the very beginning and repeat the most important ones at the end.

---

### 5. Tool descriptions lack "when NOT to use" guidance and examples

**Problem:** Tool definitions have decent `description` fields, but research (Anthropic's "Building Effective Agents") shows models need:

- **Explicit disambiguation** between similar tools (e.g., `run_shell` vs `send-keys` — when to use which?)
- **"When NOT to use"** guidance (reduces tool selection errors)
- **Example invocations** in descriptions (especially for complex parameter combinations like `wait`, `delay`, `session`/`pane`)

The `run_shell` tool has 7 parameters including risk metadata. Without examples, models (especially weaker ones) frequently get the parameter combination wrong.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Enrich tool descriptions | Add "When to use", "When NOT to use", and 1-2 example invocations to each tool definition. | Increases token cost (~200-400 tokens per tool). Worth it for reliability. |
| B. Move disambiguation to system prompt | Keep tool definitions lean, put usage rules in the system prompt. | Decouples concerns. Harder to maintain consistency. |
| C. Both, with system prompt as authority | Tool definitions have basics + examples; system prompt has the authoritative rules. | Best reliability. Slightly redundant. |

**Recommendation:** Option A. Apply the "junior developer test" — would a junior dev know exactly when to use each tool and how to call it correctly?

---

### 6. 58+ tools warning: token cost of tool definitions

**Problem:** Currently 10-12 tools, but the tool definition cost grows linearly with each tool added. Research shows 58 tools can consume ~55,000 tokens. More importantly, large tool sets increase tool selection errors.

**Current state is fine**, but this is a design constraint to be aware of as the tool set grows. The philschmid.de guide recommends capping at ~20 core tools.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Tool routing through `run_shell` | Complex or rare operations go through the general shell tool rather than getting dedicated tools. | Fewer definitions, but less structured. |
| B. Tool grouping/namespacing | Group related tools (all tmux tools → one `tmux` tool with a `subcommand` parameter). | Reduces definitions. May confuse some models. |
| C. Conditional tool registration (current approach) | Only register tools when their feature is enabled. | Already doing this. Good. |

**Recommendation:** Keep current approach. Monitor as tools are added. If approaching 20+, apply Option B.

---

### 7. No explicit "think before acting" instruction

**Problem:** The system prompt tells the agent to "default to action" but doesn't instruct it to analyze before acting. Research from Augment Code and others shows that requiring a brief planning step before tool use significantly reduces errors, especially for:

- Multi-step operations where the first step constrains later ones
- Destructive commands (the `risk`/`mutation` metadata helps, but the model should also reason about order of operations)

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Add planning instruction | "Before executing commands, briefly state what you're about to do and why." | Adds ~1 sentence per turn. Improves accuracy. |
| B. Structured reasoning block | Require `<thinking>` or similar before each tool call. | Works well with Claude/OpenAI. May confuse other models. |
| C. Rely on model's native reasoning | Models with extended thinking (Claude, o-series) already do this internally. | Not portable. |

**Recommendation:** Option A. A lightweight "state your plan before executing" instruction works across all models without relying on provider-specific features.

---

## P2 — Medium (improvement opportunities)

### 8. Token estimation is rough (1 token per 4 chars)

**Problem:** The heuristic of 1 token per 4 characters can be 20-40% off depending on content type (code vs prose, English vs other languages). Context budget decisions (warn at 80%, compact at 95%) are based on this estimate. If the estimate is too low, the agent may exceed context limits; too high, and it compacts prematurely.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Use tiktoken/tokenizer binding | Accurate counts for OpenAI models. Doesn't cover all providers. | Adds a dependency. Only accurate for one provider. |
| B. Calibrate from API responses | Use actual `usage` data from API responses to calibrate the heuristic ratio per model. Track the ratio of actual-to-estimated and adjust. | No new deps. Self-correcting. |
| C. Conservative buffer | Keep the heuristic but add a 20% safety margin to all estimates. | Simple. Wastes some context. |

**Recommendation:** Option B. When the API returns `usage`, compare to the estimate and maintain a running correction factor per model.

---

### 9. Provider compatibility is URL-based heuristic

**Problem:** `ProviderFamily` detection relies on pattern-matching the base URL (`openrouter.ai`, `api.openai.com`, etc.). This is fragile:

- Self-hosted models behind custom URLs won't match
- Proxy services (LiteLLM, custom gateways) won't match
- New providers require code changes

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Explicit provider config | Add `provider = "openai" | "openrouter" | "anthropic" | "generic"` to model profiles. Fall back to URL detection. | Most reliable. Small config addition. |
| B. Capability probing | Send a test request to detect capabilities (streaming format, tool call format). | Adds latency at startup. Most adaptive. |
| C. Keep URL detection + expand patterns | Add more URL patterns as providers emerge. | Maintenance burden. |

**Recommendation:** Option A. Make it explicit in config with URL-based fallback for backward compatibility.

---

### 10. Error traces should stay in context

**Problem:** When a tool fails, the error is pushed as a tool result (`"Tool error: {e}"`). This is correct. However, during compaction, failed tool calls are summarized the same way as successful ones — the model loses the information that something failed and why.

Research from Manus: "Leave error traces in context — they help the model correct course." Failed operations should be weighted higher during compaction because they represent learned constraints.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Tag errors in summaries | During compaction, explicitly label failed operations: `"[FAILED] run_shell ls /nonexistent: No such file"`. | Simple. Preserves failure signal. |
| B. Keep recent failures verbatim | During compaction, always keep the last N failed tool calls in full, regardless of age. | Ensures model sees exact errors. |
| C. Both | Tag in summaries + keep recent failures. | Most robust. |

**Recommendation:** Option C.

---

### 11. `sanitize_conversation_history()` runs on every turn but could be more targeted

**Problem:** Full sanitization iterates all messages before every model call. This is O(n) on message count. For long sessions (50+ tool calls), this adds up. More importantly, it re-sanitizes already-clean messages.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Sanitize on ingestion only | Clean messages when they're added to history, not before every call. | Most efficient. Requires discipline at all insertion points. |
| B. Dirty flag | Track which messages have been sanitized. Only re-sanitize dirty ones. | Moderate complexity. |
| C. Keep current approach | It works and the cost is low for typical session lengths. | Simple. |

**Recommendation:** Option C for now. Only optimize if profiling shows it matters (unlikely for <200 messages).

---

### 12. No KV cache optimization strategy

**Problem:** Beyond the system prompt issue (#1), there's no systematic approach to KV cache preservation. Every time a message is modified, re-ordered, or the system prompt changes, the entire cache is invalidated. Research shows this is the #1 cost and latency driver.

**Options:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Append-only history | Never modify existing messages. Add corrections as new messages. | Best for cache. May confuse models seeing corrections. |
| B. Stable prefix guarantee | Guarantee the first N messages (system + initial context) never change. Only append new messages. | Good balance. |
| C. Provider-specific cache control | Use Anthropic's `cache_control` breakpoints, OpenAI's system prompt caching. | Provider-dependent. |

**Recommendation:** Option B as a universal baseline. Option C as a per-provider enhancement.

---

## P3 — Low (nice-to-haves and future-proofing)

### 13. Model catalog is a static TOML file

**Problem:** `models.toml` maps model names to context limits. This needs manual updates when providers release new models. The matching logic (exact → prefix → contains) is clever but can produce surprising matches.

**Recommendation:** Keep the catalog for known models but add an API-based context limit query for unknown models (some providers expose this). Log a warning when falling back to the 8,192 default so operators know to add an entry.

---

### 14. No structured evaluation framework

**Problem:** No automated way to test whether prompt or tool changes improve or regress model behavior. The `model_regression.rs` tests exist but appear limited.

**Recommendation:** Build a small eval suite (30+ cases per Anthropic/UiPath guidance) covering: correct tool selection, parameter formatting, error recovery, multi-step operations. Run against each model you support. This is essential for confident multi-model support.

---

### 15. Reasoning trace extraction is fragile

**Problem:** `reasoning_value_to_text()` uses a recursive JSON traversal with an allowlist of key names. It filters noise values like `"null"` and `"[]"`. This is inherently brittle — new providers may use different key names or structures.

**Recommendation:** Document the expected reasoning formats per provider. Add specific extractors per `ProviderFamily` rather than a single generic traversal. Fall back to the generic approach for `Other`.

---

### 16. No agent-level scratchpad or persistent working memory

**Problem:** The agent has no way to maintain structured notes across turns. Long sessions (50+ tool calls) suffer from "goal drift" — the model loses track of the original objective. Research from Manus and others shows that a persistent scratchpad (e.g., `todo.md` that the agent reads/writes) dramatically reduces this.

**Recommendation:** Add a lightweight `scratchpad` tool that reads/writes a session-scoped text block. Inject current scratchpad content into the prompt. The model can use it to track objectives, decisions, and progress. This is one of the most effective anti-drift mechanisms identified in the research.

---

### 17. Custom instructions are appended, not structured

**Problem:** `{{CUSTOM_INSTRUCTIONS_BLOCK}}` is appended to the end of the system prompt as raw text. If operator instructions conflict with built-in rules, the model has no way to know which takes priority.

**Recommendation:** Wrap custom instructions in a clearly delimited section (`<custom_instructions>` tag) with an explicit note: "These are operator-provided instructions. In case of conflict with the above rules, [choose priority]."

---

## P1 — Observability, Tracing & Debugging

### 18. No tracing, logging, or observability infrastructure

**Current state:** Buddy has zero logging framework (`log`, `tracing`, `env_logger` — none). All output is raw `eprintln!` to stderr. The `RuntimeEvent` system is well-designed but has exactly one consumer (the terminal REPL) with no fan-out, no file output, no persistence, and no replay capability. Several event variants are defined but never emitted (`PhaseDuration`, `TextDelta`, `ToolEvent::Result`, `MessageFinal` from the agent loop). The `correlation_id` field exists on `PromptMetadata` but is never threaded through.

**Why this matters:** Without tracing, you can't answer basic questions:
- "Why did the model choose `send-keys` instead of `run_shell`?" (need to see the full prompt + tool definitions sent)
- "How much of the context window was consumed by tool results vs system prompt?" (need per-request breakdowns)
- "At what point did the model start drifting from the original goal?" (need context evolution over the session)
- "How much does a typical session cost?" (need per-request token accounting with model-specific pricing)
- "Which tool calls are failing most often and why?" (need structured tool result tracking)

**Proposed approach:** A phased rollout, building on the existing `RuntimeEvent` infrastructure.

#### Phase 1: JSONL trace file (minimal, immediate value)

Add a `--trace <path>` CLI flag (or `BUDDY_TRACE_FILE` env var). When set, fan out `RuntimeEventEnvelope` to a second sink that writes newline-delimited JSON to a file. Every envelope already has `seq`, `ts_unix_ms`, and a structured `event` — this is 90% of a trace format already.

**What to add beyond current events:**

| Data | Where to capture | Priority |
|------|-------------------|----------|
| Full messages array sent to API | Before each `client.chat()` call | High — enables prompt replay |
| Raw API response (or content + usage + finish_reason) | After each `client.chat()` call | High — enables response analysis |
| Context window token estimate | Already emitted as `ContextUsage` | Already there |
| Compaction events (pre/post message count, summary text) | In `compact_history_with_budget()` | High — context evolution |
| Request latency (wall-clock ms) | Wrap `client.chat()` | Medium |
| Time-to-first-token | In SSE parser | Medium |
| Retry attempts and backoff durations | In `RetryPolicy` | Medium |

Wire the existing but never-emitted events: `PhaseDuration` (measure tool execution time), `ToolEvent::Result` (capture tool outputs), `MessageFinal` (capture assistant response from agent loop).

Fill in `TaskRef.session_id` and `TaskRef.iteration` — they exist but are always `None`.

**Implementation:** ~200-300 lines. Add an `Option<BufWriter<File>>` to the agent or runtime, write each envelope as `serde_json::to_string(&envelope)` + newline. No new dependencies.

#### Phase 2: Structured span model (align with OTel GenAI conventions)

Layer a span hierarchy on top of the flat event stream. The industry is converging on these span kinds:

```
session (conversation)
  └─ turn (user request → final response)
       ├─ llm_call (single API request/response)
       │    ├─ gen_ai.request.model
       │    ├─ gen_ai.usage.input_tokens / output_tokens / cache_read.input_tokens
       │    ├─ gen_ai.response.finish_reasons
       │    └─ latency_ms
       ├─ tool_call (single tool execution)
       │    ├─ tool.name, tool.call.id
       │    ├─ tool.call.arguments (JSON)
       │    ├─ tool.call.result (JSON, truncated with hash if large)
       │    ├─ success/error status
       │    └─ latency_ms
       └─ compaction (if triggered)
            ├─ pre_message_count, post_message_count
            ├─ pre_token_estimate, post_token_estimate
            └─ summary_text
```

**Options for implementation:**

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Custom span model in JSONL | Define buddy-specific span types, write to the same trace file. Parse with custom tooling. | Simple. No deps. Not interoperable. |
| B. OpenTelemetry via `tracing` crate | Use `tracing` with `tracing-opentelemetry` subscriber. Emit OTel spans with GenAI semantic convention attributes. Export to Jaeger/OTLP/Langfuse/etc. | Standard. Rich ecosystem. Adds ~5 crate deps. |
| C. Hybrid: custom JSONL + OTel export | JSONL for local debugging, optional OTel export for production monitoring. | Most flexible. More code. |

**Recommendation:** Option B for the span model (the Rust `tracing` crate is lightweight and idiomatic), but start with a simple JSONL exporter before adding full OTel. The `tracing` crate also gives you `RUST_LOG`-style filtering, `#[instrument]` macros, and structured fields for free.

#### Phase 3: Analysis tooling

Build lightweight tools to analyze trace files:

| Tool | Purpose |
|------|---------|
| `buddy trace summary <file>` | Session overview: turn count, total tokens, total cost, tool call frequency, error rate |
| `buddy trace replay <file> --turn N` | Show the exact messages array sent to the model at turn N (enables "what did the model see?") |
| `buddy trace context-evolution <file>` | Show token usage over time: system prompt size, tool result accumulation, compaction events |
| `buddy trace diff <file1> <file2>` | Compare two sessions (e.g., same task on different models) |

Alternatively, export to Langfuse or Arize Phoenix and use their UIs. Both accept OTLP spans.

#### Phase 4: Cost tracking

Token counts are already tracked in `TokenTracker`, but cost requires model-specific pricing. Approach:

- Extend `models.toml` with per-model pricing fields: `input_price_per_mtok`, `output_price_per_mtok`, `cache_read_price_per_mtok`
- Calculate cost per request and emit as a `MetricsEvent::Cost` variant
- Track separately: input tokens, output tokens, cached input tokens (some providers report this)
- Session-level cost summary in trace output

---

### 19. No `--verbose` or debug mode

**Problem:** There is no way to see raw API request/response payloads, retry behavior, SSE chunk parsing, or tool execution details at runtime. The only display toggles are `show_tokens` and `show_tool_calls`.

**Proposed approach:**

Add verbosity levels via `--verbose` / `-v` flags (stackable):

| Level | What it shows |
|-------|---------------|
| Default | Current behavior (spinner, tool names, assistant response) |
| `-v` | + token counts, context usage %, request latency, retry attempts |
| `-vv` | + full tool call arguments and results (not truncated), compaction events |
| `-vvv` | + raw API request/response bodies, SSE events, HTTP headers |

Implementation: Use the `tracing` crate with `tracing-subscriber` and `EnvFilter`. Map `-v` to `info`, `-vv` to `debug`, `-vvv` to `trace`. Each component emits at the appropriate level. Also support `BUDDY_LOG=buddy::api=trace` for targeted debugging.

---

### 20. `RuntimeEvent` has dead variants and incomplete coverage

**Problem:** Several event variants are defined but never emitted. This creates a false sense of coverage.

| Event | Status |
|-------|--------|
| `PhaseDuration` | Defined, never emitted anywhere |
| `TextDelta` | Defined, never emitted (no streaming text yet) |
| `MessageFinal` | Defined, only emitted by runtime, not by agent loop |
| `ToolEvent::Result` | Defined, never emitted (legacy path renders directly) |
| `correlation_id` on `PromptMetadata` | Field exists, never read or threaded |
| `TaskRef.session_id` | Field exists, always `None` |
| `TaskRef.iteration` | Field exists, always `None` |

**Proposed approach:** As part of Phase 1 tracing work, either wire up or remove each dead variant. Specifically:

- **Wire up:** `PhaseDuration` (around tool execution and API calls), `ToolEvent::Result` (after tool execution alongside the existing rendering), `TaskRef.session_id` and `iteration`
- **Remove or mark `#[allow(dead_code)]`:** `TextDelta` (until streaming is implemented), `correlation_id` (until external tracing is integrated)
- **Fix:** `MessageFinal` — emit from the agent loop, not just the runtime wrapper

---

## Summary of Recommended Priority Actions

1. **Move tmux snapshot out of system prompt** → user message (P0, biggest single improvement)
2. **Protect tool-call/result pairs during compaction** (P0, prevents API failures)
3. **Restructure system prompt with clear section delimiters** (P1, multi-model reliability)
4. **Enrich tool descriptions** with disambiguation and examples (P1, reduces tool selection errors)
5. **Add JSONL trace file output** — `--trace` flag, fan out existing events + full prompt/response capture (P1, immediate debugging value)
6. **Add `--verbose` / `-v` flag** with `tracing` crate integration (P1, developer experience)
7. **Add explicit provider config** to model profiles (P2, reliable multi-model)
8. **Add structured compaction summaries** with error preservation (P2, better context quality)
9. **Wire up dead `RuntimeEvent` variants** or remove them (P2, code hygiene)
10. **Build span model aligned with OTel GenAI conventions** (P2, interoperability with Langfuse/Phoenix/etc.)
11. **Build eval suite** for multi-model testing (P3, confidence in changes)
12. **Add cost tracking** with per-model pricing in models.toml (P3, operational visibility)
13. **Consider scratchpad tool** for long sessions (P3, prevents goal drift)
14. **Build trace analysis CLI** — `buddy trace summary/replay/context-evolution` (P3, debugging workflows)

---

## References

- [Building Effective AI Agents — Anthropic](https://www.anthropic.com/research/building-effective-agents)
- [Effective Context Engineering for AI Agents — Anthropic Engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- [Context Engineering for AI Agents: Lessons from Building Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- [Context Engineering Part 2 — philschmid.de](https://www.philschmid.de/context-engineering-part-2)
- [11 Prompting Techniques for Better AI Agents — Augment Code](https://www.augmentcode.com/blog/how-to-build-your-agent-11-prompting-techniques-for-better-ai-agents)
- [Context Window Management Strategies — Maxim AI](https://www.getmaxim.ai/articles/context-window-management-strategies-for-long-context-ai-agents-and-chatbots/)
- [Microsoft Failure Modes Taxonomy](https://www.microsoft.com/en-us/security/blog/2025/04/24/new-whitepaper-outlines-the-taxonomy-of-failure-modes-in-ai-agents/)
- [Conversation Management — Strands Agents](https://strandsagents.com/latest/documentation/docs/user-guide/concepts/agents/conversation-management/)
- [OpenAI Responses API vs Anthropic Messages vs Chat Completions — Portkey](https://portkey.ai/blog/open-ai-responses-api-vs-chat-completions-vs-anthropic-anthropic-messages-api/)
- [KV-Cache Aware Prompt Engineering](https://ankitbko.github.io/blog/2025/08/prompt-engineering-kv-cache/)
- [Towards a Science of Scaling Agent Systems — Google Research](https://research.google/blog/towards-a-science-of-scaling-agent-systems-when-and-why-agent-systems-work/)
- [Agent Harnesses — Dev.to](https://dev.to/htekdev/agent-harnesses-why-2026-isnt-about-more-agents-its-about-controlling-them-1f24)
- [OpenAI Agents SDK Tracing](https://openai.github.io/openai-agents-python/tracing/)
- [LangSmith Observability](https://www.langchain.com/langsmith/observability)
- [Debugging Deep Agents with LangSmith](https://blog.langchain.com/debugging-deep-agents-with-langsmith/)
- [Langfuse Data Model](https://langfuse.com/docs/observability/data-model)
- [Langfuse Token and Cost Tracking](https://langfuse.com/docs/observability/features/token-and-cost-tracking)
- [OTel GenAI Semantic Conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/)
- [OTel GenAI Spans Spec](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-spans/)
- [OTel AI Agent Observability Blog](https://opentelemetry.io/blog/2025/ai-agent-observability/)
- [Datadog LLM Observability Terms](https://docs.datadoghq.com/llm_observability/terms/)
- [OpenInference Semantic Conventions — Arize](https://arize-ai.github.io/openinference/spec/semantic_conventions.html)
- [AG2 OpenTelemetry Tracing](https://docs.ag2.ai/latest/docs/blog/2026/02/08/AG2-OpenTelemetry-Tracing/)
- [Agent Context Compaction — lethain.com](https://lethain.com/agents-context-compaction/)
- [Context Engineering Compaction — Jason Liu](https://jxnl.co/writing/2025/08/30/context-engineering-compaction/)
- [Honeycomb AI/LLM Observability](https://www.honeycomb.io/use-cases/ai-llm-observability)
