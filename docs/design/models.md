# Model and Provider Compatibility

This document describes how Buddy configures and normalizes model behavior across providers, with emphasis on reasoning/thinking output.

## Scope

Buddy currently ships these default profiles:

- `gpt-codex` (OpenAI, `/responses`, `auth = "login"`)
- `gpt-spark` (OpenAI, `/responses`, `auth = "login"`)
- `openrouter-deepseek` (OpenRouter, `/chat/completions`, API key)
- `openrouter-glm` (OpenRouter, `/chat/completions`, API key)
- `kimi` (Moonshot, `/chat/completions`, API key)

## Compatibility Matrix

| Profile | Provider | API protocol | Request tuning in Buddy | Reasoning fields consumed |
| --- | --- | --- | --- | --- |
| `gpt-codex`, `gpt-spark` | OpenAI | `/responses` | `reasoning: { summary: "auto" }` for reasoning-capable model IDs | `response.reasoning_*` SSE events, reasoning output items (`summary`, `content`) |
| `openrouter-deepseek` | OpenRouter | `/chat/completions` | `include_reasoning: true`, `reasoning: { enabled: true }` | `message.reasoning`, `message.reasoning_details`, reasoning aliases |
| `openrouter-glm` | OpenRouter | `/chat/completions` | `include_reasoning: true`, `reasoning: {}` | `message.reasoning`, `message.reasoning_details`, reasoning aliases |
| `kimi` | Moonshot | `/chat/completions` | no override (provider default thinking behavior) | `message.reasoning_content` and related reasoning keys |

## Why This Exists

OpenAI-compatible APIs are not behavior-compatible in reasoning output:

- different request knobs (`reasoning`, `include_reasoning`, provider-specific flags)
- different response locations (`reasoning`, `reasoning_details`, reasoning output items, SSE event variants)
- different placeholders (`null`, empty arrays/objects, JSON-encoded strings)

Buddy preserves raw provider fields in message `extra`, then derives display text using a normalization pass that:

- extracts reasoning text from known nested structures (`summary`, `summary_text`, `reasoning_text`, `reasoning_details`)
- parses JSON-encoded reasoning strings when providers return embedded JSON blobs
- suppresses placeholder/noise values (`null`, `none`, `[]`, `{}`)

## OpenAI (`/responses`) Notes

Buddy requests reasoning summaries for OpenAI reasoning-capable model IDs:

```json
{
  "reasoning": { "summary": "auto" }
}
```

This improves REPL reasoning rendering for codex/gpt-5 style profiles where summary blocks may otherwise be sparse.

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

## Sources

- OpenAI Python SDK generated response types (reasoning config + response stream event schema):
  - https://github.com/openai/openai-python/tree/main/src/openai/types/responses
  - https://github.com/openai/openai-python/blob/main/src/openai/types/shared_params/reasoning.py
- OpenRouter OpenAPI schema (chat + responses reasoning fields):
  - https://openrouter.ai/openapi.json
- OpenRouter reasoning docs:
  - https://openrouter.ai/docs/use-cases/reasoning-tokens
- OpenRouter models catalog API (model metadata + supported parameters):
  - https://openrouter.ai/api/v1/models
- Moonshot thinking-model guide:
  - https://platform.moonshot.ai/docs/guide/use-kimi-k2-thinking-model
