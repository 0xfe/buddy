# Engineering Review: Buddy (Round 2)

**Date:** 2026-02-28
**Reviewer:** Claude (Sonnet 4.6)
**Scope:** Post-remediation review — security hardening gaps, new code introduced during remediation, architecture evolution
**Baseline:** All 30 findings from `claude-feedback-0.md` addressed; ~10,500 lines added across 51 files; 258 lib + 37 bin tests pass.

---

## Priority 1 — Security

### S1. DNS rebinding bypasses SSRF controls in `fetch_url`

**Location:** `src/tools/fetch.rs:129-137` (execute), `src/tools/fetch.rs:179-251` (validate_url_policy)

`validate_url_policy()` resolves DNS and checks IPs against `is_forbidden_ip()`, but the actual HTTP request at line 130 (`self.http.get(url).send()`) triggers a *second* DNS resolution inside `reqwest`. Between the two resolutions, an attacker-controlled DNS server can rotate the A record from a public IP to `127.0.0.1` (classic DNS rebinding).

The validated `reqwest::Url` carries only the hostname — `reqwest` re-resolves independently. This is a TOCTOU (time-of-check-to-time-of-use) gap.

**Impact:** Full SSRF bypass when the LLM is tricked into fetching a malicious domain.

**Proposed fixes:**
1. Use `reqwest::Client::builder().resolve(hostname, validated_socket_addr)` to pin the resolved IP for the actual request.
2. Alternatively, construct the request URL with the validated IP as the host and pass the original hostname via the `Host` header.

### S2. IPv4-mapped IPv6 addresses bypass SSRF IP checks

**Location:** `src/tools/fetch.rs:159-177` (is_forbidden_ip)

The IPv6 branch does not detect IPv4-mapped addresses like `::ffff:127.0.0.1` or `::ffff:10.0.0.1`. Rust's `Ipv6Addr` methods (`is_unicast_link_local()`, `is_loopback()`) do not flag these mapped addresses as private. A URL like `http://[::ffff:127.0.0.1]:8080/` passes all IPv6 checks.

**Fix:** For `Ipv6Addr`, call `to_ipv4_mapped()` (or `to_ipv4()`) and re-apply the IPv4 forbidden checks on the mapped address.

### S3. File write path restrictions bypassed by symlinks

**Location:** `src/tools/files.rs:161-177` (normalize_target_path), `src/tools/files.rs:186-200` (normalize_lexical)

Path validation uses purely lexical normalization — `..` is resolved textually without touching the filesystem. If a symlink exists at an allowed path pointing outside the allowed tree, the write follows the symlink while validation sees only the allowed prefix.

Example: If `/tmp/project/escape` is a symlink to `/etc`, writing to `/tmp/project/escape/crontab` passes the allowlist check but writes to `/etc/crontab`.

**Fix:** Canonicalize the target path after lexical normalization (e.g., `fs::canonicalize()` or manual symlink resolution) and re-check the canonical path against the policy. Handle the case where the target doesn't exist yet (canonicalize the deepest existing ancestor).

### S4. Encrypted auth store uses `thread_rng()` instead of CSPRNG for key material

**Location:** `src/auth.rs:633`, `src/auth.rs:637`, `src/auth.rs:801`

The encrypted auth store generates cryptographic salts, DEKs, and nonces using `rand::thread_rng()`. While `thread_rng()` in `rand` 0.8 is currently ChaCha12 (practically strong), the API contract does not guarantee CSPRNG properties — that's what `OsRng` is for. The session module correctly uses `OsRng` at `session.rs:215`.

**Fix:** Replace `rand::thread_rng().fill_bytes(&mut ...)` with `OsRng.fill_bytes(&mut ...)` in all three locations.

---

## Priority 2 — Security (Defense-in-Depth)

### S5. Shell denylist trivially bypassed via quoting/encoding

**Location:** `src/tools/shell.rs:366-374` (matched_denylist_pattern)

The denylist uses case-insensitive substring matching (`lowered.contains(pattern)`). Bypasses include:

