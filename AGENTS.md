# Agents

Instructions for AI agents working on this codebase.

## Project overview

This is a Rust AI agent that works with OpenAI-compatible APIs. It is both a library crate (`src/lib.rs`) and a CLI binary (`src/main.rs`). See `DESIGN.md` for full architectural details.

Maintain an ai-state.md file with whatever you (or a helper AI agent) will need to quickly understand and get working on this codebase without having to dig through it too much. It's not meant for humans, so it can be dense and compact -- be helpful to other AIs :-)

## Documentation

Keep a README.md with a high level overview of the system, quickstart instructions, pre-requisites, etc. There should also be robust build and test instructions and pointers to other docs.

Keep a `docs/` directory with things like architecture.md, playbook.md, etc. which has relevant information on architecture, design, and usage.

Keep `DESIGN.md` up to date. It must include a `## Features` section that lists all currently implemented user-facing and developer-relevant features (tools, CLI flags, execution targets, REPL UX, prompt behavior, output/rendering behavior, token/context behavior, and compatibility behaviors). Any feature addition, removal, or behavior change should update that section in the same change.

Try to keep docs updated as the system evolves.

If you're working with some complicated dependencies, like libraries, or infrastructure, or special tooling, create a dedicated "tips" document under docs/ (e.g., "rust-tips.md") so you can quickly build relevant context without having to figure things out again. This is especially helpful if you had to search the internet for stuff, or had to do a lot of digging to figure out. General context can remain in `ai-state.md`.

## Build and test

```bash
cargo build          # compile
cargo test           # run all tests (offline, no network)
cargo run            # interactive mode
cargo run -- "prompt" # one-shot mode
```

## Code structure

```
src/
  lib.rs          Public library re-exports
  main.rs         Binary entry point (CLI wiring, REPL)
  cli.rs          clap argument parsing
  config.rs       TOML config loading + env var overrides + default config bootstrap
  prompt.rs       Single-file system prompt template renderer
  templates/
    buddy.toml    Compiled default config template written to ~/.config/buddy/buddy.toml
    models.toml   Compiled model context-window catalog used by tokens.rs
    system_prompt.template Built-in system prompt template
  session.rs      Persistent session store (.buddyx/sessions; legacy .agentx fallback)
  types.rs        OpenAI API data model (Message, ChatRequest, ChatResponse, etc.)
  api.rs          Async HTTP client for /chat/completions
  agent.rs        Core agentic loop
  tokens.rs       Token estimation + tracking
  render.rs       Backward-compatible re-export for terminal renderer
  tui/            Terminal UI module (editor, prompt, layout, renderer, progress)
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
    capture_pane.rs capture-pane tool (tmux pane snapshots with optional delay)
    shell.rs      run_shell tool
    fetch.rs      fetch_url tool
    files.rs      read_file / write_file tools
    search.rs     web_search tool (DuckDuckGo)
    send_keys.rs  send-keys tool (tmux key injection for interactive control)
    time.rs       time tool (harness-recorded wall-clock formats)
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
4. Register it in `main.rs` (gated by a config flag if appropriate)
5. Add a config flag to `ToolsConfig` in `config.rs` if it should be toggleable
6. Add tests

## Key design constraints

- The `Agent` struct is the only thing that orchestrates the agentic loop. Don't add loop logic elsewhere.
- Tool definitions use raw `serde_json::Value` for parameters (JSON Schema). Don't add a schema generation dependency.
- The API client is intentionally simple: no retries, no streaming, no middleware. Keep it that way until there's a concrete need.
- Config loading order matters: env vars > CLI flags > local file > global file > defaults. Don't change the precedence.

## Coding Style

Write cohesive, decoupled, modular code. Interfaces should be simple, crisp, and clear. Design for testability and extensibility. Try to keep functions small, tight, and reusable. Try to avoid very large files -- break them up into smaller cohesive files as needed.

Always write tests -- and run them in between changes.

### Commenting Style

Write lots of comments, be detailed (but concise) -- humans should be able to read the code and understand what's going on. Pay attention to the why more than the what or how, however do comment on the what and how if things are complex.

Every function, constant, enum, class, or major relevant identifier should be clearly commented. Try to avoid long functions, but in cases where they're unavoidable, make sure to add a lot more commenting to the body so a human can understand what's going on.

### Terminal Output

Use pretty terminal output: use colors, minimal glyphs (check marks, crosses, etc.), and indentation for hierarchy. All output should be consistent looking, and really make use of terminal display features. Add progress bars and/or live update areas for long operations. Write a custom helper library for this if needed.
