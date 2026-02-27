# Claude Feedback Remediation Plan (2026-02-27)

## Status

- Program status: Active (integrated with `docs/plans/2026-02-27-streaming-runtime-architecture.md`)
- Current focus: Milestone 7 closed; maintain/runtime hardening and optional deferred items only (`D3`, remaining `D4`, `C2`).
- Completed so far:
  1. Streaming/runtime `S0` complete (typed runtime command/event schema).
  2. Streaming/runtime `S1` complete (agent emits runtime events with mockable model client interface).
  3. Streaming/runtime `S2` complete (interactive REPL + exec now runtime command/event-driven with runtime approval flow).
  4. Remediation Milestone 0 artifacts landed (`ai-state` tracking, `docs/playbook-remediation.md`, shared test fixtures).
  5. Remediation Milestone 1 `B1` landed (UTF-8-safe truncation helper and call-site migrations).
  6. Remediation Milestone 1 `R1` landed (centralized API/fetch timeout policy with config).
  7. Remediation Milestone 1 `S2` landed (fetch SSRF controls and domain policy).
  8. Remediation Milestone 1 `S3` landed (write-file path controls).
  9. Remediation Milestone 1 `S1` landed (shell denylist + non-interactive exec fail-closed behavior).
  10. Remediation Milestone 2 `S4` landed (machine-derived encrypted auth storage + migration + login check/reset flow).
  11. Remediation Milestone 3 landed (SSE parser hardening, transient retry/backoff, shared HTTP client reuse, and protocol diagnostics).
  12. Streaming/runtime `S3` landed (`ToolContext` + tool stream events with `run_shell` first).
  13. Streaming/runtime `S4` landed (CLI event-renderer adapter + alternate frontend example parity path).
  14. Streaming/runtime `S5` landed (runtime docs/stabilization updates across README/DESIGN/ai-state + plan sync).
  15. Remediation Milestone 4 landed (context budget hard-stop + compaction path, `/compact`, `u64` token accounting, CSPRNG session IDs, SSH control cleanup verification).
  16. Remediation Milestone 6 follow-up slices landed (`U3`, `U4`, `U5`, `B4`) with fresh model-regression validation.
  17. Remediation Milestone 7 landed (`D1`/`D2`/`C3`/`T4`) with backend extraction, deprecation timeline warnings, and feature-gated parser property tests.
- Next steps:
  1. Keep model-regression suite as a release gate whenever default provider/model profiles change.
  2. Revisit optional `D3` plugin MVP only when there is concrete operator demand.
  3. Continue reducing legacy compatibility surface (`C2`) and remaining modular cleanup (`D4`) in small slices.

## Integrated Program Board

- [x] Milestone 0: Baseline, Repro Harness, and Safety Net
- [x] Milestone 1: Immediate Security + Crash/Hang Fixes (P1)
- [x] Milestone 2: Auth and Credential Hardening (P1/P2)
- [x] Milestone 3: API Correctness and Robustness (P2)
- [x] Milestone 4: Conversation Safety and Session Robustness (P2)
- [x] Milestone 5: Testability and Modularization (P2/P3)
- [x] Milestone 6: UX Improvements (P3)
- [x] Milestone 7: Deferred Design Extensions (P4)

## Goal

Address the issues raised in `docs/plans/claude-feedback-0.md` with an incremental, low-regression execution plan that prioritizes security and correctness first, then robustness/testability, then architecture/UX cleanup.

## Planning Principles

1. Ship in small vertical slices, each with a hard gate and rollback-friendly commit.
2. Prioritize fixes that reduce risk immediately (security, panics, hangs) before large refactors.
3. Add tests before or with behavior changes, especially in high-churn surfaces (`main.rs`, API protocol handling, execution tools).
4. Keep compatibility where possible, but add explicit deprecation warnings and timelines.
5. Commit between tasks/milestones before marking a task complete, and record the commit ID in the execution log and task notes.

## Feedback Alignment and Disagreements

