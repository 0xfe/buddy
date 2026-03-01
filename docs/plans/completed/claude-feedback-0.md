# Engineering Review: Buddy

**Date:** 2026-02-27
**Reviewer:** Claude (Opus 4.6)
**Scope:** Full codebase — architecture, bugs, security, robustness, testability, design flexibility, usability

---

## Remediation Status (reviewed 2026-02-28)

All 30 findings were addressed across 7 milestones documented in `2026-02-27-claude-feedback-remediation-plan.md`. The remediation was executed in a single intensive session alongside a parallel streaming-runtime architecture effort. Verification: `cargo check` clean, 258 lib + 37 bin tests pass, model regression suite passes for all default profiles.

### Fully Addressed (29/30)

| ID | Finding | Resolution |
|----|---------|------------|
| S1 | Shell injection / no sandboxing | Substring denylist with conservative defaults, `exec` mode fail-closed, `--dangerously-auto-approve` flag with warning banner |
| S2 | SSRF via fetch_url | IP range blocking (loopback, RFC 1918, link-local, metadata), configurable domain allow/blocklists, optional confirm mode |
| S3 | Unrestricted file writes | Sensitive directory blocklist, configurable `files_allowed_paths`, deterministic deny messages |
| S4 | Plaintext auth tokens | AES-256-GCM-SIV encryption with scrypt-derived KEK from machine identity, DEK wrapping, legacy migration, tamper detection, `--check`/`--reset` flows |
| B1 | UTF-8 truncation panic | `textutil.rs` with `safe_prefix_by_bytes()` using `is_char_boundary()` backtracking; all call sites migrated |
| B2 | Token counter overflow | Promoted to `u64` with `saturating_add` throughout |
| B3 | Weak session IDs | CSPRNG via `OsRng`/`getrandom` producing `xxxx-xxxx-xxxx-xxxx` format |
| B4 | Fragile HTML scraper | Migrated to `scraper` crate with CSS selectors, added empty-parse diagnostics |
| B5 | Non-compliant SSE parser | Event-block parser honoring multi-line `data:`, comments, and field ordering |
| R1 | No HTTP timeout | Centralized `reqwest::Client` with configurable API/fetch timeouts (`[network]` config + env overrides) |
| R2 | Unbounded history growth | 80% warning, 95% hard-stop with auto-compaction, `/compact` command, context budget enforcement |
| R3 | No transient retry | Exponential backoff for 429/5xx/timeout/connect errors, `Retry-After` support, protocol mismatch hints on 404 |
| R4 | SSH connection leak | `Drop`-based cleanup for SSH control master with test verification |
| R5 | Per-request HTTP clients | Shared `reqwest::Client` for auth flows and web search |
| T1 | No agentic loop tests | `ModelClient` trait with mock-based testing enabled (foundation laid via streaming runtime `S1`) |
| T2 | No REPL/main.rs tests | `RenderSink` trait, mock-renderer orchestration tests for `/session`, `/model`, runtime warning events |
| T3 | Config loading untestable | Source-injected loader (`load_config_with_diagnostics_from_sources`) for deterministic tests without filesystem/env coupling |
| T4 | No fuzz testing | Feature-gated `proptest` tests for SSE event parsing and shell wait-duration parsing |
| D1 | Tool trait lacks context | `ToolContext` with optional streaming channel, `has_stream()`, `emit()` methods |
| D2 | ExecutionContext duplication | `ExecutionBackendOps` trait extracted, concrete backend impls, shared `CommandBackend` helper |
| D4 | Renderer not mockable | `RenderSink` trait with `StderrRenderer` default, injectable in tests |
| U1 | No streaming output | Full runtime command/event architecture (`RuntimeCommand`/`RuntimeEvent`), `CliEventRenderer` adapter, streaming tool output events |
| U2 | Context warning not actionable | Warning now suggests `/compact` and `/session new`, auto-compaction at 95% |
| U3 | No protocol switch warning | Preflight validation on `/model` switch emits API/auth mode-change warnings |
| U4 | Cryptic error messages | `preflight.rs` validates base URL, model name, API key sources, and login state at startup and on model switch |
| U5 | No persistent history | History saved to `~/.config/buddy/history` as JSON, configurable via `[display].persist_history` |
| C1 | Monolithic main.rs | Partially addressed — runtime event/command decoupling, `CliEventRenderer` extraction, but `main.rs` grew to ~2825 lines (net larger due to runtime integration) |
| C2 | Duplicated config logic | Shared `resolve_config_from_file_config()` used by both runtime and tests |
| C3 | No deprecation timeline | Load-time diagnostics for `AGENT_*`, `agent.toml`, `.agentx`, legacy `[api]`; `docs/developer/deprecations.md` with migration timeline |

