# AI State (quick onboarding)

## 30-second orientation
- Project: `buddy` Rust crate (`src/lib.rs`) + CLI (`src/main.rs`) for OpenAI-compatible agent workflows.
- Core loop ownership: `Agent` orchestrates conversation/tool execution; do not duplicate loop control in other modules.
- Canonical behavior inventory: `docs/design/DESIGN.md`.
- Architecture index: `docs/design/architecture.md`.
- Runtime + REPL usage/operator docs: `docs/design/terminal-repl.md`, `docs/design/tools.md`, `docs/design/remote-execution.md`.

## Fast local commands
```bash
cargo build
cargo test
cargo run
cargo run -- exec "prompt"
```

## High-value file map
- CLI/startup wiring: `src/main.rs`, `src/cli.rs`.
- Agent/runtime orchestration: `src/agent/`, `src/runtime/`, `src/app/`.
- API/auth/config: `src/api/`, `src/auth/`, `src/config/`.
- Tooling: `src/tools/` (+ execution backends under `src/tools/execution/`).
- Terminal UI layers: `src/ui/`, `src/repl/`, `src/tui/`.
- Shared tmux domain: `src/tmux/`.

## Invariants to preserve
- Error model: explicit handwritten enums; avoid `anyhow`/`thiserror`.
- Output split: status/chrome on stderr, assistant/user-facing content on stdout.
- Tool outputs must stay truncated to protect context window.
- Config precedence: env > CLI flags > local file > global file > defaults.
- Tests should stay offline and deterministic.

## Current defaults and operator-facing behavior
- Global config lives at `~/.config/buddy/buddy.toml` (bootstrapped when missing).
- Sessions persist in `.buddyx/sessions` (`.agentx` legacy fallback still supported).
- REPL history persists in `~/.config/buddy/history` when enabled.
- Local execution commonly uses tmux-backed contexts; see `docs/tips/tmux.md` + `docs/design/remote-execution.md`.

## Docs map for AI agents
- Core docs: `README.md`, `docs/design/DESIGN.md`, `docs/design/architecture.md`.
- Operational references: `docs/design/tools.md`, `docs/design/terminal-repl.md`, `docs/design/remote-execution.md`, `docs/developer/deprecations.md`.
- Tactical guidance: `docs/tips/*.md`.
- Planning/history: `docs/plans/`.

## Update policy for this file
- Keep this file concise and current; replace stale text instead of appending timelines.
- Keep only context needed to start productive work quickly.
- Move tactical playbooks/tips into `docs/tips/`.
