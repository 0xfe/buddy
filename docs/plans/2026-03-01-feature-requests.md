# Feature Requests Delivery Plan (2026-03-01)

## Status

- Program status: Active
- Scope status: Locked to the confirmed feature requests plus a required UI-test prerequisite and first-class tmux-management feature.
- Current focus: Milestones 1-2 complete; Milestone 3 (theme library) next.
- Completed so far:
  1. Corrected scope to the confirmed feature-request list.
  2. Added a hard prerequisite milestone for tmux-based UI integration/regression testing before terminal work.
  3. Structured milestones with acceptance gates, explicit tests, docs, and commit slices.
  4. Closed Milestone 0 with module-boundary freeze, config-schema decisions, docs pointers, and baseline validation (`cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`) in commit `f218ec2`.
  5. Closed Milestone 1 with an opt-in tmux/asciinema UI harness, deterministic fake-model integration scenario, artifactized failure reporting, docs, and make targets in commit `892ca5d`.
  6. Implemented Milestone 2 with first-class tmux lifecycle tools, ownership boundaries, tmux target selectors on existing tools, session/pane limits, target-aware default-pane snapshot logic, and extended opt-in UI regression coverage in commit `78415e4`.
- Next steps:
  1. Start Milestone 3 (theme library, tokenized palette migration, `/theme`, explorer).
  2. Keep the UI harness suite as a pre-merge gate for terminal-facing rendering changes.
  3. Continue with Milestone 4 (build metadata, Makefile-first release workflow) after M3 acceptance.

## Scope (Locked)

Primary requested features:

1. Theme system:
   - default dark and light themes,
   - small composable semantic palette,
   - all buddy colors sourced from palette tokens only,
   - theme explorer,
   - `/theme` selector.
2. Build/version metadata and release workflow:
   - embed version/build date/commit hash in binary,
   - show metadata on startup,
   - make Makefile first-class (`build`, `test`, `release`, version bump),
   - GitHub Actions release builds on release tags.
3. `buddy init` improvements:
   - interactive TUI Q/A flow (model selection, login guidance, overwrite prompts),
   - update existing config,
   - auto-run on first `buddy` command when config is missing.
4. Packaging/distribution:
   - curl-style installer,
   - broad macOS/Linux support,
   - install to `~/.local/bin`,
   - hand off to `buddy init`.
5. Login UX:
   - `auth=login` missing credentials should not exit buddy,
   - show clear instruction to run `/login`.
6. First-class tmux management:
   - model-accessible tmux tools for create/kill session, create/kill pane, send keys, and capture pane,
   - buddy may manage only sessions/panes it created,
   - all managed tmux entities must use buddy/name-prefixed naming,
   - shell/capture-pane/send-keys accept optional tmux session/pane selectors (defaulting to shared pane),
   - clear logging/approval UX for tmux session/pane management operations,
   - system prompt must clearly mark default-pane snapshot and avoid injecting it when operating against non-default capture target,
   - configurable `max_sessions` and `max_panes` limits (defaults: `1` session, `5` panes; including default shared resources).

Required prerequisite before terminal feature work:

7. Robust tmux UI integration/regression harness:
   - launches buddy in isolated tmux session/pane with mock/fake models,
   - validates UI flow/colors/dynamic elements,
   - uses `capture-pane` and `pipe-pane`,
   - records asciinema sessions and preserves failure artifacts,
   - opt-in only (does not run under default `cargo test`).

## Plan Maintenance Rules

1. Keep this file in `docs/plans/` while active; move to `docs/plans/completed/` when done.
2. Update `## Status`, milestone checkboxes, and `## Execution Log` during execution.
3. Commit between tasks/milestones; record commit IDs in milestone notes and log.
4. Do not mark tasks complete until code + tests + docs are complete.
5. If scope changes, add a log entry before implementing.

## Integrated Program Board

- [x] Milestone 0: Design Freeze + Baseline Validation
- [x] Milestone 1: Tmux UI Integration/Regression Harness (Prerequisite)
- [x] Milestone 2: First-Class Tmux Management + Targeted Tmux Tooling
- [ ] Milestone 3: Theme Library + `/theme` + Theme Explorer
- [ ] Milestone 4: Build Metadata + Makefile-First + Release CI
- [ ] Milestone 5: Interactive `buddy init` + First-Run Bootstrap
- [ ] Milestone 6: Packaging + Curl Installer + Init Handoff
- [ ] Milestone 7: `auth=login` Soft-Fail UX
- [ ] Milestone 8: Final Integration, Regression, and Docs Closure

