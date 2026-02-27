# Claude Feedback Remediation Plan (2026-02-27)

## Status

- Program status: Active (integrated with `docs/plans/2026-02-27-streaming-runtime-architecture.md`)
- Current focus: finish runtime refactor milestones needed for remediation Milestones 5/6 while starting P1 security fixes.
- Completed so far:
  1. Streaming/runtime `S0` complete (typed runtime command/event schema).
  2. Streaming/runtime `S1` complete (agent emits runtime events with mockable model client interface).
  3. Streaming/runtime `S2` partial (runtime actor core and `buddy exec` path migrated).
  4. Remediation Milestone 0 artifacts landed (`ai-state` tracking, `docs/playbook-remediation.md`, shared test fixtures).
  5. Remediation Milestone 1 `B1` landed (UTF-8-safe truncation helper and call-site migrations).
  6. Remediation Milestone 1 `R1` landed (centralized API/fetch timeout policy with config).
  7. Remediation Milestone 1 `S2` landed (fetch SSRF controls and domain policy).
- Next steps:
  1. Complete runtime `S2` interactive-path migration and approval command wiring.
  2. Execute remaining Milestone 1 P1 fixes in small gated commits (`S3`, `S1`).
  3. Keep Milestones 5/6 checkpoints synchronized with streaming `S2`-`S4`.

## Integrated Program Board

- [x] Milestone 0: Baseline, Repro Harness, and Safety Net
- [ ] Milestone 1: Immediate Security + Crash/Hang Fixes (P1)
- [ ] Milestone 2: Auth and Credential Hardening (P1/P2)
- [ ] Milestone 3: API Correctness and Robustness (P2)
- [ ] Milestone 4: Conversation Safety and Session Robustness (P2)
- [ ] Milestone 5: Testability and Modularization (P2/P3) - in progress via streaming runtime milestones
- [ ] Milestone 6: UX Improvements (P3) - in progress via streaming runtime milestones
- [ ] Milestone 7: Deferred Design Extensions (P4)

## Goal

Address the issues raised in `docs/plans/claude-feedback-0.md` with an incremental, low-regression execution plan that prioritizes security and correctness first, then robustness/testability, then architecture/UX cleanup.

## Planning Principles

1. Ship in small vertical slices, each with a hard gate and rollback-friendly commit.
2. Prioritize fixes that reduce risk immediately (security, panics, hangs) before large refactors.
3. Add tests before or with behavior changes, especially in high-churn surfaces (`main.rs`, API protocol handling, execution tools).
4. Keep compatibility where possible, but add explicit deprecation warnings and timelines.
5. Commit between tasks/milestones, and record the commit ID in the execution log when a task is marked complete.

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
4. [ ] `S3` write-file path controls:
   - add `files_allowed_paths`.
   - hard-block sensitive paths unless explicitly allowed.
   - clear error messages when denied.
5. [ ] `S1` shell guardrails phase 1:
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

1. Add history budget enforcement:
   - refuse send above hard threshold (for example 95%).
   - pruning strategy preserving system/tool coherence.
   - optional `/compact` command for summarizing older turns.
2. Promote token counters to `u64` with saturating updates.
3. Replace session ID generator with CSPRNG bytes (`getrandom`).
4. Ensure SSH control connection cleanup on normal exit (`Drop`/shutdown hook).

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

1. `feat(agent): enforce context budget with pruning and compact command`
2. `fix(core): widen token counters and secure session id generation`
3. `fix(exec): clean up ssh control master on normal shutdown`

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

1. Expose true incremental streaming render for responses/completions where supported.
2. Implement the runtime/event refactor track in `docs/plans/2026-02-27-streaming-runtime-architecture.md` so streaming is a first-class library interface (not just CLI rendering).
3. Improve context warnings with actionable next steps (`/compact`, `/session new`).
4. Add protocol-switch warning on `/model` when API/auth mode changes.
5. Add pre-flight model/profile validation to reduce cryptic API errors.
6. Persist input history to `~/.config/buddy/history`.
7. Upgrade DuckDuckGo parser:
   - short-term diagnostics for empty parse;
   - medium-term HTML parser crate migration.

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

1. `feat(runtime): introduce streaming command/event interface for model/tool/metrics flows`
2. `feat(ui): stream assistant output incrementally for supported apis`
3. `feat(repl): improve context/model-switch guidance and preflight errors`
4. `feat(repl): persist command history across sessions`
5. `fix(search): improve parser resilience and diagnostics`

## Milestone 7: Deferred Design Extensions (P4)

### Scope

Address lower-priority architecture items after stabilization (`D1`, `D2`, `D3`, remaining `D4`, `T4`, `C3`).

### Tasks

1. Introduce `ToolContext` API and migrate tools incrementally.
2. Begin `ExecutionContext` backend trait extraction to reduce duplication.
3. Add deprecation warnings + timeline for `AGENT_*`, `agent.toml`, `.agentx`, legacy auth keys.
4. Add parser fuzz/property tests (`proptest`/`cargo-fuzz` targets).
5. Evaluate script-based tool plugin MVP only after above completes.

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

1. `refactor(tools): add tool context interface and migrate core tools`
2. `refactor(exec): extract backend trait and reduce execution duplication`
3. `chore(compat): add deprecation warnings and migration timeline`
4. `test(fuzz): add parser property/fuzz targets`
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