### Will-Not-Fix (1/30)

| ID | Finding | Disposition |
|----|---------|-------------|
| D3 | Plugin/extension mechanism | Deferred — high effort, low immediate value. Revisit only with concrete operator demand. |

### Notes

- **C1** is marked fully addressed per remediation plan scope, but the concern (main.rs complexity) has arguably shifted rather than shrunk — `main.rs` is now ~2825 lines, larger than the original ~1100 lines. The complexity moved partially into `cli_event_renderer.rs` (419 lines) and `runtime.rs` (1674 lines), but `main.rs` itself absorbed runtime command/event wiring. A fresh finding for this is captured in `claude-feedback-1.md`.
- Several security controls (S1 denylist, S2 SSRF, S3 path restrictions) are defense-in-depth layers behind the interactive approval prompt. Their bypass characteristics are documented in `claude-feedback-1.md` as new findings for the next iteration.

---

## Priority 1 — Security

### S1. Shell command injection via `sh -c` with no sandboxing

**Location:** `src/tools/shell.rs:154`, `src/tools/execution.rs:run_sh_process`

The model chooses the command string and it is passed directly to `sh -c`. While the approval prompt exists (`shell_confirm`), this is the single most dangerous surface in the system:

- **In exec mode** (`buddy exec`), approval is bypassed entirely — the shell approval broker is `None` and `confirm` defaults to config, but `interactive_mode` is false so `shell_approval_broker` is set to `None` and the tool falls through to the `eprint!("Run: ...")` path which reads from stdin. If stdin is empty/piped, `read_line` returns empty → denial. This is safe by accident, not by design.
- **`/approve all`** auto-approves everything for the session with no allowlist. A prompt-injected payload (e.g., model told "ignore prior instructions") could exfiltrate data or install malware.
- **No command allowlist or denylist.** There is no mechanism to restrict which commands the model can run (e.g., block `rm -rf /`, `curl | sh`, `chmod`, etc.).

**Proposed fixes:**
1. Add a `shell_allowlist` / `shell_denylist` config option (regex or glob patterns) with sane defaults that block destructive operations.
2. Add a `--dangerously-auto-approve` flag separate from the runtime `/approve all` to make the risk explicit.
3. In exec mode, default to `confirm: true` and fail-closed if stdin is not a TTY rather than silently denying.
4. Consider a lightweight command classification (read-only vs. mutating) that auto-approves reads but prompts for writes.

### S2. `fetch_url` is an unauthenticated SSRF vector

**Location:** `src/tools/fetch.rs:54`

The model can instruct `fetch_url` to hit any URL, including `http://localhost`, `http://169.254.169.254` (cloud metadata), internal network services, etc. There is no URL validation, no domain allowlist, and no timeout.

