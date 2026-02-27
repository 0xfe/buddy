# Model/Rendering Regression Fix Plan (2026-02-27)

## Scope

Fix the currently reported regressions:

1. Reasoning output formatting across providers:
   - Never print `null`.
   - Never dump raw JSON structures for reasoning metadata.
   - Show reasoning text only when textual reasoning is actually present.
2. Provider compatibility regressions:
   - Prevent empty assistant messages from being sent back to providers (fixes Moonshot/Kimi 400s and related cross-model contamination).
   - Verify OpenRouter DeepSeek/GLM behavior with targeted parsing and transcript hygiene.
3. Approval prompt rendering stability:
   - Eliminate long-command redraw drift/corruption.
   - Render approval requests in a stable multi-line block:
     - status line separate
     - command shown in tinted indented block
     - approval input on its own line.
4. Add model regression test suite:
   - Separate suite not run by default.
   - Explicit run command.
   - Covers default template profiles and auth modes.
5. Documentation updates for the new regression suite and troubleshooting.

## Repro/Test Strategy

- Unit tests for reasoning extraction edge cases (`null`, structured reasoning, text-only extraction).
- Unit tests for transcript sanitization (drop empty assistant messages before request dispatch).
- Unit tests for approval prompt formatting helpers and single-line prompt behavior.
- Explicit network regression suite (`#[ignore]`) that exercises all default template model profiles with tiny prompts.
- Full local offline suite: `cargo test`.
- Optional explicit network suite: `cargo test --test model_regression -- --ignored --nocapture`.

## Progress Log

- [x] Plan created.
- [x] Audit and patch reasoning extraction + transcript sanitation.
- [x] Redesign approval prompt rendering path and remove unstable inline multiline prompt behavior.
- [x] Add/verify model profile defaults alignment (including template/default-map consistency).
- [x] Implement ignored model regression integration suite.
- [x] Add docs for regression suite design/usage.
- [x] Run `cargo fmt` + `cargo test` and targeted regression checks.
- [ ] Commit with detailed message.

## Execution Notes

- Implemented text-only reasoning extraction (`src/agent.rs`):
  - suppresses `null`/metadata-only values,
  - extracts nested textual reasoning fields only,
  - avoids raw JSON structure dumps in UI traces.
- Added transcript sanitization before each request:
  - drops empty assistant turns with no tool calls,
  - trims invalid/empty tool metadata,
  - strips null/empty extra fields.
- Reworked approval rendering flow:
  - approval command now renders in a stable tinted block before input,
  - prompt line is now compact `• approve? [y/n]`,
  - liveness-line updates are disabled while waiting for approval input to avoid redraw drift.
- Added provider regression suite (`tests/model_regression.rs`, ignored by default) + usage doc (`docs/model-regression-tests.md`).
- Explicit regression run in this environment:
  - passed: `gpt-codex`, `gpt-spark`, `openrouter-deepseek`, `openrouter-glm`
  - failed preflight: `kimi` (missing `MOONSHOT_API_KEY`).

## Notes

- Keep token usage low in network tests: single short prompt per profile and strict assertion on minimal non-empty assistant text.
- For `auth = "login"` profiles, tests should fail with a clear instruction if login credentials are missing.
