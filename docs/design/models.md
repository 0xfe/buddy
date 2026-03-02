# Model and Provider Compatibility

This document describes how Buddy configures and normalizes model behavior across providers, with emphasis on reasoning/thinking output.

## Scope

Buddy currently ships these default profiles:

- `gpt-codex` (OpenAI, `/responses`, `auth = "login"`)
- `gpt-spark` (OpenAI, `/responses`, `auth = "login"`)
- `openrouter-deepseek` (OpenRouter, `/chat/completions`, API key)
- `openrouter-glm` (OpenRouter, `/chat/completions`, API key)
- `kimi` (Moonshot, `/chat/completions`, API key)
- `claude-sonnet` (Anthropic, `/v1/messages`, API key only)
- `claude-haiku` (Anthropic, `/v1/messages`, API key only)

## M0 Provider/API Freeze (2026-03-02)

This section freezes provider semantics before implementation so request/response
and tool-loop behavior stays consistent across refactors.

| Provider | Primary API in Buddy | Tool shape Buddy must emit | Tool-loop shape Buddy must consume | Auth modes in Buddy |
| --- | --- | --- | --- | --- |
| OpenAI | `/responses` (default for GPT-5 Codex/Spark) | Function tools (`type=function`) plus supported built-in tools (`web_search`, `code_interpreter`) | Responses API function-call + function-call-output flow | `api-key`, `login` |
| OpenRouter | `/chat/completions` | OpenAI-compatible function tools | OpenAI-compatible tool calls/messages with provider-specific reasoning fields | `api-key` |
| Moonshot | `/chat/completions` | OpenAI-compatible function tools | OpenAI-compatible tool calls/messages with `reasoning_content` variants | `api-key` |
| Anthropic | `/v1/messages` | `tools: [{name, description, input_schema}]` (custom tools) | Assistant `tool_use` blocks followed by user `tool_result` blocks | `api-key` only (`login` not supported) |

## OpenAI Tooling Contract (Frozen)

For OpenAI Responses API profiles, Buddy should align to these constraints:

- Built-in tool types use provider-native names from OpenAI docs:
  - `web_search` (and compatibility with older `web_search_preview` where needed).
  - `code_interpreter` with container settings (`container: { type: "auto" }` supported).
- Custom tools stay in the function schema expected by Responses:
  - `{ "type": "function", "name": "...", "description": "...", "parameters": { ... } }`
- Tool-loop events map through Responses function-call flow:
  - model emits function call items,
  - Buddy executes tool locally/remotely,
  - Buddy sends `function_call_output` records back on the next turn.
- Text item content types remain protocol-valid:
  - user/system input as `input_text`,
  - assistant text as `output_text`.

## Anthropic Tooling Contract (Implemented)

For Anthropic profiles, Buddy implements native Messages API semantics:

- Request format:
  - `POST /v1/messages`
  - `anthropic-version` header required.
- Custom tool declarations:
  - `tools: [{ "name": "...", "description": "...", "input_schema": { ... } }]`
- Tool-loop behavior:
  - assistant returns `tool_use` content blocks,
  - Buddy executes tool and returns user `tool_result` blocks.
- Current scope:
  - custom tool declarations are supported and mapped to/from Buddy's normalized tool model,
  - Anthropic server tools (for example versioned web/code tools) are not currently auto-enabled by Buddy.
- No login auth support:
  - Anthropic provider is API-key only in Buddy.

## Model IDs (Configured Defaults)

Anthropic docs currently expose these aliases and snapshots:

- Sonnet alias: `claude-sonnet-4-5` (latest snapshot example: `claude-sonnet-4-5-20250929`)
- Haiku alias: `claude-haiku-4-5` (latest snapshot example: `claude-haiku-4-5-20251001`)

Buddy template defaults should use stable aliases (not pinned snapshots) unless
operators explicitly prefer snapshot pinning.

## Compatibility Matrix

| Profile | Provider | API protocol | Request tuning in Buddy | Reasoning fields consumed |
| --- | --- | --- | --- | --- |
| `gpt-codex`, `gpt-spark` | OpenAI | `/responses` | `reasoning: { summary: "auto", effort: "<selected>" }` for reasoning-capable model IDs | `response.reasoning_*` SSE events, reasoning output items (`summary`, `content`) |
| `openrouter-deepseek` | OpenRouter | `/chat/completions` | `include_reasoning: true`, `reasoning: { enabled: true }` | `message.reasoning`, `message.reasoning_details`, reasoning aliases |
| `openrouter-glm` | OpenRouter | `/chat/completions` | `include_reasoning: true`, `reasoning: {}` | `message.reasoning`, `message.reasoning_details`, reasoning aliases |
| `kimi` | Moonshot | `/chat/completions` | no override (provider default thinking behavior) | `message.reasoning_content` and related reasoning keys |

Each profile can set:

- `provider = "openai" | "openrouter" | "moonshot" | "other" | "auto"`
- `auto` is the default and falls back to base-URL inference.
- explicit provider values override URL heuristics for compatibility behavior.

## Why This Exists

OpenAI-compatible APIs are not behavior-compatible in reasoning output:

- different request knobs (`reasoning`, `include_reasoning`, provider-specific flags)
- different response locations (`reasoning`, `reasoning_details`, reasoning output items, SSE event variants)
- different placeholders (`null`, empty arrays/objects, JSON-encoded strings)

Buddy preserves raw provider fields in message `extra`, then derives display text using a normalization pass that:

