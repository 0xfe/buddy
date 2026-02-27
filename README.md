# buddy -- a sysadmin AI assistant

A terminal AI agent written in Rust with native `tmux` support. Specifically designed to be a sysadmin assistant that can work on remote servers, local containers, or directly on the local host.

Works with any OpenAI API-compatible model — OpenAI, Ollama, OpenRouter, vLLM, LM Studio, or anything else that speaks the same protocol.

Runs as an interactive REPL or in one-shot mode. Handles multi-step tool-use loops automatically: the model can run shell commands, fetch URLs, read/write files, search the web, capture tmux panes (when available), and read harness time, with results fed back into the conversation until the model produces a final answer.

Interactive mode includes slash commands with live autocomplete (type `/`), history navigation (`↑`/`↓`), multiline entry (`Alt+Enter`), background task control (`/ps`, `/kill <id>`), session persistence (`/session`), and foreground approval handoff for `run_shell` confirmations.

Usable as both a standalone CLI binary and a Rust library crate.

## Pre-requisites

- **Rust** 1.70+ (install via [rustup](https://rustup.rs/))
- An OpenAI-compatible API endpoint. Any of:
  - [OpenAI](https://platform.openai.com/) — API key auth or `buddy login`
  - [Ollama](https://ollama.ai/) — runs locally, no API key needed
  - [OpenRouter](https://openrouter.ai/) — multi-model gateway
  - [LM Studio](https://lmstudio.ai/), [vLLM](https://vllm.ai/), or any server implementing OpenAI-compatible `/v1/chat/completions` or `/v1/responses`

## Quickstart

**1. Build**

```bash
cargo build --release

# Install to $HOME/.local/bin
make install
```

**2. Configure**

The fastest way is via environment variables:

```bash
# Set base url and api key of Model provider
export BUDDY_BASE_URL="http://localhost:11434/v1"
export BUDDY_API_KEY="sk-..."
export BUDDY_MODEL="llama3.2"
```

Or use the generated config file:

```bash
# buddy creates ~/.config/buddy/buddy.toml on first startup if missing
$EDITOR ~/.config/buddy/buddy.toml
```

**3. Run**

```bash
# Interactive mode
buddy

# One-shot mode
buddy exec "how much free disk space do I have?"

# Login for the active/default profile
buddy login

# Run with tmux (buddy prints attach instructions on startup)
buddy --tmux
# example:
tmux attach -t buddy-1a2b

# Operate a remote ssh host
buddy --ssh user@hostname

# Operate a local container
buddy --container my-dev-container
```

**4. In the REPL**

```
> What files are in the current directory?
[|] task #1 running 0.8s

> /ps
  • background tasks
    #1: prompt "What files are in the current directory?" [running (1.2s)]

> /kill 1
warning: Cancelled task #1.

> What files are in the current directory?
dev@my-host$ ls -- approve? y
  • task #2 exited with code 0, stdout: "Cargo.toml src ..."
  • prompt #2 processed in 2.3s

Here are the files in the current directory:
- Cargo.toml
- src/
...

> /quit
```

Type `/` to open slash-command autocomplete.
Use `↑`/`↓` for history, and `Alt+Enter` to insert a newline without submitting.
Common editing shortcuts are supported (`Ctrl-A`, `Ctrl-E`, `Ctrl-B`, `Ctrl-F`, `Ctrl-K`, `Ctrl-U`, `Ctrl-W`).
In interactive mode, prompts run as background tasks so the REPL remains available; use `/ps` and `/kill <id>` to inspect/cancel active tasks.
If a background task reaches a shell confirmation point, input is interrupted and the confirmation is brought to the foreground as a one-line inline approval prompt (`user@host$ <command> -- approve?`).
Background task activity (reasoning traces and tool results) is forwarded to the foreground loop and rendered cleanly without breaking keyboard input.
A live liveness line (spinner + task state) is rendered above the prompt while background tasks are active.
When running in a tmux-backed execution target, use `run_shell` with `wait: false` to dispatch long-running/interactive commands, then poll with `capture-pane`. Use `send-keys` for control input (for example Ctrl-C, Ctrl-Z, Enter, arrows) when interacting with full-screen TUIs or stuck jobs.
On tmux-backed startup, buddy shows a friendly `tmux attach` command (local/SSH/container) and works in a shared window named `buddy-shared`.

- `/status` shows model, endpoint, enabled tools, and session stats.
- `/context` shows estimated context usage and recent token counts.
- `/ps` lists background tasks currently running.
- `/kill <id>` cancels a background task by ID.
- `/timeout <duration> [id]` sets/corrects background task deadlines.
- `/approve ask|all|none|<duration>` changes shell-approval policy.
- `/session` lists saved sessions; `/session resume <name|last>` resumes one; `/session new <name>` creates a fresh one.
- While background tasks are running, only `/ps`, `/kill <id>`, `/timeout <duration> [id]`, `/approve <mode>`, `/status`, and `/context` are accepted.
- For foreground approvals, reply `y`/`yes` to approve or `n`/`no` (or empty enter) to deny.

## Developers

**Build**

```bash
cargo build
# or:
make build
```

**Install to `~/.local/bin`**

```bash
make install
```

**Test**

```bash
cargo test
```

Tests cover config parsing, API type serialization/deserialization, token estimation, and message constructors. All tests run offline (no network).

**Run from source**

```bash
cargo run -- exec "your prompt here"
cargo run                          # interactive mode
```

**Use as a library**

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
use buddy::tools::ToolRegistry;
use buddy::tools::shell::ShellTool;

#[tokio::main]
async fn main() {
    let config = load_config(None).unwrap();
    let execution = ExecutionContext::local();

    let mut tools = ToolRegistry::new();
    tools.register(ShellTool {
        confirm: true,
        color: true,
        execution: execution.clone(),
        approval: None,
    });

    let mut agent = Agent::new(config, tools);
    let response = agent.send("Hello!").await.unwrap();
    println!("{response}");
}
```

Custom tools implement the `Tool` trait:

```rust
use async_trait::async_trait;
use buddy::tools::Tool;
use buddy::error::ToolError;
use buddy::types::{ToolDefinition, FunctionDefinition};

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }

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

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        Ok("result".into())
    }
}
```

## Configuring

Configuration is loaded with this precedence (highest wins):

1. **CLI flags** — `--config`, `--model`, `--base-url`, `--container`, `--ssh`, `--tmux`, `--no-color`
2. **Environment variables** — `BUDDY_API_KEY`, `BUDDY_BASE_URL`, `BUDDY_MODEL`
3. **Local config** — `./buddy.toml` in the current directory
4. **Global config** — `~/.config/buddy/buddy.toml` (auto-created on startup if missing)
5. **Built-in defaults**

Legacy compatibility:
- `AGENT_API_KEY`, `AGENT_BASE_URL`, and `AGENT_MODEL` are still accepted.
- If `buddy.toml` is not present, `agent.toml` is still loaded.

### Full config reference

```toml
[models.gpt-codex]
api_base_url = "https://api.openai.com/v1" # API endpoint
api = "responses"                           # responses | completions
auth = "login"                              # login | api-key
# Only one may be set: api_key, api_key_env, api_key_file.
# api_key_env = "OPENAI_API_KEY"            # env var name containing the key
# api_key = "sk-..."                        # inline key
# api_key_file = "/path/to/key.txt"         # file containing key bytes
model = "gpt-5.3-codex"                     # concrete provider model id
# context_limit = 128000                    # optional override; otherwise from models.toml catalog

[models.gpt-spark]
api_base_url = "https://api.openai.com/v1"
api = "responses"
auth = "login"
model = "gpt-5.3-spark"

[agent]
model = "gpt-codex"                         # active profile key from [models.<name>]
# system_prompt = "Optional additional operator instructions appended to the built-in template."
max_iterations = 20                         # safety cap on tool-use loops
# temperature = 0.7
# top_p = 1.0

[tools]
shell_enabled = true                       # run_shell tool
fetch_enabled = true                       # fetch_url tool
files_enabled = true                       # read_file / write_file tools
search_enabled = true                      # web_search tool (DuckDuckGo)
shell_confirm = true                       # prompt before running shell commands

[display]
color = true                               # ANSI color output
show_tokens = false                        # show token usage after each turn
show_tool_calls = true                     # show tool invocations inline
```

### CLI flags

```
buddy [OPTIONS] [COMMAND]

Commands:
  exec <PROMPT>           Execute one prompt and exit
  login [MODEL_PROFILE]   Login to provider for profile (defaults to [agent].model; shared per provider)
  help                    Print command help

Options:
  -c, --config <CONFIG>      Path to config file
  -m, --model <MODEL>        Override model profile key (if configured) or raw API model id
      --base-url <BASE_URL>  Override API base URL
      --container <ID/NAME>  Run shell/files tools with docker/podman exec in this container
      --ssh <USER@HOST>      Run shell/files tools on this host over persistent ssh
      --tmux [SESSION]       tmux session to use/create (default: auto buddy-xxxx per target; local by default, or on --ssh/--container target)
      --no-color             Disable color output
  -h, --help                 Print help
  -V, --version              Print version
```

At startup, the system prompt is rendered from one compiled template with runtime placeholders (enabled tools, execution target, and optional `[agent].system_prompt` operator instructions). When `--container` or `--ssh` is set, the rendered prompt includes explicit remote-target guidance.

### Built-in tools

| Tool | Description |
|------|-------------|
| `run_shell` | Execute shell commands. Output truncated to 4K chars. Optional user confirmation. `wait` can be `true` (default), `false` (tmux-backed targets; return immediately), or a timeout duration string like `10m`. Respects `--container`, `--ssh`, and `--tmux`. |
| `fetch_url` | HTTP GET a URL, return body as text. Truncated to 8K chars. |
| `read_file` | Read a file's contents. Truncated to 8K chars. Respects `--container`, `--ssh`, and `--tmux`. |
| `write_file` | Write content to a file. Creates or overwrites. Respects `--container`, `--ssh`, and `--tmux`. |
| `web_search` | Search DuckDuckGo and return top results with titles, URLs, and snippets. No API key needed. |
| `capture-pane` | Capture tmux pane output (with common `capture-pane` flags and optional delay) to inspect interactive/stuck terminal state. By default it uses tmux screenshot behavior (current visible pane content). |
| `send-keys` | Inject tmux keys/text into a pane (Ctrl-C/Ctrl-Z/Enter/arrows/literal text) for interactive control. |
| `time` | Return harness-recorded current wall-clock time in multiple common formats (epoch + UTC text formats). |

### REPL slash commands

| Command | Description |
|---------|-------------|
| `/status` | Show current model, base URL, enabled tools, and session counters. |
| `/model <name\|index>` | Switch the active configured model profile. |
| `/models` | List configured model profiles and pick one interactively. |
| `/login [name\|index]` | Start login flow for a configured profile. |
| `/context` | Show estimated context usage (`messages` estimate / context window) and token stats. |
| `/ps` | Show running background tasks with IDs and elapsed time. |
| `/kill <id>` | Cancel a running background task by task ID. |
| `/timeout <duration> [id]` | Set timeout for a background task (`id` optional only when one task exists). |
| `/approve ask|all|none|<duration>` | Configure shell approval policy for this REPL session. |
| `/session` | List saved sessions (ordered by last use). |
| `/session resume <name\|last>` | Resume a saved session by name, or the most recently used one. |
| `/session new <name>` | Create and switch to a fresh named session. |
| `/help` | Show slash command help (only when no background tasks are running). |
| `/quit` `/exit` `/q` | Exit interactive mode (only when no background tasks are running). |

### Context window catalog

Context-limit estimates are loaded from `src/templates/models.toml` (compiled into the binary).
The catalog is a local snapshot of common model IDs/families and their
`context_length` values (sourced from OpenRouter's models API), with an
`8192` fallback for unknown models.

### Provider examples

**OpenAI**
```bash
# API-key auth
export BUDDY_API_KEY="sk-..."
cargo run

# or login auth (when profile uses auth = "login")
buddy login gpt-codex
```

**Ollama**
```bash
ollama serve
export BUDDY_BASE_URL="http://localhost:11434/v1"
export BUDDY_MODEL="llama3.2"
cargo run
```

**OpenRouter**
```bash
export BUDDY_BASE_URL="https://openrouter.ai/api/v1"
export BUDDY_API_KEY="sk-or-..."
export BUDDY_MODEL="anthropic/claude-3.5-sonnet"
cargo run
```

## Links and references

- [OpenAI Chat Completions API](https://platform.openai.com/docs/api-reference/chat) — the protocol this agent speaks
- [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses/create) — supported per-profile with `api = "responses"`
- [OpenAI Function Calling guide](https://platform.openai.com/docs/guides/function-calling) — how tool use works in the API
- [Ollama OpenAI compatibility](https://ollama.ai/blog/openai-compatibility) — running models locally
- [OpenRouter docs](https://openrouter.ai/docs) — multi-provider API gateway
- [OpenRouter Models API](https://openrouter.ai/docs/api-reference/list-available-models) — source for `models.toml` context windows
- [clap](https://docs.rs/clap/) — CLI argument parsing
- [reqwest](https://docs.rs/reqwest/) — async HTTP client
- [crossterm](https://docs.rs/crossterm/) — terminal colors and styles
- [async-trait](https://docs.rs/async-trait/) — async methods in trait objects
- [serde](https://serde.rs/) — serialization framework
