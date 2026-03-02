# Model Regression Tests

## Purpose

`tests/model_regression.rs` is a live network regression suite for provider/model compatibility checks.

It verifies that the default model profiles shipped in `src/templates/buddy.toml` can each complete:

1. a tiny plain-text round-trip, and
2. a protocol-valid follow-up turn that includes prior assistant tool-calls plus a tool-error result.

This suite is intentionally separate from normal unit tests because it requires network access and real credentials.

## What It Checks

For each profile in the default template:

1. Select the profile via normal runtime config resolution.
2. Validate auth prerequisites:
   - `auth = "api-key"`: resolved key must be non-empty.
   - `auth = "login"`: saved provider login tokens must already exist.
   - If a profile does not set `api_key_env`, the suite falls back to provider
     defaults: `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `MOONSHOT_API_KEY`,
     `ANTHROPIC_API_KEY`.
3. Send a minimal prompt (`Reply with exactly OK.`) with no tools.
4. Assert:
   - response has at least one choice,
   - no unexpected tool calls are returned,
   - assistant text content is non-empty.
5. Send a second probe with injected tool history:
   - assistant tool-call message,
   - matching tool-result message containing an error payload,
   - user follow-up requesting a plain text reply.
6. Assert:
   - provider accepts the history shape,
   - assistant text content is still non-empty (error recovery path remains viable).

Profile execution is independent:

- missing credentials remain failures for that profile,
- `model_not_found` responses are reported as skipped (profile unavailable on the current account/API),
- one profile failure does not stop probes for other profiles.
- auth-mode expectations are read from `src/templates/models.toml` capability metadata
  (for example login-only models are probed with login auth even when profile auth defaults to API key).

## Why It Is Ignored By Default

The test is marked `#[ignore]` so `cargo test` remains offline, fast, and deterministic.

## How To Run

```bash
cargo test --test model_regression -- --ignored --nocapture
```

## Required Setup

1. Ensure template-referenced API key env vars are available for API-key profiles.
   - Example defaults: `OPENROUTER_API_KEY`, `MOONSHOT_API_KEY`, `ANTHROPIC_API_KEY`.
2. Ensure login profiles are already authenticated (for default OpenAI login profiles):

```bash
buddy login openai
```

3. Unset global runtime override env vars before running this suite:
   - `BUDDY_API_KEY`, `AGENT_API_KEY`
   - `BUDDY_BASE_URL`, `AGENT_BASE_URL`
   - `BUDDY_MODEL`, `AGENT_MODEL`

These overrides intentionally fail the suite because they mask per-profile behavior.

## Cost/Token Discipline

The suite uses two short requests per profile and keeps prompts minimal.
The tool-error-history probe uses synthetic tool history in the message list (it does not actually execute shell commands).

## Failure Triage

Common failure classes:

- auth preflight failed (missing env key or missing login tokens)
- provider timeout / transport error
- provider accepted request but returned empty assistant content
- unexpected tool-call response for a no-tools probe
- provider rejected prior tool-call/tool-result history shape

When a failure appears, rerun the single suite command with `--nocapture` to see per-profile logs.