1. `D3` (plugin/extension mechanism): agree this is useful, but disagree with its priority now. It is high effort and low immediate risk reduction. We should defer until security/testability baselines are stronger.
2. `S4` (encrypt with machine-derived key): updated decision per product requirements. We will use a cross-platform machine-derived key encryption-at-rest scheme (no keychain dependency) so behavior is consistent for distributed binaries on macOS/Linux.
3. `U1` (no streaming output): partially outdated. Responses API transport already supports streaming ingestion internally, but incremental rendering is not surfaced to users. Treat as “partial implementation” and finish UX wiring.

## Cross-Plan Mapping (Remediation <-> Streaming Runtime)

1. Remediation Milestone 5.1 (`MockApiClient`/testability boundary) maps to streaming `S1` and is complete.
2. Remediation Milestone 5.2/5.4 (extract REPL orchestration + renderer boundary) maps to streaming `S2` and `S4`.
3. Remediation Milestone 6.1/6.2 (incremental streaming UX + runtime interface) maps to streaming `S3` and `S4`.
4. Remediation Milestone 6 acceptance gate depends on streaming plan success criteria and CLI parity tests.
5. Security/auth milestones (`M1`-`M4`) are intentionally independent and can land in parallel with streaming work.

## Milestone 0: Baseline, Repro Harness, and Safety Net

### Scope

Create repeatable repro and validation for the highest-risk behaviors before changing code.

### Tasks

1. Add a remediation tracking section in `ai-state.md` with issue IDs and status.
2. Add a `docs/playbook-remediation.md` runbook:
   - local repro commands for shell approval, fetch SSRF attempts, file write guards, large history growth, SSE edge payloads.
   - how to run offline + ignored regression suites.
3. Add a lightweight fixture module for parser/auth/execution tests to avoid repeated setup code.

### Acceptance Gate

1. Every P1/P2 issue has a reproducible failing test or documented manual repro case.
2. `cargo test` stays green before remediation starts.

### Tests

1. `cargo test`
2. `cargo test --test model_regression -- --ignored --nocapture` (optional smoke only; not required for merge gate)

### Docs

1. `ai-state.md`
2. New `docs/playbook-remediation.md`

### Commit

1. `docs(test): add remediation runbook and baseline repro matrix`

## Milestone 1: Immediate Security + Crash/Hang Fixes (P1)

### Scope

Address `S2`, `S3`, `B1`, `R1` first, then tighten `S1` to fail closed safely.

### Tasks

1. [x] `B1` UTF-8-safe truncation:
   - replace byte slicing with character-boundary-safe truncation helper used by shell/files/fetch.
2. [x] `R1` timeouts:
   - centralized `reqwest::Client` construction with default API timeout (configurable).
   - shorter timeout for `fetch_url`.
3. [x] `S2` fetch SSRF controls:
   - block loopback/link-local/RFC1918/metadata IPs by default.
   - add config `fetch_allowed_domains` and `fetch_blocked_domains`.
   - optional confirm mode for fetch (default off for compatibility, documented).
4. [x] `S3` write-file path controls:
   - add `files_allowed_paths`.
   - hard-block sensitive paths unless explicitly allowed.
   - clear error messages when denied.
5. [x] `S1` shell guardrails phase 1:
   - explicit non-interactive behavior in `exec` mode (fail closed with actionable message).
   - add config denylist with conservative dangerous patterns.
   - add CLI flag `--dangerously-auto-approve` and warning banner.

### Acceptance Gate

1. No panic on non-ASCII truncation tests.
2. Requests fail predictably on timeout in tests.
3. SSRF/write guard tests demonstrate blocked and allowed paths/domains.
4. `buddy exec` without TTY approval path returns explicit, deterministic guidance.

### Tests

1. New unit tests in `tools/shell.rs`, `tools/files.rs`, `tools/fetch.rs`, `api/*`.
2. `cargo test`

### Docs

1. `README.md` security behavior notes for shell/fetch/files.
2. `DESIGN.md` Features updates for guardrails and approval modes.
3. `docs/tools.md` policy/config reference.

### Commits