- extracts reasoning text from known nested structures (`summary`, `summary_text`, `reasoning_text`, `reasoning_details`)
- parses JSON-encoded reasoning strings when providers return embedded JSON blobs
- suppresses placeholder/noise values (`null`, `none`, `[]`, `{}`)
- prefers provider-specific reasoning keys first (`reasoning_stream`, `reasoning_details`, `reasoning_content`, etc.), then falls back to generic reasoning-key extraction

## OpenAI (`/responses`) Notes

Buddy requests reasoning summaries for OpenAI reasoning-capable model IDs:

```json
{
  "reasoning": { "summary": "auto" }
}
```

This improves REPL reasoning rendering for codex/gpt-5 style profiles where summary blocks may otherwise be sparse.

When an OpenAI model supports configurable reasoning effort, Buddy also sends:

```json
{
  "reasoning": { "summary": "auto", "effort": "medium" }
}
```

Buddy’s `/model` command uses a second picker step for reasoning effort only when the selected profile supports it. Unsupported profiles skip this picker.

Buddy also enables OpenAI native built-ins for GPT-5/Codex-family `/responses`
profiles:

```json
[
  { "type": "web_search" },
  { "type": "code_interpreter", "container": { "type": "auto" } }
]
```

To avoid duplicate capabilities, Buddy suppresses its local `web_search`
function tool when OpenAI built-in `web_search` is active for the profile.

Model auth capabilities are also tracked in `src/templates/models.toml`.
For example, `gpt-5.3-codex-spark` is marked login-only (`supports_api_key_auth = false`,
`supports_login_auth = true`) so preflight/regression can enforce the correct auth path.

Buddy’s SSE parser handles:

- `response.reasoning_text.delta`
- `response.reasoning_text.done`
- `response.reasoning_summary_text.delta`
- `response.reasoning_summary_text.done`
- `response.reasoning_summary_part.added`
- `response.reasoning_summary_part.done`
- `response.output_item.done` (structured reasoning items)

## OpenRouter (`/chat/completions`) Notes

Buddy enables surfaced reasoning for reasoning-capable OpenRouter profiles by default:

```json
{
  "include_reasoning": true,
  "reasoning": {}
}
```

For DeepSeek V3.2, Buddy also applies:

```json
{
  "reasoning": { "enabled": true }
}
```

Reasoning data is consumed from both plaintext (`message.reasoning`) and structured blocks (`message.reasoning_details`).

## Moonshot/Kimi Notes

Buddy keeps Moonshot chat-completions behavior intact and preserves `reasoning_content` fields across tool turns.

Kimi’s API guidance stresses keeping reasoning context in follow-up requests for multi-step tool usage; Buddy preserves provider extras for this reason.

## Test Coverage

Coverage is split across:

- protocol/unit tests:
  - OpenAI `/responses` SSE reasoning variants
  - reasoning normalization noise filtering + JSON-encoded reasoning extraction
  - OpenRouter request override injection tests
- ignored live regression suite:
  - `cargo test --test model_regression -- --ignored --nocapture`
  - probes all default profiles end-to-end
  - verifies assistant output is non-empty and reasoning payloads do not degrade to placeholder noise values

Token estimation:

- Buddy keeps a per-model runtime calibration multiplier.
- Raw heuristic estimates (character-based) are adjusted using observed provider `usage.prompt_tokens`.
- Calibration is bounded and smoothed to avoid overreacting to one outlier response.

Pricing/cost estimation:

- `src/templates/models.toml` can attach pricing metadata to model-match rules:
  - `input_price_per_mtok`
  - `output_price_per_mtok`
  - `cache_read_price_per_mtok` (optional)
- When pricing metadata exists and provider returns usage totals, Buddy emits `Metrics.Cost` runtime events with request/session USD estimates.
- Missing pricing metadata is non-fatal; cost metrics are omitted for those requests.

## Sources

- OpenAI Responses API and tool guides:
  - https://platform.openai.com/docs/api-reference/responses/create
  - https://platform.openai.com/docs/guides/tools?api-mode=responses
  - https://platform.openai.com/docs/guides/function-calling?api-mode=responses
  - https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses
  - https://platform.openai.com/docs/guides/tools-code-interpreter?api-mode=responses
- OpenAI Python SDK generated response types (reasoning config + response stream event schema):
  - https://github.com/openai/openai-python/tree/main/src/openai/types/responses
  - https://github.com/openai/openai-python/blob/main/src/openai/types/shared_params/reasoning.py
  - https://github.com/openai/openai-python/blob/main/src/openai/types/shared/reasoning_effort.py
- Anthropic APIs/models/tooling:
  - https://docs.anthropic.com/en/api/messages
  - https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/overview
  - https://docs.anthropic.com/en/docs/build-with-claude/tool-use/web-search-tool
  - https://docs.anthropic.com/en/docs/build-with-claude/tool-use/code-execution-tool
  - https://docs.anthropic.com/en/docs/about-claude/models/overview
- OpenRouter OpenAPI schema (chat + responses reasoning fields):
  - https://openrouter.ai/openapi.json
- OpenRouter reasoning docs:
  - https://openrouter.ai/docs/use-cases/reasoning-tokens
- OpenRouter models catalog API (model metadata + supported parameters):
  - https://openrouter.ai/api/v1/models
- Moonshot thinking-model guide:
  - https://platform.moonshot.ai/docs/guide/use-kimi-k2-thinking-model
