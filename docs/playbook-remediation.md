# Remediation Playbook (2026-02-27)

## Purpose

This playbook is the baseline repro/validation harness for the remediation
plan in `docs/plans/2026-02-27-claude-feedback-remediation-plan.md`.

It focuses on:

1. reproducible local checks for high-priority issues (`S*`, `B*`, `R*`),
2. deterministic offline test commands,
3. optional network regression checks.

## Quick Baseline

Run this first before and after each milestone slice:

```bash
cargo fmt --all
cargo test
```

## Repro Matrix

| ID | Area | Repro Command / Flow | Expected Current Behavior (before fix) |
| --- | --- | --- | --- |
| `S1` | shell guardrails | `cargo test tools::shell::tests::execute_confirm_approved_via_broker_runs_command -- --nocapture` | Approval flow works, but no denylist/sandbox policy yet. |
| `S2` | fetch SSRF | Manual REPL with `fetch_url` against `http://127.0.0.1:<port>` | Request is blocked by default unless explicitly allowed in `tools.fetch_allowed_domains`. |
| `S3` | file write path policy | `cargo test tools::files::tests::write_file_creates_and_reports_bytes -- --nocapture` | Write works with no allowlist/denylist path checks yet. |
| `B1` | UTF-8 truncation | Add/execute non-ASCII truncation case in `tools::shell` / `tools::files` | Some truncation code still slices by byte index and can panic. |
| `R1` | HTTP timeout | `cargo test api::client::tests::api_client_respects_timeout_policy -- --nocapture` and `cargo test tools::fetch::tests::fetch_tool_respects_configured_timeout -- --nocapture` | Requests time out predictably using configured `[network]` timeout policy. |
| `R2` | history growth | Long REPL session (many tool turns) | History grows without hard-budget enforcement/pruning. |
| `B5` | SSE parser robustness | `cargo test api::responses::tests::parse_streaming_responses_payload_extracts_completed_response -- --nocapture` | Basic streaming parse works; event-block compliance still needs hardening. |

## Detailed Repro Notes

### Shell approval behavior (`S1`)

Deterministic tests:

```bash
cargo test tools::shell::tests::execute_confirm_approved_via_broker_runs_command -- --nocapture
cargo test tools::shell::tests::execute_confirm_denied_via_broker_skips_command -- --nocapture
```

Manual interactive check:

1. Start REPL: `cargo run`
2. Ask for a shell action requiring approval.
3. Confirm prompt rendering and decision handling.

### Fetch SSRF behavior (`S2`)

Manual local probe:

1. Start a local server: `python3 -m http.server 8787`
2. In buddy, trigger `fetch_url` for `http://127.0.0.1:8787`.
3. Expected now: request is blocked by default. To allow intentionally, add host/domain to `tools.fetch_allowed_domains`.

### HTTP timeout behavior (`R1`)

Deterministic checks:

```bash
cargo test api::client::tests::api_client_respects_timeout_policy -- --nocapture
cargo test tools::fetch::tests::fetch_tool_respects_configured_timeout -- --nocapture
```

Configuration knobs:

```toml
[network]
api_timeout_secs = 120
fetch_timeout_secs = 20
```

### File path policy behavior (`S3`)

Deterministic baseline:

```bash
cargo test tools::files::tests::write_file_creates_and_reports_bytes -- --nocapture
```

Current baseline: write succeeds for arbitrary writable paths.

### SSE edge payload behavior (`B5`)

Current parser checks:

```bash
cargo test api::responses::tests::parse_streaming_responses_payload_extracts_completed_response -- --nocapture
cargo test api::responses::tests::parse_streaming_responses_payload_captures_reasoning_deltas -- --nocapture
cargo test api::responses::tests::parse_streaming_responses_payload_captures_reasoning_items -- --nocapture
```

These are baseline checks before switching to a stricter event-block parser.

## Offline Test Suites

Required for all remediation milestones:

```bash
cargo test
```

Focused module runs:

```bash
cargo test tools::shell::tests -- --nocapture
cargo test tools::files::tests -- --nocapture
cargo test api::responses::tests -- --nocapture
cargo test auth::tests -- --nocapture
```

## Ignored / Network Regression Suite

Run only when credentials/network are available:

```bash
cargo test --test model_regression -- --ignored --nocapture
```

Guidance:

1. Keep prompts minimal to limit token spend.
2. Run after API/auth/profile changes and before release.
3. Treat failures as provider/API drift until proven local regression.