1. `fix(core): make output truncation UTF-8 safe and add shared helper`
2. `feat(net): add request timeouts and shared http client policy`
3. `feat(security): add fetch SSRF protections and domain policy`
4. `feat(security): restrict write_file paths and sensitive targets`
5. `feat(security): harden shell approval modes and dangerous auto-approve flag`

## Milestone 2: Auth and Credential Hardening (P1/P2)

### Scope

Address `S4` with provider-scoped encrypted local token storage and migration.

### Tasks

1. Add encrypted token store abstraction with machine-derived key:
   - generate/store random data-encryption key (DEK) encrypted by a machine-derived key-encryption key (KEK),
   - derive KEK using stable host/user attributes + strong KDF (Argon2id/scrypt) + per-install salt,
   - use AEAD (for example XChaCha20-Poly1305 or AES-256-GCM) for token payload encryption with nonce per record.
2. Migration:
   - read existing plaintext `auth.json` entries and migrate to encrypted format per provider.
   - redact sensitive values in logs and error paths.
3. Add explicit credential-health command path for `/login` and `buddy login` diagnostics.
4. Add corruption/recovery behavior:
   - clear actionable error when decryption fails (host changed, attributes changed, or file tampered),
   - support explicit `buddy login --reset` (or equivalent) to re-auth cleanly.

### Acceptance Gate

1. Existing login users can still authenticate after migration.
2. New logins are encrypted on disk and cannot be read as plaintext.
3. No secret values appear in stderr/stdout tests.
4. Encrypted credentials work on both macOS and Linux with identical code path.

### Tests

1. Unit tests for key derivation, encrypt/decrypt roundtrip, and tamper detection.
2. Migration tests for legacy profile-scoped/provider-scoped records.
3. Cross-platform path/permission tests (best-effort, OS-conditional).
3. `cargo test`

### Docs

1. `README.md` auth storage behavior and fallback notes.
2. `docs/architecture.md` or `DESIGN.md` auth subsystem update.
3. New `docs/auth-storage.md` operational notes and recovery steps.

### Commit

1. `feat(auth): add machine-derived encrypted provider token storage with legacy migration`

## Milestone 3: API Correctness and Robustness (P2)

### Scope

Address `B5`, `R3`, `R5`, and protocol-specific reliability gaps.

### Tasks

1. Replace line-based SSE parsing with spec-compliant event-block parser.
2. Add retry policy for transient API failures:
   - 429/5xx/network timeout with capped exponential backoff.
   - respect `Retry-After` when present.
3. Remove per-request client creation in search/auth paths; use shared client.
4. Improve protocol mismatch diagnostics:
   - pre-flight checks for `api` mode, auth availability, base URL sanity.

### Acceptance Gate

1. SSE parser passes multi-line and mixed-field tests.
2. Retry behavior deterministic in mocked HTTP tests.
3. Shared client usage validated in targeted tests and code paths.

### Tests

1. Unit tests for SSE parser fixtures and malformed events.
2. API client tests with mock server for retry/`Retry-After`.
3. `cargo test`

### Docs

1. `DESIGN.md` API behavior/retry section.
2. `docs/architecture.md` API flow update (if present), else `docs/agent-loop.md`.

### Commit

1. `fix(api): make SSE parsing spec-compliant and harden streaming decode`
2. `feat(api): add transient retry policy and shared http client reuse`

## Milestone 4: Conversation Safety and Session Robustness (P2)

### Scope

Address `R2`, `B2`, `B3`, `R4`.

### Tasks

1. [x] Add history budget enforcement:
   - refuse send above hard threshold (for example 95%).
   - pruning strategy preserving system/tool coherence.
   - optional `/compact` command for summarizing older turns.
2. [x] Promote token counters to `u64` with saturating updates.
3. [x] Replace session ID generator with CSPRNG bytes (`getrandom`).
4. [x] Ensure SSH control connection cleanup on normal exit (`Drop`/shutdown hook).

### Acceptance Gate