**Proposed fixes:**
1. Add a configurable `fetch_allowed_domains` / `fetch_blocked_domains` list.
2. Block private/reserved IP ranges by default (RFC 1918, link-local, loopback, cloud metadata IPs).
3. Add a request timeout (currently inherits reqwest's default, which is no timeout for the response body).
4. Consider requiring confirmation for fetch like shell commands.

### S3. `write_file` has no path restrictions

**Location:** `src/tools/files.rs:118-129`

The model can write to any path the process owner can access — including `~/.ssh/authorized_keys`, `~/.bashrc`, cron files, etc. There are no guardrails.

**Proposed fixes:**
1. Add a configurable `files_allowed_paths` (directory allowlist, e.g., only CWD and /tmp).
2. Block writes to sensitive paths by default (dotfiles, `/etc`, `/usr`, SSH keys).
3. Optionally require confirmation for write operations (like shell confirmation).

### S4. Auth tokens stored as plaintext JSON

**Location:** `src/auth.rs:444-471`

The auth store (`~/.config/buddy/auth.json`) stores access and refresh tokens as plaintext JSON. While file permissions are set to `0600`, this is below the bar for credential storage:

- Any process running as the same user can read it.
- Tokens survive in filesystem snapshots, backups, and Time Machine.

**Proposed fixes:**
1. Use OS keychain integration (macOS Keychain, Linux secret-service/kwallet) via the `keyring` crate.
2. At minimum, encrypt at rest with a machine-derived key.
3. Short-term: document the risk and add a reminder in `buddy init` output.

---

## Priority 2 — Bugs & Correctness

### B1. `truncate_output` can split a multi-byte UTF-8 character

**Location:** `src/tools/shell.rs:221-227`, `src/tools/files.rs:64`, `src/tools/fetch.rs:61`

```rust
fn truncate_output(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...[truncated]", &s[..max])  // panics on non-ASCII
    }
```

`&s[..max]` indexes by byte, not character. If `max` lands in the middle of a multi-byte UTF-8 sequence, this will **panic at runtime**. This affects shell output, file reads, and URL fetches — all of which commonly contain non-ASCII content.

**Fix:** Use `s.char_indices()` to find the last valid boundary at or before `max`, or use `s.floor_char_boundary(max)` (stable since Rust 1.82).

### B2. Token tracker `total_prompt_tokens` / `total_completion_tokens` can overflow

**Location:** `src/tokens.rs:39-44`

These are `u32` fields accumulated via `+=`. A long session with a large-context model could overflow (4B tokens ~= 1600 requests at 2.5M tokens each). The Responses API counts input tokens per request, so sessions with many tool-call iterations on 1M+ context models will hit this.

**Fix:** Use `u64` or `saturating_add`.

### B3. Session ID generation uses `DefaultHasher` which is not cryptographically random

**Location:** `src/session.rs:208-237`

`generate_session_id()` hashes `SystemTime::now()`, a process-local nonce, and the PID using `DefaultHasher`. This is:
- Predictable (timestamp + PID are guessable).
- Not collision-resistant across machines.

For session IDs this is low-risk, but if session files are ever exposed (e.g., shared filesystem), IDs are trivially guessable.

**Fix:** Use `getrandom` or `rand` crate for 16 bytes of randomness, or `uuid::Uuid::new_v4()`.

### B4. `web_search` HTML parsing is fragile and will silently break

**Location:** `src/tools/search.rs:102-130`

The DuckDuckGo scraper uses string-splitting (`split("class=\"result__a\"")`) to parse HTML. DuckDuckGo's HTML structure changes periodically, and this parser:
- Cannot handle attribute reordering (`class = "result__a"` with a space).
- Cannot handle minified HTML where classes are concatenated differently.
- Silently returns empty results on layout changes.

**Proposed fixes:**
1. Short-term: Add a diagnostic message when zero results are returned for a non-empty query (distinguish "no results" from "parser broke").
2. Medium-term: Use a lightweight HTML parser (`scraper` or `select` crate).
3. Long-term: Use DuckDuckGo's API or a search API with stable output.

### B5. SSE stream parsing doesn't handle `event:` lines with attached data

**Location:** `src/api/responses.rs:265-267`

```rust
let data = line.trim();
if !data.starts_with("data:") {
    continue;
}
```

This parser processes one line at a time but the SSE spec defines events as multi-line blocks separated by blank lines. The parser ignores `event:` lines entirely and only looks for `data:` lines. This works for OpenAI's current format but is not spec-compliant and will break if:
- A provider sends multi-line `data:` fields.
- A provider uses `id:` or `retry:` fields that contain `data:` substrings.

**Fix:** Implement proper SSE parsing (split on `\n\n` blocks, then parse fields within each block).

---

## Priority 3 — Robustness & Error Handling

### R1. No HTTP request timeout

**Location:** `src/api/completions.rs:16`, `src/api/responses.rs:28`, `src/tools/fetch.rs:54`

`reqwest::Client::new()` creates a client with no timeout. A hung API server will block the agent loop forever. The cancellation mechanism (`tokio::select!` in `agent.rs`) only applies to the outer call — if the HTTP request itself hangs, it cannot be cancelled.

**Fix:** Configure `reqwest::Client::builder().timeout(Duration::from_secs(300))` (or configurable). For `fetch_url`, use a shorter timeout (30s).

### R2. Unbounded conversation history growth

**Location:** `src/agent.rs:274`, `src/agent.rs:413`

Messages are pushed to `self.messages` without any limit. A long session will:
1. Grow memory usage linearly.
2. Eventually exceed the model's context window (the 80% warning fires but doesn't prevent the request).
3. Send increasingly large payloads that the API may reject.