## Goal

Ship the confirmed feature requests in incremental, testable slices with UI reliability protected by a tmux-based regression harness before terminal-facing changes, and with first-class safe tmux management as a foundational capability.

## Milestone 0: Design Freeze + Baseline Validation

### Scope

Finalize architecture and test strategy for all scoped features, including the tmux harness contract.

### Tasks

1. [x] Lock module boundaries for:
   - theme tokens/theme registry/theme rendering hooks,
   - UI harness runner and artifact writer,
   - init flow state machine,
   - installer/package pipeline,
   - startup metadata display.
2. [x] Define config schema changes (theme selection and any harness toggles needed for deterministic tests).
3. [x] Capture baseline behavior with reproducible smoke commands.
4. [x] Add milestone implementation checklist to this plan.

### Acceptance Gate

1. Architecture decisions documented and stable.
2. Baseline tests pass before implementation.
3. Clear mapping from milestones to files/modules.

### Tests

1. `cargo test`
2. `cargo fmt --check`
3. `cargo clippy --all-targets -- -D warnings`

### Docs

1. `DESIGN.md` roadmap/features references (high-level).
2. Add pointer to UI regression approach in docs index.

### Commit

1. `docs(plan): freeze feature-request scope and baseline gates`
2. Completed as `f218ec2`.

### Design Freeze Decisions (Milestone 0)

1. Theme system boundary:
   - new module family under `src/ui/theme/` with:
   - semantic token definitions,
   - theme registry (dark/light/custom),
   - renderer adapters that map tokens to terminal styles.
   - rule: renderer code must consume semantic theme tokens only.
2. Tmux UI harness boundary:
   - new opt-in integration test harness under `tests/ui_tmux/` plus a test entrypoint (`tests/ui_tmux_regression.rs`).
   - helper utilities for tmux session lifecycle, pane IO capture, and artifact emission.
   - no default `cargo test` wiring; explicit ignored/integration invocation only.
3. Init flow boundary:
   - init orchestration and question state machine extracted under `src/init/` (prompt model, transitions, write/apply handlers).
   - CLI wiring in app entry remains thin adapter.
4. Packaging/distribution boundary:
   - distribution assets under `scripts/install/` and/or `dist/`.
   - build metadata + release artifact wiring remains in Makefile + CI workflow files.
5. Startup metadata boundary:
   - centralized version/build metadata provider in a dedicated module (`src/version.rs` or equivalent),
   - all startup/help/version surfaces consume the same metadata source.

### Planned Config Schema Delta (Milestone 0)

1. Add top-level display theme selection:
   - `display.theme = "dark"` (default), `"light"`, or custom theme name.
2. Add optional custom theme overrides:
   - `[themes.<name>]` semantic token keys to terminal style values.
3. Reserve test-only deterministic hooks for UI harness:
   - environment-driven test knobs preferred over persistent config where possible.

### Baseline Validation Snapshot (Milestone 0, 2026-03-01)

1. `cargo test`: PASS
   - lib tests: 279 passed, 0 failed
   - bin tests: 39 passed, 0 failed
   - integration/doc: 1 ignored regression test, 1 doc test passed
2. `cargo fmt --check`: PASS
3. `cargo clippy --all-targets -- -D warnings`: PASS

### Milestone 0 Implementation Checklist

1. [x] Module boundaries documented in this plan.
2. [x] Config schema deltas documented in this plan.
3. [x] Baseline validation commands executed and results recorded.
4. [x] Docs index points to UI regression approach stub (`docs/testing-ui.md`).
5. [x] `DESIGN.md` includes near-term feature-request roadmap reference.

## Milestone 1: Tmux UI Integration/Regression Harness (Prerequisite)

### Scope

Build an on-demand, high-signal UI test system that drives buddy inside tmux and validates terminal behavior before terminal feature changes.

### Tasks

1. [x] Build harness runner that:
   - creates isolated tmux session + pane,
   - starts buddy in the pane,
   - uses mock/fake model backend for deterministic outputs,
   - feeds scripted REPL input.
2. [x] Capture/observe terminal output with:
   - `tmux capture-pane` snapshots at defined checkpoints,
   - `tmux pipe-pane` live logs for full stream capture.
3. [x] Record asciinema cast per scenario and store artifact paths.
4. [x] Create assertion/report layer:
   - expected vs actual checks for key UI blocks,
   - clear failure report with offending section,
   - preserve full artifacts on failure.
