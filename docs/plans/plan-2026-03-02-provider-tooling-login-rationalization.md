# Provider Tooling And Login Rationalization Plan (2026-03-02)

## Status

- Program status: Active
- Current focus: M2 OpenAI built-in tooling and prompt contract
- Next step: implement OpenAI Responses built-in tool declaration/feature gating paths
- Completed so far:
  1. Captured scope from the request: OpenAI tooling correctness, Claude Sonnet/Haiku support, provider-scoped login/logout UX.
  2. Split execution into incremental milestones with acceptance gates, tests, docs, and commit slices.
  3. Collected canonical provider docs for OpenAI Responses tooling and Anthropic Messages/tool-use semantics.
  4. Added frozen provider/API/tooling matrix and model-ID planning targets to `docs/design/models.md`.
  5. Closed M0 baseline gate: `cargo test`, `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings` all passing.
  6. Implemented provider-first auth selector resolution with compatibility support for legacy profile selectors.
  7. Added `buddy logout [provider]` and `/logout [provider]`.
  8. Updated login output/behavior to be provider-scoped and concise, including "already logged in" fast-path guidance to use `/logout`.
  9. Updated login-missing preflight/API guidance to use provider-first commands (`buddy login <provider>` / `/login <provider>`).
  10. Synced command reference docs for provider-first login/logout surfaces.
- Blockers: none

## Scope (Locked)

1. OpenAI model/tool integration correctness:
   - align request payloads, tool declarations, and prompt guidance with current OpenAI Responses API behavior for built-in tools and function tools.
2. Claude Sonnet and Haiku support:
   - add default model entries to `buddy.toml` template,
   - add provider/API support and tool semantics in runtime,
   - explicitly do not support `auth=login` for Claude/Anthropic profiles.
3. Provider-scoped auth UX:
   - `buddy login <provider>` and `/login <provider>`,
   - `/logout` command (provider-aware),
   - if already logged in, return a short success/status message with `/logout` hint.
4. Login flow output simplification:
   - replace current verbose output with the concise message format requested by user.
5. Config/CLI rationalization:
   - treat login as provider-level identity, not model-level identity,
   - retain compatibility aliases where feasible and provide clear deprecation guidance.

## Non-Goals

1. Adding login/OAuth support for Claude providers.
2. Adding non-requested providers beyond current set plus Anthropic Claude.
3. Unrelated tmux/runtime refactors.

## Plan Maintenance Rules

1. Keep this plan in `docs/plans/` while active; move to `docs/plans/completed/` when closed.
2. Update `## Status`, milestone checkboxes, and `## Execution Log` while executing.
3. Commit between tasks/milestones and record commit IDs in this file.
4. Do not mark a milestone complete until code, tests, and docs are all complete.

## Milestone Board

- [x] M0: API Research And Design Freeze
- [x] M1: Provider-Scoped Auth Surface And Migration
- [ ] M2: OpenAI Built-In Tooling And Prompt Contract
- [ ] M3: Claude Sonnet/Haiku Provider Support
- [ ] M4: Login/Logout UX Simplification
- [ ] M5: Regression Coverage, Docs, And Release Validation

## M0: API Research And Design Freeze

### Scope

Create a canonical behavior matrix for OpenAI and Anthropic APIs/tooling, then freeze implementation decisions before coding.

### Tasks

1. Collect and link canonical docs for:
   - OpenAI Responses API tool invocation and built-in tools,
   - model-specific behavior notes for relevant OpenAI models,
   - Anthropic Claude API tool use and streaming/event model.
2. Build a provider/model capability matrix:
   - protocol, streaming shape, tool call shape, reasoning text availability, auth modes.
3. Map required code touchpoints (`api`, `runtime`, `slash commands`, `config`, `auth`, `templates`).
4. Define compatibility policy for old login command inputs (profile names) and new provider-first inputs.
5. Record exact expected user-visible login/logout output strings.

### Acceptance Gate

1. Capability matrix and implementation decisions documented with source links.
2. No unresolved ambiguity on API/tool payload format for OpenAI and Anthropic.
3. Baseline quality checks pass before implementation starts.

### Tests

1. `cargo test`
2. `cargo fmt --check`
3. `cargo clippy --all-targets -- -D warnings`

### Docs

1. Update `docs/design/models.md` with provider/model capability matrix.
2. Update this plan status/log with frozen decisions.

### Commit Slice

1. `docs(plan): freeze provider tooling/auth scope and API research matrix`