1. Agent refuses clearly when context budget exceeded and offers action.
2. Session IDs pass randomness/format tests and uniqueness checks.
3. SSH control master exits cleanly in integration tests.

### Tests

1. Agent history pruning/overflow behavior tests.
2. Session ID property tests (format + collision smoke).
3. Execution/SSH cleanup tests (mock shell runner where possible).
4. `cargo test`

### Docs

1. `README.md` session and context budget behavior.
2. `DESIGN.md` Features updates (`/compact`, pruning, cleanup).
3. `docs/remote-execution.md` cleanup lifecycle notes.

### Commits

1. `571b124` - `feat(core): complete milestone 4 context safety and session robustness`
2. `2fc542d` - `chore(fmt): apply rustfmt cleanup in api/auth/tool modules` (follow-up formatting pass)

## Milestone 5: Testability and Modularization (P2/P3)

### Scope

Address `T1`, `T2`, `T3`, `C1`, `C2`, partial `D4`.

### Tasks

1. Introduce API trait abstraction for agent loop tests (`MockApiClient`).
2. Extract REPL orchestration from `main.rs` into testable modules:
   - `repl_controller`
   - `approval_flow`
   - `startup`
3. Refactor config resolution shared path to eliminate duplicate logic.
4. Introduce renderer trait boundary (start with minimal interface used by controller/tests).

### Acceptance Gate

1. New agent-loop integration tests cover:
   - plain response
   - tool-call cycle
   - cancellation
   - max-iteration stop.
2. `main.rs` size reduced substantially with no behavior regressions.
3. Config resolution logic used by both runtime and tests from one path.

### Tests

1. New `tests/agent_loop.rs` (offline, deterministic).
2. New REPL controller tests in module-level test files.
3. `cargo test`

### Docs

1. `DESIGN.md` architecture sections for new controller/module boundaries.
2. `ai-state.md` module map update.

### Commits

1. `test(agent): add mock-driven agent loop integration coverage`
2. `refactor(repl): extract controller and approval flow from main`
3. `refactor(config): unify config resolution path and improve test hooks`
4. `refactor(ui): introduce renderer trait boundary for testability`

## Milestone 6: UX Improvements (P3)

### Scope

Address `U1`, `U2`, `U3`, `U4`, `U5`, and `B4`.

### Tasks

1. [x] Expose incremental runtime stream surfaces for model/tool output (`S0`-`S5` runtime track); continue iterating renderer polish as follow-up work.
2. [x] Implement the runtime/event refactor track in `docs/plans/2026-02-27-streaming-runtime-architecture.md` so streaming is a first-class library interface (not just CLI rendering).
3. [x] Improve context warnings with actionable next steps (`/compact`, `/session new`).
4. [x] Add protocol-switch warning on `/model` when API/auth mode changes.
5. [x] Add pre-flight model/profile validation to reduce cryptic API errors.
6. [x] Persist input history to `~/.config/buddy/history`.
7. [x] Upgrade DuckDuckGo parser with empty-parse diagnostics and selector-based HTML parsing.

### Acceptance Gate

1. Long model responses visibly stream content incrementally.
2. `/model` switch warns on protocol change and confirms selected behavior.
3. History persists across restarts and can be disabled by config (optional toggle).

### Tests

1. Renderer/controller tests for streaming chunks.
2. Slash command tests for switch warnings.
3. Search parser tests for attribute-order resilience.
4. `cargo test`

### Docs

1. `README.md` UX notes for streaming/history/context management.
2. `docs/terminal-repl.md` updates for `/compact` and model-switch behavior.
3. `DESIGN.md` feature list updates.

### Commits

1. `324b0d2` — `feat(runtime): introduce streaming command/event interface for model/tool/metrics flows`
2. `c877b06` — `feat(repl): add profile preflight checks and model mode-switch warnings`
3. `95b77f9` — `feat(repl): persist input history with configurable toggle`
4. `f9fba20` — `fix(search): switch to scraper-based DuckDuckGo parsing with diagnostics`

## Milestone 7: Deferred Design Extensions (P4)

### Scope