- Whitespace insertion: `rm  -rf  /` (double space)
- Shell quoting: `r"m" -rf /`
- Backslash escapes: `rm\ -rf /`
- Variable expansion: `$'rm' -rf /`
- Subshell: `$(echo rm) -rf /`
- Backtick expansion: `` `echo rm` -rf / ``
- Fork bomb variant: `:(){ :|: & };:` (space before `&`)

**Mitigation context:** The interactive confirmation prompt (`shell_confirm=true`) is the real safety gate. The denylist is defense-in-depth. But under `--dangerously-auto-approve` or `/approve all`, the denylist becomes the only defense.

**Proposed fixes:**
1. Accept that substring matching is inherently bypassable and document it as advisory, not a security boundary.
2. For stronger protection, shell-parse the command (split on pipes/semicolons/`&&`/`||`, expand simple quotes) before matching.
3. Consider matching against the first token of each pipeline segment (the command name) rather than the full string.

### S6. Machine-derived KEK has low effective entropy

**Location:** `src/auth.rs:760-779` (machine_secret_material)

The KEK is derived from: OS name, hostname, `$USER`, home directory, and `/etc/machine-id`. On macOS, `/etc/machine-id` does not exist, so `machine_id` is always empty. The remaining inputs (hostname, username, home) are often publicly known or guessable. Any local user on the same machine can derive the KEK and decrypt the auth store.

This is not necessarily wrong — the threat model may accept "same-user local access" — but it should be documented. The encryption protects against backup exfiltration and casual filesystem browsing, not determined local attackers.

