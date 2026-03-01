# 2026-03-01 Model Thinking Compatibility Plan

## Status

- state: completed
- current milestone: complete
- next step: monitor live provider regressions as model APIs evolve

## Goal

Make reasoning/thinking rendering consistent across configured models/providers (OpenAI Responses, OpenRouter Completions, Moonshot Completions), avoiding noisy `null`/raw-JSON blobs while preserving useful reasoning text when providers emit it.

## Milestones

### M1. Research and capability mapping

- [x] Collect primary-source API behavior for:
  - OpenAI Responses reasoning config + stream events.
  - OpenRouter reasoning request parameters + response fields.
  - Moonshot/Kimi thinking controls and `reasoning_content`.
- [x] Produce a provider/model capability matrix (request knobs + response shapes).
- [x] Identify behavior deltas between current parser and documented payloads.

### M2. Structural reasoning normalization

- [x] Add provider-aware reasoning normalization module(s) so parsing does not rely on generic key heuristics only.
- [x] Keep raw provider payloads available in `Message.extra`, but derive display-ready reasoning text separately.
- [x] Filter placeholder/noise values (`"null"`, empty arrays/objects, ID-only payloads).

### M3. Request-shape compatibility

- [x] OpenAI Responses: request reasoning summaries by default for configured OpenAI reasoning profiles.
- [x] OpenRouter Completions: send reasoning enable/include flags needed for DeepSeek V3.2 + GLM-5.
- [x] Moonshot Kimi: preserve existing behavior and ensure no regressions in tool loop continuity.

### M4. Parser/event compatibility

- [x] Extend Responses SSE reasoning handling to include additional reasoning summary event variants.
- [x] Normalize reasoning detail payloads from OpenRouter (`reasoning`, `reasoning_details`) and Moonshot (`reasoning_content`).
- [x] Ensure no reasoning block is rendered when only structural metadata is present.

### M5. Regression tests

- [x] Add protocol-level fixture tests for provider-specific reasoning payloads.
- [x] Extend model regression suite expectations to account for model-specific reasoning semantics (presence optional, noise forbidden).
- [x] Validate full suite:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
  - ignored model regression suite (manual run)

### M6. Design docs

- [x] Add `docs/design/models.md` with:
  - model/provider catalog
  - API protocol per model
  - auth mode
  - reasoning controls
  - response-shape peculiarities
  - compatibility notes and limitations
- [x] Link/update existing docs where needed.

## Execution Log

- 2026-03-01: Plan created. Started M1 source gathering and parser gap analysis.
- 2026-03-01: Added provider compatibility layer (`src/api/provider_compat.rs`) for OpenAI/OpenRouter reasoning request tuning.
- 2026-03-01: Refactored `/chat/completions` request path to build normalized payloads and parse content-array/object response variants.
- 2026-03-01: Extended `/responses` SSE parser for `response.reasoning_summary_part.*`, `response.reasoning_summary_text.done`, and `response.reasoning_text.done`.
- 2026-03-01: Hardened reasoning text normalization to suppress placeholder noise and parse JSON-encoded reasoning strings.
- 2026-03-01: Added protocol + normalization fixture tests and model-regression hygiene checks.
- 2026-03-01: Added `docs/design/models.md`, updated design links/catalog docs, and validated:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
  - `cargo test --test model_regression -- --ignored --nocapture` (all default profiles passed)