Address lower-priority architecture items after stabilization (`D1`, `D2`, `D3`, remaining `D4`, `T4`, `C3`).

### Tasks

1. [x] Introduce `ToolContext` API and migrate tools incrementally.
2. [x] Begin `ExecutionContext` backend trait extraction to reduce duplication.
3. [x] Add deprecation warnings + timeline for `AGENT_*`, `agent.toml`, `.agentx`, legacy auth keys.
4. [x] Add parser fuzz/property tests (`proptest`/`cargo-fuzz` targets).
5. [ ] Evaluate script-based tool plugin MVP only after above completes (deferred by priority).

### Acceptance Gate

1. Zero behavior regression in existing tool flows.
2. Deprecated paths emit once-per-session warning with targeted migration hint.
3. Fuzz/property tests run in CI optional job.

### Tests

1. `cargo test`
2. `cargo test --features fuzz-tests` (if gated)
3. `cargo fuzz run ...` (manual/CI nightly track)

### Docs

1. `DESIGN.md` deprecation policy and timeline.
2. `docs/tools.md` ToolContext/backends architecture.
3. New `docs/deprecations.md`.

### Commits

1. `324b0d2` — `feat(runtime): introduce streaming command/event interface for model/tool/metrics flows` (`D1` foundation)
2. `efb1116` — `refactor(exec): extract backend trait implementations from ExecutionContext`
3. `e5b09e4` — `chore(compat): add legacy deprecation warnings and migration timeline`
4. `42084dc` — `test(fuzz): add feature-gated parser property tests`
5. `feat(tools): prototype config-driven script tool loading` (optional, only if prioritized later)

## Cross-Milestone Quality Gates

1. Every milestone ends with `cargo fmt` and `cargo test` green.
2. Network regression suite (`model_regression`) run at least after Milestones 3 and 6.
3. `README.md`, `DESIGN.md` (`## Features`), and `ai-state.md` updated in the same PR as behavior changes.
4. No milestone should land with known failing tests or undocumented breaking behavior.

## Suggested Delivery Sequence

1. Milestone 0-1 in one short cycle (security and crash/hang risk reduction first).
2. Milestone 2-3 next (auth and API correctness to stabilize provider behavior).
3. Milestone 4-5 after reliability baseline.
4. Milestone 6 once core stability is strong.
5. Milestone 7 only after prior milestones are complete or explicitly re-prioritized.

## Execution Log

- 2026-02-27: Plan drafted from `claude-feedback-0.md` with priority-order milestones and acceptance gates.
- 2026-02-27: Integrated this plan with the streaming runtime plan:
  - Added synchronized status and integrated board.
  - Added explicit cross-plan mapping for Milestones 5/6 to streaming `S1`-`S4`.
  - Set immediate next steps to complete `S2` and begin Milestone 0/1 remediation slices.
- 2026-02-27: Completed Milestone 0 baseline artifacts:
  - Added remediation runbook: `docs/playbook-remediation.md` with reproducible commands/matrix.
  - Added remediation tracking section to `ai-state.md`.
  - Added shared fixture module `src/testsupport.rs` and adopted it in parser/files tests.
  - Validation: `cargo test` passed (`208` lib, `31` bin, doc-tests pass).
- 2026-02-27: Completed Milestone 1 `B1` (UTF-8 truncation safety):
  - Added shared helpers in `src/textutil.rs` for UTF-8-safe truncation by bytes/chars.
  - Migrated truncation call sites in `shell`, `files`, `fetch`, `capture-pane`, `main` preview, `runtime` preview, and TUI text helpers.
  - Added regression tests for UTF-8 truncation behavior in the updated modules.
  - Validation: `cargo fmt --all` and `cargo test` passed (`218` lib, `31` bin, doc-tests pass).