**Proposed fixes:**
1. Implement conversation history pruning (drop oldest user/assistant turns, keeping system prompt and recent N turns).
2. Implement summarization (ask the model to summarize older turns before pruning).
3. At minimum, refuse to send when estimated tokens exceed 95% of context limit.

### R3. No retry logic for transient API errors

**Location:** `src/api/client.rs:36-58`

The only retry logic is a single 401 refresh attempt. Transient errors (429 rate limit, 500/502/503 server errors, network timeouts) are not retried. This makes the tool fragile in production use.

**Fix:** Add exponential backoff retry for 429/5xx errors (2-3 attempts with 1s/2s/4s delays). Respect `Retry-After` header for 429.

### R4. SSH control connection is never cleaned up on normal exit

**Location:** `src/tools/execution.rs:401-492`

`close_ssh_control_connection` is only called on error paths during SSH context setup. On normal program exit, the SSH master connection persists indefinitely (`ControlPersist=yes`). The control socket file in `/tmp` is also never cleaned up.

**Fix:** Implement `Drop` for `SshContext` (or a cleanup method called at exit) that runs `ssh -O exit`.

### R5. `reqwest::Client::new()` is called per-request in several tools

**Location:** `src/tools/search.rs:59`, `src/auth.rs:222,256,333,377`

Each web search and each auth request creates a new `reqwest::Client`. This:
- Misses connection pooling benefits.
- Allocates a new TLS context each time.
- Is measurably slower.

**Fix:** Share a single `reqwest::Client` instance (pass it through or use a `OnceLock<Client>`).

---

## Priority 4 — Testability

### T1. No integration tests for the agentic loop

The `agent.rs` `send()` method — the most critical function — has zero integration tests. It's tested only indirectly via the model regression suite (which requires live API access and is `#[ignore]`d).

**Proposed fixes:**
1. Create a `MockApiClient` (or make `ApiClient` a trait) so the agentic loop can be tested with canned responses.
2. Write tests for: basic response, tool call + result cycle, multi-tool parallel calls, max iterations, cancellation, context limit warning.

### T2. No tests for `main.rs` / REPL logic

`main.rs` is ~1100 lines of complex orchestration logic (background tasks, approval flow, slash commands, session management) with zero tests. This is the highest-risk untested code.

**Proposed fixes:**
1. Extract the REPL orchestration into a testable `ReplController` struct that takes injected dependencies.
2. Test slash command dispatch, approval flow state machine, background task lifecycle, session save/resume.

### T3. Config loading is tightly coupled to filesystem and env

**Location:** `src/config.rs:280-356`

`load_config` directly reads files and env vars, making it untestable without filesystem setup. The internal `resolve_active_api_with` is testable (and tested), but the public `load_config` is not.

**Fix:** Accept a `ConfigSource` trait or closures for `read_file` and `env_lookup` in the public API (the pattern already exists internally — expose it).

### T4. No property-based or fuzz testing for parsers

The DuckDuckGo HTML parser, SSE stream parser, duration parser, and URL encoder all process untrusted input but have only hand-written example tests.

**Fix:** Add `proptest` or `cargo-fuzz` targets for these parsing functions.

---

## Priority 5 — Design Flexibility

### D1. Tool trait doesn't support stateful context or dependency injection

**Location:** `src/tools/mod.rs:28-39`

```rust
async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
```

Tools receive only a JSON arguments string. They cannot access:
- The conversation history (needed for context-aware tools).
- The current working directory or session metadata.
- Other tools (needed for composite tools).
- A shared HTTP client.