5. [x] Add coverage scenarios for current UI interactions and dynamic elements:
   - startup banner + prompt line,
   - spinner lifecycle while task runs,
   - approval prompt formatting and response flow,
   - task output block rendering,
   - command completion/final status.
6. [x] Keep suite opt-in only:
   - not part of default `cargo test`,
   - explicit command path (similar to model-regression suites).

### Acceptance Gate

1. Harness can run end-to-end on local dev machine in isolated tmux session.
2. Every scenario emits deterministic pass/fail with actionable diffs.
3. Failure artifacts include capture snapshots, pipe logs, and asciinema recording.
4. Suite is opt-in and documented.

### Tests

1. `cargo test`
2. `cargo test --test ui_tmux_regression -- --ignored --nocapture` (target command; exact name may vary during implementation)
3. Optional make target for suite (for example `make test-ui-regression`).

### Docs

1. New `docs/testing-ui.md`:
   - harness architecture,
   - running tests,
   - artifact layout,
   - interpreting failures.
2. `README.md` test section: mention opt-in UI regression command.

### Commits

1. `test(ui): add tmux harness runner with capture-pane and pipe-pane collection`
2. `test(ui): add asciinema artifact recording and failure-reporting`
3. `test(ui): add opt-in regression scenarios for spinner prompt approval and output`
4. Completed as `892ca5d`.

### Milestone 1 Validation Snapshot (2026-03-01)

1. Opt-in UI suite:
   - `cargo test --test ui_tmux_regression -- --ignored --nocapture`: PASS
2. Local quality gates after harness integration:
   - `cargo test`: PASS (includes ignored suite registration)
   - `cargo fmt --check`: PASS
   - `cargo clippy --all-targets -- -D warnings`: PASS
3. Makefile wrappers now include:
   - `make test-ui-regression`
   - `make test-model-regression`

## Milestone 2: First-Class Tmux Management + Targeted Tmux Tooling

### Scope

Add first-class tmux management tools and tmux-target routing with strict ownership/safety constraints, while preserving the shared-pane default workflow.

### Tasks

1. Add dedicated tmux management tools for:
   - create session,
   - kill session,
   - create pane,
   - kill pane,
   - send keys (tmux-target aware),
   - capture pane (tmux-target aware).
2. Implement ownership boundaries:
   - buddy can only manage sessions and panes it created,
   - maintain an internal registry/metadata store of managed tmux entities,
   - reject unmanaged targets with explicit error messaging.
3. Enforce naming constraints:
   - all managed sessions and panes must use the buddy/name prefix system,
   - shared default pane naming remains deterministic and discoverable.
4. Extend existing tools (`run_shell`, `capture_pane`, `send_keys`) with optional `session`/`pane` target parameters:
   - default target remains the shared pane when unset,
   - explicit target resolution and validation for all execution paths.
5. Add configurable limits:
   - `max_sessions` and `max_panes` in config,
   - defaults `1` session and `5` panes,
   - limits include default shared session/pane.
6. Add approvals/logging for tmux management operations:
   - clear target + action details in approval prompts,
   - mutation/risk context included for create/kill operations,
   - structured logs for auditability.
7. System prompt snapshot behavior:
   - when default shared pane is the active execution target, inject snapshot and label it clearly as default shared pane state,
   - when command routing/capture targets a non-default pane, do not inject the default-pane snapshot for that request.
8. Extend opt-in UI harness scenarios (post-M1 enhancement):
   - managed session/pane create/kill flows,
   - target-specific shell/send-keys/capture behavior,
   - limit enforcement and unmanaged-target rejection.

### Acceptance Gate

1. Model can manage tmux sessions/panes through dedicated tools with explicit approvals.
2. No tmux operation can mutate unmanaged (non-buddy-created) sessions/panes.
3. Target-aware `run_shell`/`capture_pane`/`send_keys` work with explicit session/pane and correct shared-pane default behavior.
4. `max_sessions`/`max_panes` are enforced with clear errors.
5. System prompt snapshot logic is default-pane explicit and target-sensitive.
6. New tmux scenarios pass in opt-in UI regression suite.

### Tests

1. Unit tests:
   - tmux ownership registry and name validation,
   - target resolution/defaulting,
   - session/pane limit enforcement.
2. Integration tests:
   - tmux tool execution against managed/unmanaged targets,
   - system prompt snapshot injection/omission rules by target.
3. Opt-in UI regression updates:
   - multi-pane/session flows using `tests/ui_tmux_regression.rs` ignored scenarios.

