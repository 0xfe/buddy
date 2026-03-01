# Developer Guide

This document covers library embedding and extension points. For build/test/release commands, use [docs/developer/BUILD.md](BUILD.md).

## Run from source

```bash
cargo run -- exec "your prompt here"
cargo run
```

## Use as a library

Add to your `Cargo.toml`:

```toml
[dependencies]
buddy = { path = "../buddy" }
```

Then in your code:

```rust
use buddy::agent::Agent;
use buddy::config::load_config;
use buddy::tools::execution::ExecutionContext;
use buddy::tools::shell::ShellTool;
use buddy::tools::ToolRegistry;

#[tokio::main]
async fn main() {
    let config = load_config(None).unwrap();
    let execution = ExecutionContext::local();

    let mut tools = ToolRegistry::new();
    tools.register(ShellTool {
        confirm: true,
        denylist: vec!["rm -rf /".to_string(), "mkfs".to_string()],
        color: true,
        execution: execution.clone(),
        approval: None,
    });

    let mut agent = Agent::new(config, tools);
    let response = agent.send("Hello!").await.unwrap();
    println!("{response}");
}
```

## Implement a custom tool

```rust
use async_trait::async_trait;
use buddy::error::ToolError;
use buddy::tools::{Tool, ToolContext};
use buddy::types::{FunctionDefinition, ToolDefinition};

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &'static str {
        "my_tool"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: "my_tool".into(),
                description: "Does something useful.".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
        }
    }

    async fn execute(
        &self,
        arguments: &str,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let _ = arguments;
        Ok("result".into())
    }
}
```

## Runtime embedding

An alternate frontend can be built against the runtime actor/event API:

```bash
cargo run --example alternate_frontend -- "list files"
```

## Extension points

- Model transport/runtime integration:
  - `src/api/mod.rs` exposes `ModelClient`.
  - `src/runtime/{schema,mod}.rs` exposes runtime command/event channels.
- Tools:
  - implement `Tool` in `src/tools/mod.rs`
  - register tools in `src/app/entry.rs` (`build_tools`)
  - shared execution backends are in `src/tools/execution/*`
- Terminal/runtime rendering:
  - `src/ui/render.rs` exposes `RenderSink`
  - `src/ui/runtime/*` converts runtime events to render actions
  - `src/repl/*` contains shared REPL/runtime helper state
- Config/auth:
  - `src/config/*` handles layered config loading and profile resolution
  - `src/auth/*` handles provider login flows and encrypted credential storage

## Architecture docs

- High-level: [docs/design/DESIGN.md](../design/DESIGN.md)
- Detailed module map: [docs/design/module-map.md](../design/module-map.md)
- Runtime/protocol behavior: [docs/design/runtime-and-protocols.md](../design/runtime-and-protocols.md)
- Tool/execution contracts: [docs/design/tools-and-execution.md](../design/tools-and-execution.md)