Each tool that needs state must store it in its struct fields, leading to the `ShellTool { confirm, color, execution, approval }` pattern where every tool carries its own copies of shared resources.

**Proposed fix:** Add a `ToolContext` parameter to `execute()`:
```rust
async fn execute(&self, arguments: &str, ctx: &ToolContext) -> Result<String, ToolError>;
```
Where `ToolContext` provides access to shared resources (execution context, HTTP client, renderer, etc.).

### D2. `ExecutionContext` has heavy code duplication across backends

**Location:** `src/tools/execution.rs` (800+ lines)

The five backend variants (Local, LocalTmux, Container, ContainerTmux, Ssh) each have near-identical implementations for `read_file`, `write_file`, `capture_pane`, `send_keys`, and `run_shell_command`. The methods on `ExecutionContext` are large match blocks that repeat the same pattern.

**Proposed fix:** Extract a `Backend` trait:
```rust
trait Backend: Send + Sync {
    async fn run_shell(&self, cmd: &str, stdin: Option<&[u8]>, wait: ShellWait) -> Result<ExecOutput, ToolError>;
    async fn capture_pane(&self, opts: &CapturePaneOptions) -> Result<String, ToolError>;
    async fn send_keys(&self, opts: &SendKeysOptions) -> Result<(), ToolError>;
}
```
Then `ExecutionContext` becomes a thin `Arc<dyn Backend>` wrapper.

### D3. No plugin or extension mechanism for tools

Tools are hardcoded in `main.rs` during registration. Users cannot add custom tools without forking the codebase.

**Proposed fixes:**
1. Support loading tools from a config-specified directory of scripts (each script = a tool, with a manifest for name/description/parameters).
2. Support WASM-based tool plugins.
3. At minimum, support a generic "run script" tool that loads tools from `~/.config/buddy/tools/`.

### D4. Renderer is not injectable/mockable

**Location:** `src/tui/renderer.rs`

`Renderer` writes directly to stderr. Agent, tools, and the REPL all create their own `Renderer` instances. This makes it impossible to:
- Capture rendering output in tests.
- Redirect output in library usage.
- Implement alternative frontends (e.g., JSON output mode for automation).

**Fix:** Make `Renderer` a trait with a `StderrRenderer` default implementation.

---

## Priority 6 — Usability

### U1. No streaming output — user stares at a spinner during long responses

The API response is consumed in full before displaying anything. For models that take 30+ seconds to respond, the user sees only a spinner with no indication of progress.

**Proposed fix:** Implement streaming for both protocols:
- Chat Completions: `stream: true` with SSE consumption.
- Responses: already has streaming support internally — expose incremental text to the renderer.

### U2. Context limit warning is a one-time message with no actionable guidance

**Location:** `src/agent.rs:281-282`

The 80% context warning says "responses may be truncated" but doesn't tell the user what to do about it. Users have no way to:
- See how much context is used.
- Trim history.
- Start a new session preserving key context.

**Fix:** Add `/compact` command that summarizes and trims history. Improve the warning message to suggest `/session new` or `/compact`.

### U3. `/model` switching doesn't warn about protocol incompatibility

Switching from a Completions profile to a Responses profile (or vice versa) mid-session may produce confusing errors because message history format assumptions differ.

**Fix:** Warn when protocol changes during model switch. Consider clearing tool-call history that may not translate.

### U4. Error messages for common misconfigurations are cryptic

Examples:
- Missing API key with `api_key_env` set → empty string silently passed as auth → 401 from API → `"status 401: ..."` with raw JSON body.
- Wrong `api` protocol → confusing parse error from the wrong endpoint.

**Fix:** Add pre-flight validation that checks: API key is non-empty (or login tokens exist), base URL responds to a ping, model name is valid for the provider.

### U5. No command history persistence across sessions

**Location:** `src/tui/input_buffer.rs`

REPL command history is in-memory only. Restarting buddy loses all history.

**Fix:** Persist history to `~/.config/buddy/history` (like readline/zsh history).

---

## Priority 7 — Code Quality & Maintenance