**Proposed improvements:**
1. Document the threat model explicitly (what the encryption protects against and what it doesn't).
2. On macOS, use `IOPlatformUUID` from `ioreg` as a stable machine identifier.
3. Consider optional passphrase-based KEK for users who want stronger protection.

---

## Priority 3 — Robustness

### R1. Auth store write is not atomic

**Location:** `src/auth.rs:606-621` (write_store)

The auth store is written using `OpenOptions::create(true).truncate(true)` directly to the target path. A crash or power loss mid-write leaves the file truncated or corrupt. The session store correctly uses write-to-temp + rename (`session.rs:103-115`).

**Fix:** Write to a temp file in the same directory, then `fs::rename()` to the target path. This is atomic on POSIX filesystems.

### R2. `scrypt` with `recommended()` parameters adds startup latency

**Location:** `src/auth.rs:753`

`ScryptParams::recommended()` uses high-cost parameters designed for password hashing (N=2^15 or higher). This is called on every auth store read/write, which happens at startup for token refresh checks. On low-resource machines (CI runners, containers, Raspberry Pi), this adds 100-500ms per operation.

Since the input is machine-derived material (not a user-chosen password), the KDF cost can be lower without meaningful security loss.

**Fix:** Use explicit parameters like `ScryptParams::new(14, 8, 1)` — still expensive enough to deter brute-force against the low-entropy machine inputs, but fast enough for interactive startup.

### R3. Token estimation heuristic can misfire for non-ASCII content

**Location:** `src/tokens.rs:48-70` (estimate_messages)

The 1-token-per-4-chars heuristic works reasonably for English prose and code but diverges significantly for:
- CJK text: ~1 token per 1-2 characters (estimate is 2-4x too low)
- Emoji-heavy content: ~1 token per emoji (estimate is too low)
- Highly repetitive boilerplate: tokens compress better (estimate is too high)

The hard context limit check at `CONTEXT_HARD_LIMIT_FRACTION = 0.95` relies on this estimate, meaning conversations can either be cut too aggressively (false positive) or blow past the limit (false negative producing an API error).

**Proposed improvements:**
1. Use the actual token count from the last API response as calibration for the next estimate (the `usage` field returns exact counts).
2. Track the ratio of estimated-to-actual across the session and apply a correction factor.

### R4. `as i64` truncation in `unix_now_secs()`

**Location:** `src/auth.rs:883-888`

`.as_secs() as i64` silently truncates in year 2262. While not urgent, it's a time bomb in a security-critical path (token expiry checks).

**Fix:** Use `i64::try_from(secs).unwrap_or(i64::MAX)`.

---

## Priority 4 — Architecture & Code Organization

### A1. `main.rs` grew larger during remediation (~2825 lines)

**Location:** `src/main.rs`

The original review flagged `main.rs` at ~1100 lines (C1). After remediation, it's ~2825 lines. While some logic was extracted (`cli_event_renderer.rs` at 419 lines, `runtime.rs` at 1674 lines), `main.rs` absorbed the runtime command/event wiring, approval flow integration, and streaming setup.

The file now has multiple distinct concerns: CLI argument parsing, config loading, REPL orchestration, runtime command dispatch, session management, background task coordination, signal handling, and startup validation.

**Proposed extraction targets:**
1. `src/cli.rs` — argument parsing and subcommand dispatch (~300 lines)
2. `src/startup.rs` — config loading, preflight, session resolution (~400 lines)
3. `src/repl.rs` — the interactive REPL loop and input handling (~500 lines)
4. Keep `main.rs` as thin glue (~200 lines) that wires these together.

### A2. `execution.rs` is ~2631 lines with deep nesting

**Location:** `src/tools/execution.rs`

Despite the `ExecutionBackendOps` trait extraction, this file remains very large. It contains the trait definition, five backend implementations, SSH context management, tmux session handling, process spawning, and extensive test code. The SSH connection lifecycle alone is ~400 lines.

**Proposed split:**
1. `src/tools/execution/mod.rs` — trait + `ExecutionContext` wrapper
2. `src/tools/execution/local.rs` — local and local-tmux backends
3. `src/tools/execution/container.rs` — container backends
4. `src/tools/execution/ssh.rs` — SSH backend + control connection management

### A3. `RuntimeApprovalPolicy::None` naming is confusing

**Location:** `src/runtime.rs:905-919`

The enum variant `RuntimeApprovalPolicy::None` means "deny all", which is the opposite of what "none" typically implies in policy contexts (e.g., "no approval needed" = auto-approve). This will cause operator confusion.

**Fix:** Rename to `RuntimeApprovalPolicy::DenyAll` or `RuntimeApprovalPolicy::Manual`.

---

## Priority 5 — Testability & Maintenance

### T1. Container and SSH execution backends have no unit tests

**Location:** `src/tools/execution.rs`

The `ContainerBackend`, `ContainerTmuxBackend`, and `SshBackend` implementations have no unit tests. Testing requires actual Docker/SSH infrastructure. Even basic logic tests (command construction, path handling, option serialization) could run without real backends.

**Proposed fix:** Extract command-building logic into pure functions that can be tested independently, separate from the I/O that executes them.

### T2. No integration test for the full runtime command/event loop

**Location:** `src/runtime.rs`

The runtime command/event architecture is the new core orchestration layer but has no end-to-end test. A test that sends `RuntimeCommand::SubmitPrompt` through to `RuntimeEvent::TaskComplete` with a mock `ModelClient` would catch regressions in the command dispatch and event emission pipeline.

### T3. `DefaultHasher` used for SSH control path uniqueness is not stable

**Location:** `src/tools/execution.rs:1084-1095` (build_ssh_control_path)

`std::collections::hash_map::DefaultHasher` is not guaranteed stable across Rust versions. If the binary is updated and an orphaned SSH control socket exists from a previous build, the new binary won't find it for cleanup. This is a minor resource leak.

**Fix:** Use a stable hash like `sha2` (already a dependency) or a simple deterministic string construction.

### T4. `rand` 0.8 is outdated

**Location:** `Cargo.toml:27`

`rand` 0.8 is superseded by 0.9 which changed `thread_rng()` semantics. Neither has known vulnerabilities, but staying current matters for ongoing security patches. The `hostname` crate (0.4) is also in maintenance mode.

### T5. No `rust-toolchain.toml` despite nightly-gated API usage

**Location:** project root (missing file)

The codebase uses APIs like `is_none_or` (`agent.rs:1057`) and `is_unicast_link_local` (`fetch.rs:173`) that may require nightly or recent stable Rust. Without a pinned toolchain file, builds can break silently on older Rust versions.

**Fix:** Add `rust-toolchain.toml` with the minimum supported Rust version.

---

## Priority 6 — Minor / Hygiene

### M1. Auth store write races with concurrent processes

**Location:** `src/auth.rs:606-621`

If two buddy processes attempt to update the auth store simultaneously (e.g., parallel exec invocations refreshing tokens), the non-atomic write can produce a corrupt file. The atomic write fix (R1 above) partially addresses this, but a file lock (`flock`) would fully solve concurrent access.

### M2. Session ID validation allows hidden files

**Location:** `src/session.rs:182-198` (validate_session_id)

While `.` and `..` are rejected, IDs like `.hidden` are accepted. Combined with the `.json` extension, this creates hidden files (`.hidden.json`) on Unix. The validation also trims whitespace but later uses the untrimmed value.

**Fix:** Reject IDs starting with `.`.

### M3. Login poll loop could be aggressive

**Location:** `src/auth.rs:452-453`

The device login poll interval is clamped to `max(1, server_interval)` seconds. If the server returns `interval=0`, polling happens every 1 second for up to 15 minutes (900 requests). While the `max(1)` prevents a tight loop, a floor of 5 seconds would be more respectful.

---

## Summary Table

| ID  | Category | Severity | Effort | Description |
|-----|----------|----------|--------|-------------|
| S1  | Security | High     | Low    | DNS rebinding bypasses SSRF controls |
| S2  | Security | High     | Low    | IPv4-mapped IPv6 bypasses IP checks |
| S3  | Security | High     | Low    | Symlink traversal bypasses file path restrictions |
| S4  | Security | Medium   | Low    | `thread_rng()` for crypto key material |
| S5  | Security | Medium   | Medium | Shell denylist trivially bypassable |
| S6  | Security | Low      | Low    | Machine KEK has low effective entropy |
| R1  | Robust   | Medium   | Low    | Auth store write not atomic |
| R2  | Robust   | Low      | Low    | Scrypt cost too high for startup |
| R3  | Robust   | Low      | Low    | Token estimation diverges for non-ASCII |
| R4  | Robust   | Low      | Low    | `as i64` truncation in unix time |
| A1  | Arch     | Medium   | Medium | main.rs grew to ~2825 lines |
| A2  | Arch     | Low      | Medium | execution.rs still ~2631 lines |
| A3  | Arch     | Low      | Low    | Confusing `RuntimeApprovalPolicy::None` naming |
| T1  | Test     | Medium   | Medium | Container/SSH backends untested |
| T2  | Test     | Medium   | Medium | No runtime command/event integration test |
| T3  | Test     | Low      | Low    | Unstable DefaultHasher for SSH paths |
| T4  | Maint    | Low      | Low    | Outdated `rand` 0.8 dependency |
| T5  | Maint    | Low      | Low    | No rust-toolchain.toml |
| M1  | Maint    | Low      | Low    | Auth store write races |
| M2  | Maint    | Low      | Low    | Session ID allows hidden files |
| M3  | Maint    | Low      | Low    | Login poll floor too low |

---

## Recommended Action Order

1. **S1** — Pin resolved IP in fetch to close DNS rebinding (small change in `fetch.rs`)
2. **S2** — Add IPv4-mapped IPv6 check (3-line fix in `is_forbidden_ip`)
3. **S3** — Canonicalize file write targets before policy check (small change in `files.rs`)
4. **S4** — Switch `thread_rng()` to `OsRng` in auth crypto (3 lines)
5. **R1** — Atomic auth store writes via temp+rename (10 minutes)
6. **S5** — Document denylist as advisory; optionally add command-name extraction
7. **A1** — Extract CLI/startup/REPL from main.rs (half-day refactor)
8. **T1/T2** — Add backend command-building tests and runtime integration test