## M1: Provider-Scoped Auth Surface And Migration

### Scope

Make provider identity the single source of truth for login state across CLI, slash commands, and auth storage.

### Tasks

1. Update command parsing to accept `buddy login <provider>` and `buddy logout <provider?>`.
2. Add provider alias mapping (`openai`, `openrouter`, `moonshot`/`kimi`, `anthropic`/`claude`).
3. Refactor auth store lookup/write APIs to provider-scoped operations only.
4. Add migration path for any existing profile-scoped login records.
5. Keep profile-based login input as compatibility alias (deprecated warning), resolved to provider.
6. Add explicit guard: Anthropic provider rejects `auth=login` with actionable error.

### Acceptance Gate

1. Provider-scoped login works for CLI and slash command paths.
2. Existing users with older auth records are migrated or receive deterministic guidance.
3. No code path requires model-profile login identity.

### Tests

1. Auth store migration tests (legacy profile key -> provider key).
2. CLI parsing tests for provider and alias forms.
3. Runtime auth resolution tests.
4. `cargo test`

### Docs

1. Update `docs/developer/REFERENCE.md` command/auth sections.
2. Update `README.md` auth quickstart examples.

### Commit Slices

1. `refactor(auth): make login identity provider-scoped with compatibility aliases`
2. `feat(cli): add provider-first login/logout command parsing`

## M2: OpenAI Built-In Tooling And Prompt Contract

### Scope

Ensure OpenAI models receive the correct request/tool/prompt structures so built-in and function tools are fully usable.

### Tasks

1. Align OpenAI request builder with current Responses API expectations for:
   - function tools,
   - built-in tools (for example web search and code interpreter/python where supported),
   - tool choice behavior and turn-level tool result flow.
2. Update system prompt/tool guidance so model behavior prefers proper tool usage and avoids unsupported patterns.
3. Add provider/model feature gating so unsupported tools are not advertised.
4. Validate reasoning/analysis output handling remains readable and does not leak raw protocol internals.
5. Add configuration knobs as needed for enabling/disabling built-in tools per profile.

### Acceptance Gate

1. OpenAI profile flows can invoke configured built-in tools and function tools correctly.
2. No schema/parameter validation errors for supported OpenAI models in regression tests.
3. Tool and prompt instructions are consistent and documented.

### Tests

1. OpenAI payload unit tests (snapshot/shape assertions).
2. Runtime integration tests with mock OpenAI responses covering tool call loops.
3. Opt-in model regression checks for default OpenAI profiles.

### Docs

1. Update `docs/design/models.md` OpenAI section with concrete tooling behavior.
2. Update `docs/design/prompt.md` prompt contract notes for built-in tools.

### Commit Slice

1. `feat(openai): align responses tooling contract and prompt guidance`

## M3: Claude Sonnet/Haiku Provider Support

### Scope

Add Anthropic Claude model profiles and runtime provider support with correct API/tool semantics and no login auth mode.

### Tasks

1. Add `claude-sonnet` and `claude-haiku` model profiles to default template config.
2. Implement provider transport/mapping for Claude API request/response/tool events.
3. Map tool definitions/results to/from internal runtime event model.
4. Ensure auth modes for Anthropic are API-key only (env/file/store), with explicit rejection of `auth=login`.
5. Add model/protocol metadata so `/model` selection and runtime reporting are correct.

### Acceptance Gate

1. Claude profiles can run prompt + tool workflows through buddy runtime.
2. Unsupported auth mode paths fail fast with clear instructions.
3. No regressions for existing OpenAI/OpenRouter/Moonshot providers.

### Tests

1. Provider transport unit tests for Claude request/response mapping.
2. Tool loop integration tests with mock Claude events.
3. Opt-in model regression entries for Claude profiles (skipped when keys unavailable).
4. `cargo test`

### Docs

1. Update `src/templates/buddy.toml` comments/examples.
2. Update `docs/design/models.md` Claude provider section.
3. Update `docs/developer/model-regression-tests.md` for new profiles.

### Commit Slice

1. `feat(anthropic): add claude sonnet/haiku provider support and template profiles`

## M4: Login/Logout UX Simplification

### Scope

Deliver the requested concise login flow messaging and add logout UX parity.

### Tasks