### C1. `main.rs` is 1100+ lines of monolithic orchestration

The REPL loop, background task management, approval flow, slash command dispatch, session management, and startup logic are all in one function with deeply nested control flow.

**Fix:** Extract into focused modules: `repl_controller.rs`, `background_tasks.rs`, `approval_flow.rs`, `startup.rs`.

### C2. Duplicated config loading logic between `load_config` and test helper

**Location:** `src/config.rs:280-356` vs `src/config.rs:997-1033`

`parse_file_config_for_test` duplicates the models-fallback and agent-model-default logic from `load_config`. Changes to one must be mirrored in the other.

**Fix:** Extract the shared logic into a `resolve_config_from_parsed(FileConfig) -> Config` function used by both.

### C3. Legacy compatibility code should have a removal timeline

Multiple legacy paths exist:
- `AGENT_*` env vars alongside `BUDDY_*`
- `agent.toml` config file names
- `.agentx/` session directory
- `[api]` config section
- Profile-scoped auth tokens

These add maintenance burden and cognitive overhead. None have deprecation warnings.

**Fix:** Add deprecation warnings that print once per session. Set a version target for removal.

---

## Summary Table

| ID  | Category | Severity | Effort | Description |
|-----|----------|----------|--------|-------------|
| S1  | Security | Critical | Medium | Shell injection with no sandboxing |
| S2  | Security | High     | Low    | SSRF via fetch_url |
| S3  | Security | High     | Low    | Unrestricted file writes |
| S4  | Security | Medium   | Medium | Plaintext auth token storage |
| B1  | Bug      | High     | Low    | UTF-8 panic in truncate_output |
| B2  | Bug      | Low      | Low    | Token counter overflow |
| B3  | Bug      | Low      | Low    | Weak session ID generation |
| B4  | Bug      | Medium   | Medium | Fragile HTML scraper |
| B5  | Bug      | Low      | Medium | Non-compliant SSE parser |
| R1  | Robust   | High     | Low    | No HTTP timeout |
| R2  | Robust   | High     | Medium | Unbounded history growth |
| R3  | Robust   | Medium   | Low    | No retry for transient errors |
| R4  | Robust   | Low      | Low    | SSH connection leak on exit |
| R5  | Robust   | Low      | Low    | Redundant HTTP client creation |
| T1  | Test     | High     | Medium | No agentic loop integration tests |
| T2  | Test     | High     | High   | No REPL/main.rs tests |
| T3  | Test     | Medium   | Low    | Config loading not unit-testable |
| T4  | Test     | Low      | Medium | No fuzz testing for parsers |
| D1  | Design   | Medium   | Medium | Tool trait lacks context injection |
| D2  | Design   | Medium   | Medium | ExecutionContext code duplication |
| D3  | Design   | Low      | High   | No plugin/extension mechanism |
| D4  | Design   | Low      | Medium | Renderer not mockable |
| U1  | UX       | High     | High   | No streaming output |
| U2  | UX       | Medium   | Low    | Context limit warning not actionable |
| U3  | UX       | Low      | Low    | No protocol switch warning |
| U4  | UX       | Medium   | Medium | Cryptic error messages |
| U5  | UX       | Low      | Low    | No persistent command history |
| C1  | Code     | Medium   | Medium | Monolithic main.rs |
| C2  | Code     | Low      | Low    | Duplicated config test logic |
| C3  | Code     | Low      | Low    | No legacy deprecation timeline |

---

## Recommended Action Order

1. **B1** — Fix UTF-8 truncation panic (5 minutes, prevents production crashes)
2. **R1** — Add HTTP timeouts (10 minutes, prevents hangs)
3. **S2** — Block private IPs in fetch_url (30 minutes)
4. **S3** — Add file path restrictions (1 hour)
5. **S1** — Add shell command guardrails (2-4 hours for allowlist/denylist)
6. **R2** — Implement conversation pruning (2-4 hours)
7. **T1** — Add mock-based agentic loop tests (4-8 hours)
8. **U1** — Implement streaming output (8-16 hours)
9. **C1** — Extract main.rs into modules (4-8 hours)
10. **D1/D2** — Tool context injection + backend trait (8-16 hours)