- 2026-02-27: Completed Milestone 1 `R1` (HTTP timeout policy):
  - Added `[network]` config (`api_timeout_secs`, `fetch_timeout_secs`) and env overrides (`BUDDY_API_TIMEOUT_SECS`, `BUDDY_FETCH_TIMEOUT_SECS`).
  - `ApiClient` now uses a centralized `reqwest::Client` with configured timeout.
  - `FetchTool` now owns a timeout-configured `reqwest::Client`; `main.rs` wires `fetch_timeout_secs`.
  - Added timeout behavior tests for API client and fetch tool using local hanging socket fixtures.
  - Validation: `cargo test` passed (`219` lib, `31` bin, doc-tests pass).
- 2026-02-27: Checkpoint commit created for integrated runtime/remediation baseline and Milestone 0/B1/R1 slices.
  - commit: `ace8000`
- 2026-02-27: Completed Milestone 1 `S2` (fetch SSRF policy + confirm controls):
  - Added default fetch target blocking for localhost/private/link-local IP ranges.
  - Added `tools.fetch_allowed_domains` and `tools.fetch_blocked_domains` policy config.
  - Added optional `tools.fetch_confirm` and interactive approval-broker integration.
  - Updated docs/template/config tests for new policy fields.
  - Validation: `cargo test` passed (lib/bin/doc tests).
  - commit: `26e389d`
- 2026-02-27: Completed Milestone 1 `S3` (write-file path controls):
  - Added `tools.files_allowed_paths` allowlist config for `write_file`.
  - Added sensitive-directory blocklist with explicit allowlist override.
  - Enforced policy in `WriteFileTool` with deterministic deny messages.
  - Validation: `cargo test` passed.
  - commit: `81325b0`
- 2026-02-27: Completed Milestone 1 `S1` (shell guardrails phase 1):
  - Added `tools.shell_denylist` config with conservative dangerous patterns and enforcement in `run_shell`.
  - Added `buddy exec` fail-closed behavior when `tools.shell_confirm=true` without interactive approval path.
  - Added CLI escape hatch `--dangerously-auto-approve` with explicit warning path.
  - Validation: `cargo test` passed.
  - commit: `e5ad7ee`
- 2026-02-27: Completed streaming/runtime `S2` integration gate for Milestones 5/6:
  - Migrated interactive REPL prompt/task flow to runtime command/event orchestration.
  - Added runtime approval command wiring and approval command regression tests.
  - Validation: `cargo test` passed.
  - commit: `84724e3`
- 2026-02-27: Completed Milestone 2 `S4` (auth hardening):
  - Added machine-derived encrypted auth store with DEK wrapping and AEAD per-token records.
  - Added legacy plaintext migration, tamper/error recovery messaging, and credential reset flow.
  - Added login diagnostics (`buddy login --check`) and reset path (`buddy login --reset`) used by both CLI and REPL login flow.
  - Added auth storage docs (`docs/auth-storage.md`) and auth regression tests.
  - Validation: `cargo test` passed.
  - commit: `84724e3`
- 2026-02-27: Completed Milestone 3 (API correctness/robustness):
  - Replaced line-based Responses SSE parsing with event-block parsing that honors multiline `data:` payloads and comments.
  - Added transient retry/backoff policy in `ApiClient` (429/5xx/timeouts/connectivity) with `Retry-After` support and protocol mismatch hints on 404s.
  - Added shared HTTP client reuse for auth flows and web search tool executions.
  - Validation: `cargo test` passed; `cargo test --test model_regression -- --ignored --nocapture` ran and failed only for missing `MOONSHOT_API_KEY` on `kimi` profile.
  - commit: `e4cf33c`
- 2026-02-27: Completed streaming/runtime `S3` + `S4` cross-milestone tasks:
  - Added `ToolContext` + `ToolStreamEvent` support in the tool interface/registry and wired runtime tool stream events through `Agent`.
  - Added runtime tool event variants for incremental output and updated `run_shell` to emit stream events.
  - Introduced `src/cli_event_renderer.rs` to decouple runtime-event rendering from `main.rs`.
  - Added alternate frontend parity example (`examples/alternate_frontend.rs`) consuming runtime command/event APIs directly.
  - Validation: `cargo test` and `cargo build --examples` passed.
  - commit: `324b0d2`