### Docs

1. `DESIGN.md` features + tmux-ownership constraints.
2. `docs/testing-ui.md` scenario catalog update for tmux management coverage.
3. `docs/terminal-repl.md` tmux targeting semantics and snapshot rules.

### Commits

1. `feat(tmux): add managed session and pane lifecycle tools with ownership boundaries`
2. `feat(tools): add optional tmux target parameters for shell send-keys and capture-pane`
3. `test(ui): add tmux-management regression scenarios and target-routing assertions`
4. Status: complete (commit id recorded in execution log).

### Milestone 2 Validation Snapshot (2026-03-01)

1. `cargo fmt --check`: PASS
2. `cargo test`: PASS
3. `cargo clippy --all-targets -- -D warnings`: PASS
4. `make test-ui-regression`: PASS (2 scenarios, 0 failures)

## Milestone 3: Theme Library + `/theme` + Theme Explorer

### Scope

Implement composable semantic theming after harness baseline is in place.

### Tasks

1. Define semantic color tokens (for example `error_bg`, `error_fg`, `code_bg`, `status_dim`, `prompt_fg`).
2. Research terminal palette options (Solarized dark/light baseline) and map to tokens.
3. Implement theme library:
   - dark default,
   - light theme,
   - user custom overrides.
4. Migrate all renderer color usage to token-based theme lookups only.
5. Add `/theme` interactive selector and persistence.
6. Build theme explorer that previews representative REPL blocks and allows theme switching.

### Acceptance Gate

1. No direct hardcoded color escapes remain outside the theme module.
2. `/theme` switches/persists theme correctly.
3. Explorer preview aligns with actual REPL rendering.
4. UI regression suite covers theme-sensitive blocks.

### Tests

1. Theme library unit tests (lookup/fallback/custom override).
2. Renderer snapshot tests for dark/light themes.
3. UI regression scenarios for themed prompt/approval/code/output blocks.

### Docs

1. `README.md` theme usage and `/theme`.
2. `docs/terminal-repl.md` semantic token model and explorer usage.
3. `DESIGN.md` feature list update.

### Commits

1. `feat(ui): add semantic theme library with dark and light defaults`
2. `feat(repl): add /theme selector and persisted theme selection`
3. `feat(ui): add theme explorer with sample repl blocks`

## Milestone 4: Build Metadata + Makefile-First + Release CI

### Scope

Embed build metadata in the binary and standardize make targets + tagged release automation.

### Tasks

1. Embed:
   - app version,
   - commit hash,
   - build date/time.
2. Show metadata at startup and in version/help output.
3. Promote Makefile to primary entrypoint:
   - build/test/check/lint/format,
   - release packaging,
   - version bump helpers.
4. Add GitHub Actions release workflow for release tags.

### Acceptance Gate

1. Startup metadata is present and accurate.
2. Make targets cover daily dev and release flow.
3. Tag-triggered CI builds release artifacts.

### Tests

1. `make test`
2. `make check` (or equivalent aggregate target)
3. CI workflow validation for tag/non-tag paths.

### Docs

1. `README.md` build/version/release sections.
2. New or updated `docs/ci-release.md`.

### Commits

1. `feat(build): embed version commit and build-date metadata`
2. `build(make): make makefile the primary build and release interface`
3. `ci(release): add release-tag artifact workflow`

## Milestone 5: Interactive `buddy init` + First-Run Bootstrap

### Scope

Upgrade init into an interactive TUI flow for setup and config updates.

### Tasks

1. Implement guided init Q/A:
   - model selection,
   - login guidance,
   - overwrite/update confirmations.
2. Add existing config update mode (load current values and edit safely).
3. Ensure overwrite flow includes clear prompts and backup semantics.
4. Trigger init automatically on first `buddy` run when config missing.

### Acceptance Gate

1. New users can onboard without manual config editing.
2. Existing users can safely update configuration.
3. First-run auto-init behavior is clear and consistent.

### Tests

1. Init state-machine unit tests.
2. Integration tests for first-run and update paths.
3. UI regression scenarios for init prompt rendering.

### Docs

1. `README.md` quickstart/init guidance.
2. `docs/configuration.md` init/update behavior.

### Commits

1. `feat(init): add interactive tui onboarding flow`
2. `feat(init): support existing config update and overwrite prompts`
3. `feat(cli): auto-run init when config is missing`

## Milestone 6: Packaging + Curl Installer + Init Handoff

### Scope

