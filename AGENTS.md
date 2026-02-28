# Agents

Instructions for AI agents working on this codebase.

## Project overview

This is a Rust AI agent that works with OpenAI-compatible APIs. It is both a library crate (`src/lib.rs`) and a CLI binary (`src/main.rs`). See `DESIGN.md` for full architectural details.

Maintain `ai-state.md` as a compact quick-onboarding handoff for AI agents. Keep it dense, current, and short enough to scan quickly.

## Documentation

Keep a README.md with a high level overview of the system, quickstart instructions, pre-requisites, etc. There should also be robust build and test instructions and pointers to other docs.

Keep a `docs/` directory with things like architecture.md, playbook.md, etc. which has relevant information on architecture, design, and usage.

Keep `DESIGN.md` up to date. It must include a `## Features` section that lists all currently implemented user-facing and developer-relevant features (tools, CLI flags, execution targets, REPL UX, prompt behavior, output/rendering behavior, token/context behavior, and compatibility behaviors). Any feature addition, removal, or behavior change should update that section in the same change.

Try to keep docs updated as the system evolves.

If you're working with complicated tooling/infrastructure/workflows, add or update short practical notes under `docs/tips/` (for example tmux, shell, testing, docs hygiene). Keep tactical details in those tip files; keep `ai-state.md` high-signal and brief.

## AI state workflow

- Treat `ai-state.md` as a fast cache for the next AI: current architecture map, active defaults, and immediate gotchas.
- Do not keep a long chronological changelog there. Replace stale sections instead of appending.
- Move deep operational guidance to `docs/tips/*.md` and long-lived planning details to `docs/plans/`.
- Aim for a quick-scan document (roughly <= 150 lines unless there is a strong reason to exceed it).

## Build and test

```bash
cargo build          # compile
cargo test           # run all tests (offline, no network)
cargo run            # interactive mode
cargo run -- exec "prompt" # one-shot mode
```

## Code structure

```
src/
  lib.rs          Public library re-exports
  main.rs         Binary entry point (CLI wiring + startup branch)
  cli.rs          clap argument parsing
  app/            App orchestration (entry, REPL/exec mode, approvals, startup)
  runtime/        Runtime command/event actor + schemas
  agent/          Core agentic loop + history/normalization/prompt augmentation
  api/            Model transport (completions + responses + retries/policy)
  auth/           Login flows + encrypted credential storage
  config/         TOML/env config loading + resolution + init
  prompt.rs       System prompt template renderer
  preflight.rs    Startup/model-switch config validation
  templates/
    buddy.toml    Compiled default config template written to ~/.config/buddy/buddy.toml
    models.toml   Compiled model context-window catalog used by tokens.rs
    system_prompt.template Built-in system prompt template
  session.rs      Persistent session store (.buddyx/sessions; legacy .agentx fallback)
  types.rs        OpenAI API data model (Message, ChatRequest, ChatResponse, etc.)
  tokens.rs       Token estimation + tracking
  textutil.rs     UTF-8-safe truncation helpers
  ui/             Rendering contracts + runtime event renderer + terminal facade
  repl/           Shared REPL/runtime helper state
  tui/            Terminal UI implementation (editor/layout/render/progress)
    mod.rs        TUI public surface + re-exports
    commands.rs   Slash command metadata, parsing, autocomplete matching
    input.rs      Interruptible raw-mode editor loop
    input_buffer.rs Input/history editing primitives
    input_layout.rs Terminal row/column wrap math for editor surface
    prompt.rs     Prompt text and styled prompt/status rendering helpers
    renderer.rs   Status/chrome rendering API used across app
    progress.rs   Spinner/liveness primitives + progress metrics
    settings.rs   Centralized hardcoded UI constants (colors/glyphs/prompts/indent)
    text.rs       Shared single-line truncation helper
  error.rs        Error enums (AgentError, ApiError, ConfigError, ToolError)
  tools/
    mod.rs        Tool trait (async) + ToolRegistry
    execution/    Local/container/ssh execution backends
    capture_pane.rs capture-pane tool (tmux pane snapshots with optional delay)
    shell.rs      run_shell tool
    fetch.rs      fetch_url tool
    files.rs      read_file / write_file tools
    search.rs     web_search tool (DuckDuckGo)
    send_keys.rs  send-keys tool (tmux key injection for interactive control)
    time.rs       time tool (harness-recorded wall-clock formats)
  tmux/           Shared tmux session/pane/capture/send/run domain
```

## Conventions

- **Error handling**: Hand-written error enums with `Display` and `From` impls. No `anyhow` or `thiserror`. Errors propagate via `?` to `AgentError` at the top level. Only `main.rs` calls `process::exit()`.
- **Dependencies**: Minimal. Every dependency should justify its inclusion. No utility crates (`anyhow`, `thiserror`, `derive_more`, etc.) when manual impls are short.
- **Testing**: Inline `#[cfg(test)] mod tests` at the bottom of each module. Tests should run offline with no network access.
- **Output separation**: Status/chrome goes to stderr (`eprintln!`), assistant responses go to stdout (`println!`). This enables piping.
- **Tool output**: Always truncated to prevent context window exhaustion. Shell: 4K, files/fetch: 8K.
- **Async**: The `Tool` trait is async via `async_trait`. Tools use `tokio::process`, `tokio::fs`, and async reqwest.

## Adding a new tool

1. Create `src/tools/your_tool.rs`
2. Implement the `Tool` trait (name, definition with JSON Schema, async execute)
3. Add `pub mod your_tool;` to `src/tools/mod.rs`
4. Register it in `src/app/entry.rs` (`build_tools`) (gated by a config flag if appropriate)
5. Add a config flag to `ToolsConfig` in `src/config/types.rs` if it should be toggleable
6. Add tests

## Key design constraints

- The `Agent` struct is the only thing that orchestrates the agentic loop. Don't add loop logic elsewhere.
- Tool definitions use raw `serde_json::Value` for parameters (JSON Schema). Don't add a schema generation dependency.
- Keep API behavior centralized in `src/api/*` (protocol routing, retries, auth transport policy) rather than duplicating provider logic in higher layers.
- Config precedence matters: CLI flags > env vars > local file > global file > defaults. Don't change the precedence.

## Coding Style

Write cohesive, decoupled, modular code. Interfaces should be simple, crisp, and clear. Design for testability and extensibility. Try to keep functions small, tight, and reusable. Try to avoid very large files -- break them up into smaller cohesive files as needed.

Always write tests -- and run them in between changes.

### Commenting Style

Write lots of comments, be detailed (but concise) -- humans should be able to read the code and understand what's going on. Pay attention to the why more than the what or how, however do comment on the what and how if things are complex.

Every function, constant, enum, class, or major relevant identifier should be clearly commented. Try to avoid long functions, but in cases where they're unavoidable, make sure to add a lot more commenting to the body so a human can understand what's going on.

### Terminal Output

Use pretty terminal output: use colors, minimal glyphs (check marks, crosses, etc.), and indentation for hierarchy. All output should be consistent looking, and really make use of terminal display features. Add progress bars and/or live update areas for long operations. Write a custom helper library for this if needed.

### Committing

Always commit after a change. Use `git add` to stage all relevant changes, and `git commit -m "<message>"` to commit. The commit message should have a short summary of the changes, and a blank line, followed by a more detailed description of the changes. If you're working with planning docs, include the commit IDs when you mark tasks closed.