- 2026-02-27: Completed streaming/runtime `S5` stabilization handoff:
  - Synced runtime/tool-stream architecture docs across `README.md`, `DESIGN.md`, and `ai-state.md`.
  - Updated integrated remediation/streaming plan statuses for completed `S0`-`S5` runtime track.
  - Validation: `cargo test -q` passed.
  - commit: `6858f9c`
- 2026-02-27: Completed remediation Milestone 4 (conversation safety/session robustness):
  - Added hard context budgeting with warning + refusal path, auto-compaction under pressure, and explicit `/compact` runtime command flow.
  - Promoted token accounting (`usage`, tracker totals, runtime metrics) to `u64` with saturating updates.
  - Switched generated session IDs to OS-backed CSPRNG bytes (`xxxx-xxxx-xxxx-xxxx`) and added coverage for format/distinctness.
  - Added SSH control-master cleanup verification hook test to ensure shutdown cleanup is exercised on drop.
  - Validation: `cargo test -q` passed.
  - commits: `571b124`, `2fc542d`
- 2026-02-27: Re-ran live provider regression gate before closing Milestone 6:
  - Validation command: `cargo test --test model_regression -- --ignored --nocapture`.
  - Result: all default template profiles passed (`gpt-codex`, `gpt-spark`, `kimi`, `openrouter-deepseek`, `openrouter-glm`).
- 2026-02-27: Completed Milestone 6 follow-up slice `U3` + `U4`:
  - Added shared preflight validation module (`src/preflight.rs`) for startup + model-switch checks (base URL, model name, auth readiness).
  - Runtime model switching now emits explicit API/auth mode-change warnings and includes selected API/auth in `ProfileSwitched` events.
  - Validation: `cargo test -q` passed.
  - commit: `c877b06`
- 2026-02-27: Completed Milestone 6 follow-up slice `U5`:
  - Added REPL history load/save (`~/.config/buddy/history`) with compatibility fallback for line-based files.
  - Added config toggle `[display].persist_history` and template/docs updates.
  - Validation: `cargo test -q` passed.
  - commit: `95b77f9`
- 2026-02-27: Completed Milestone 6 follow-up slice `B4`:
  - Migrated `web_search` parsing from string splitting to CSS-selector parsing using `scraper`.
  - Added empty-parse diagnostics so parser breakage is distinguishable from true no-results pages.
  - Added parser resilience tests (attribute reordering, fallback extraction, limit enforcement).
  - Validation: `cargo test -q` passed.
  - commit: `f9fba20`
- 2026-02-27: Completed Milestone 7 slice `D2`:
  - Refactored `ExecutionContext` to store `Arc<dyn ExecutionBackendOps>` and moved backend behavior into concrete backend impls.
  - Added shared `CommandBackend` helper contract to deduplicate shell-backed `read_file`/`write_file` paths across local tmux/container/ssh backends.
  - Validation: `cargo test -q` passed.
  - commit: `efb1116`
- 2026-02-27: Completed Milestone 7 slice `C3`:
  - Added load-time config diagnostics (`load_config_with_diagnostics`) and startup warnings for deprecated `AGENT_*`, `agent.toml`, legacy `[api]`, `.agentx`, and legacy auth profile records.
  - Added migration policy doc `docs/deprecations.md` and updated README/DESIGN/docs/tools references.
  - Validation: `cargo test -q` passed.
  - commit: `e5b09e4`
- 2026-02-27: Completed Milestone 7 slice `T4`:
  - Added feature-gated parser property tests (`--features fuzz-tests`) for Responses SSE event parsing and shell wait-duration parsing.
  - Added `fuzz-tests` feature wiring in `Cargo.toml` and remediation playbook command coverage.
  - Validation: `cargo test -q` and `cargo test -q --features fuzz-tests` passed.
  - commit: `42084dc`
- 2026-02-27: Milestone 7 closed.
  - Scope landed: `D1` foundation + `D2` + `C3` + `T4`.
  - Deferred by priority: optional `D3` plugin MVP.