Add cross-platform packaging/distribution and installer flow to `~/.local/bin`.

### Tasks

1. Define artifact packaging for macOS/Linux.
2. Add curl-style installer script:
   - install to `~/.local/bin`,
   - validate platform/arch,
   - set executable permissions.
3. Post-install handoff:
   - run `buddy init` or trigger first-run init path.
4. Idempotent reinstall behavior with clear errors.

### Acceptance Gate

1. Installer works across supported macOS/Linux targets.
2. Installed binary is runnable from `~/.local/bin`.
3. Post-install init handoff works reliably.

### Tests

1. Installer smoke tests in CI containers/VMs.
2. Reinstall/idempotency tests.

### Docs

1. `README.md` install section.
2. New `docs/install.md` for installer behavior/troubleshooting.

### Commits

1. `feat(dist): add packaging and curl installer for macos and linux`
2. `feat(installer): add post-install init handoff and idempotent behavior`

## Milestone 7: `auth=login` Soft-Fail UX

### Scope

Do not abort startup when login credentials are missing; provide clear recovery guidance.

### Tasks

1. Replace hard-fail startup path for missing login credentials with non-fatal warning.
2. Show explicit recovery guidance:
   - `/login` in REPL,
   - `buddy login <model>` in CLI mode where relevant.
3. Keep authenticated behavior unchanged.

### Acceptance Gate

1. Missing login no longer exits buddy.
2. Guidance is explicit and actionable.
3. Existing login-authenticated path remains stable.

### Tests

1. Startup behavior tests for missing login creds.
2. Regression tests for authenticated profiles.
3. UI regression scenario for warning message formatting.

### Docs

1. `README.md` auth troubleshooting section.
2. `DESIGN.md` auth behavior update.

### Commit

1. `fix(auth): soft-fail missing login credentials with /login guidance`

## Milestone 8: Final Integration, Regression, and Docs Closure

### Scope

Run final validation across all milestones and close documentation/workflow updates.

### Tasks

1. Execute full quality gates.
2. Run opt-in UI regression suite and model regression suite.
3. Verify release workflow and make targets after integration.
4. Update docs for final consistency.
5. Move plan to `docs/plans/completed/` once done.

### Acceptance Gate

1. Lint/tests pass.
2. Opt-in UI suite passes with clean artifact reporting.
3. Docs and commands reflect shipped behavior.
4. Plan includes completion log with commit IDs.

### Tests

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. UI regression suite (opt-in).
5. Model regression suite (opt-in).

### Docs

1. Full docs consistency pass (excluding archived plans).

### Commit

1. `docs: finalize feature-request delivery and archive plan`

## Execution Log

- 2026-03-01: Initial feature-request plan created.
- 2026-03-01: Re-scoped to exact confirmed requests.
- 2026-03-01: Added Milestone 1 as a hard prerequisite for tmux/asciinema UI integration regression before terminal/UI feature work.
- 2026-03-01: Milestone 0 completed. Captured module boundaries, planned config schema deltas, and baseline validation results (`cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`), then updated docs pointers for the upcoming UI harness. Commit: `f218ec2`.
- 2026-03-01: Milestone 1 implementation landed: added `tests/ui_tmux/` harness helpers, `tests/ui_tmux_regression.rs` ignored scenario, fake-model server, tmux `capture-pane`/`pipe-pane` checkpoints, asciinema recording, structured `report.json`, docs (`docs/testing-ui.md`), README integration notes, and Makefile test targets. Validation: opt-in suite PASS + full local gates PASS. Commit: `892ca5d`.
- 2026-03-01: Scope expanded to include first-class tmux management (managed session/pane lifecycle, tmux-targeted tool routing, ownership boundaries, and session/pane limits). Inserted new Milestone 2 and shifted prior Milestones 2-7 to 3-8.
- 2026-03-01: Milestone 2 implementation complete: added managed tmux lifecycle tools (`tmux-create-session`, `tmux-kill-session`, `tmux-create-pane`, `tmux-kill-pane`), optional `session`/`pane` selectors for `run_shell`/`capture-pane`/`send-keys`, managed ownership markers and canonical naming, `[tmux]` limits (`max_sessions`, `max_panes`), target-aware default-pane snapshot injection rules, and extended ignored UI regression coverage for targeted managed-pane execution. Validation: `cargo fmt --check` PASS, `cargo test` PASS, `cargo clippy --all-targets -- -D warnings` PASS, `make test-ui-regression` PASS. Commit: `78415e4`.
