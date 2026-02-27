# Model Regression Tests

## Purpose

`tests/model_regression.rs` is a live network regression suite for provider/model compatibility checks.

It verifies that the default model profiles shipped in `src/templates/buddy.toml` can each complete a tiny prompt round-trip using the current auth and protocol logic.

This suite is intentionally separate from normal unit tests because it requires network access and real credentials.

## What It Checks

For each profile in the default template:

1. Select the profile via normal runtime config resolution.
2. Validate auth prerequisites:
   - `auth = "api-key"`: resolved key must be non-empty.
   - `auth = "login"`: saved provider login tokens must already exist.
3. Send a minimal prompt (`Reply with exactly OK.`) with no tools.
4. Assert:
   - response has at least one choice,
   - no unexpected tool calls are returned,
   - assistant text content is non-empty.

## Why It Is Ignored By Default

The test is marked `#[ignore]` so `cargo test` remains offline, fast, and deterministic.

## How To Run

```bash
cargo test --test model_regression -- --ignored --nocapture
```

## Required Setup

1. Ensure template-referenced API key env vars are available for API-key profiles.
   - Example defaults: `OPENROUTER_API_KEY`, `MOONSHOT_API_KEY`.
2. Ensure login profiles are already authenticated (for default OpenAI login profiles):

```bash
buddy login gpt-codex
```

3. Unset global runtime override env vars before running this suite:
   - `BUDDY_API_KEY`, `AGENT_API_KEY`
   - `BUDDY_BASE_URL`, `AGENT_BASE_URL`
   - `BUDDY_MODEL`, `AGENT_MODEL`

These overrides intentionally fail the suite because they mask per-profile behavior.

## Cost/Token Discipline

The suite uses one very short request per profile and avoids tools to keep token usage and runtime cost low.

## Failure Triage

Common failure classes:

- auth preflight failed (missing env key or missing login tokens)
- provider timeout / transport error
- provider accepted request but returned empty assistant content
- unexpected tool-call response for a no-tools probe

When a failure appears, rerun the single suite command with `--nocapture` to see per-profile logs.