1. Implement `/logout` slash command with provider resolution rules.
2. Add `buddy logout <provider?>` CLI command.
3. If credentials already exist, `login` prints short "already logged in" status and `/logout` hint.
4. Replace login banner/copy with requested minimal format:
   - `logging you into <provider> via <url>`
   - `device code: <code>`
   - helper line explaining browser/code step.
5. Remove noisy login health blocks unless explicitly requested via verbose/debug mode.

### Acceptance Gate

1. Login output matches requested concise format.
2. Already-logged-in path is short and includes logout guidance.
3. Logout clears provider credentials and confirms result.

### Tests

1. CLI/login UX tests (golden output fragments).
2. Slash command tests for `/logout`.
3. Auth store state transition tests (login -> already logged in -> logout).
4. `cargo test`

### Docs

1. Update `README.md` auth command examples.
2. Update `docs/developer/REFERENCE.md` slash/CLI auth commands.

### Commit Slice

1. `feat(auth-ui): simplify login copy and add provider-aware logout commands`

## M5: Regression Coverage, Docs, And Release Validation

### Scope

Consolidate behavior with robust tests/docs and ensure release readiness.

### Tasks

1. Extend regression coverage for provider/tool behavior differences (OpenAI vs Claude).
2. Ensure CI defaults remain offline-safe; keep external-model regressions opt-in.
3. Update all relevant docs and feature catalog references.
4. Run full local quality gates and targeted opt-in suites.
5. Final plan closure, archive this plan to `completed/`, and include commit map.

### Acceptance Gate

1. `fmt`, `clippy`, and `test` are clean.
2. Provider/tool/auth regressions pass in expected environments.
3. Docs are consistent with shipped behavior.

### Tests

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. Opt-in:
   - `cargo test --test model_regression -- --ignored --nocapture`
   - UI harness smoke if auth/login rendering changed

### Docs

1. `docs/design/feature-catalog.md`
2. `docs/design/models.md`
3. `docs/developer/REFERENCE.md`
4. `README.md`

### Commit Slice

1. `test+docs: finalize provider tooling/login coverage and documentation`

## Risks And Mitigations

1. API behavior drift across providers/models:
   - mitigate with source-linked matrix and payload-shape tests.
2. Backward compatibility break from provider-scoped login shift:
   - mitigate with aliases + deprecation messages + migration tests.
3. Tooling behavior inconsistencies between providers:
   - mitigate with explicit capability gating and per-provider prompt/tool contracts.

## M0 Research Notes (Frozen Decisions)

1. OpenAI Responses:
   - Continue using `/responses` for GPT-5 Codex/Spark profiles.
   - Preserve function tools and add support for provider-native built-ins where configured (`web_search`, `code_interpreter`).
   - Keep strict message/input mapping required by Responses (assistant text as `output_text`, tool outputs as `function_call_output`).
2. Anthropic:
   - Implement dedicated provider transport for `/v1/messages` (not OpenAI-compatible wire shape).
   - Add Claude profile defaults using alias IDs (`claude-sonnet-4-5`, `claude-haiku-4-5`).
   - Enforce API-key-only auth for Anthropic (`auth=login` rejected with guidance).
3. Provider-scoped auth UX:
   - Normalize login/logout commands around provider identity (not model profile identity).
   - Keep profile-selector compatibility aliases with deprecation warning during migration window.
4. Primary code touchpoints for implementation:
   - CLI and slash surface: `src/cli.rs`, `src/tui/commands.rs`, `src/app/entry.rs`, `src/app/repl_mode.rs`
   - Auth/provider mapping: `src/auth/provider.rs`, `src/auth/store.rs`, `src/api/client/auth.rs`, `src/preflight.rs`
   - Transport/protocol layers: `src/api/client/transport.rs`, `src/api/responses/*`, `src/api/completions.rs`
   - Config/template defaults: `src/config/types.rs`, `src/config/defaults.rs`, `src/templates/buddy.toml`
   - Prompt/tool advertising: `src/templates/system_prompt.template`, runtime tool registration paths

## Execution Log

- 2026-03-02: Created initial plan, locked scope, and defined milestones/gates/tests/docs for provider-tooling + auth/login rationalization work.
- 2026-03-02: Completed provider API/tooling research pass and documented frozen semantics in `docs/design/models.md`.
- 2026-03-02: Passed M0 baseline gates (`cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`) and marked M0 complete.
- 2026-03-02: Implemented M1 provider-first login/logout plumbing across CLI, slash commands, auth selector resolution, and preflight guidance.
- 2026-03-02: Verified post-change quality gates: `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` all passing.
