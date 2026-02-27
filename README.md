# buddy -- a sysadmin AI assistant

A terminal AI agent written in Rust with native `tmux` support. Specifically designed to be a sysadmin assistant that can work on remote servers, local containers, or directly on the local host.

Works with any OpenAI API-compatible model — OpenAI, Ollama, OpenRouter, vLLM, LM Studio, or anything else that speaks the same protocol.

Usable as both a standalone CLI binary and a Rust library crate.

## Pre-requisites

- **Rust** 1.70+ (install via [rustup](https://rustup.rs/))
- `tmux` must be installed on the host you're operating on
- An OpenAI-compatible API endpoint. Any of:
  - [OpenAI](https://platform.openai.com/) — API key auth or `buddy login`
  - [Ollama](https://ollama.ai/) — runs locally, no API key needed
  - [OpenRouter](https://openrouter.ai/) — multi-model gateway
  - [LM Studio](https://lmstudio.ai/), [vLLM](https://vllm.ai/), or any server implementing OpenAI-compatible `/v1/chat/completions` or `/v1/responses`

## Models

Buddy supports most models that implement the OpenAI completions or responses APIs, and provides a broad set of common tools. We've explicitly tested with the models listed in `src/templates/buddy.toml`.

- `gpt-5.3-codex` on [OpenAI](https://platform.openai.com/)
- `gpt-5.3-codex-spark` on [OpenAI](https://platform.openai.com/)
- `kimi-k2.5` on [Moonshot AI](https://www.moonshot.ai/)
- `deepseek-v3.2` on [OpenRouter](https://openrouter.ai/)
- `glm-5` on [OpenRouter](https://openrouter.ai/)

## Quickstart

**1. Build**

```bash
cargo build --release

# Install to $HOME/.local/bin
make install
```

**2. Configure**

You can configure buddy via the config file at `~/.config/buddy/buddy.toml`.

```bash
# Initialize config files under ~/.config/buddy
buddy init

$EDITOR ~/.config/buddy/buddy.toml
```

**3. Run**

```bash
# Optional: If you're not using API keys, e.g., OpenAI user login
buddy login --check
buddy login
# If credentials are corrupted/stale:
buddy login --reset

# Interactive mode on local machine, this creates a tmux session named buddy-...
buddy
tmux attach -t buddy-xxxx # if you want to watch or co-work with buddy

# Operate a remote ssh host (in a tmux session on the host)
buddy --ssh user@hostname

# Operate a local container (in a tmux session in the container)
buddy --container my-dev-container

# One-shot mode
buddy exec "how much free disk space do I have?"

# Resume a prior session by ID (or last in this directory)
buddy resume f4e3-5bc3-a912-1f0d
buddy resume --last
```

### REPL slash commands

| Command | Description |
|---------|-------------|
| `/status` | Show current model, base URL, enabled tools, and session counters. |
| `/model [name\|index]` | Switch the active configured model profile (`/model` opens arrow-key picker). |
| `/login [name\|index]` | Show login health and start login flow for a configured profile. |
| `/context` | Show estimated context usage (`messages` estimate / context window) and token stats. |
| `/ps` | Show running background tasks with IDs and elapsed time. |
| `/kill <id>` | Cancel a running background task by task ID. |
| `/timeout <duration> [id]` | Set timeout for a background task (`id` optional only when one task exists). |
| `/approve ask|all|none|<duration>` | Configure shell approval policy for this REPL session. |
| `/session` | List saved sessions (ordered by last use). |
| `/session resume <session-id\|last>` | Resume a saved session by ID, or the most recently used one. |
| `/session new` | Create and switch to a fresh generated session ID. |
| `/help` | Show slash command help (only when no background tasks are running). |
| `/quit` `/exit` `/q` | Exit interactive mode (only when no background tasks are running). |


## Developers

**Test**

```bash
cargo test
```

Tests cover config parsing, API type serialization/deserialization, token estimation, and message constructors. All tests run offline (no network).

Live provider/model regressions are in an explicit ignored suite:

```bash
cargo test --test model_regression -- --ignored --nocapture
```

See [`docs/model-regression-tests.md`](docs/model-regression-tests.md) for setup and auth requirements.

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
use buddy::tools::{Tool, ToolContext};
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

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        Ok("result".into())
    }
}
```

Runtime/event embedding is available through the runtime actor API:

```bash
cargo run --example alternate_frontend -- "list files"
```

## Configuring

Configuration is loaded with this precedence (highest wins):

1. **CLI flags** — `--config`, `--model`, `--base-url`, `--container`, `--ssh`, `--tmux`, `--no-color`
2. **Environment variables** — `BUDDY_API_KEY`, `BUDDY_BASE_URL`, `BUDDY_MODEL`, `BUDDY_API_TIMEOUT_SECS`, `BUDDY_FETCH_TIMEOUT_SECS`
3. **Local config** — `./buddy.toml` in the current directory
4. **Global config** — `~/.config/buddy/buddy.toml` (create with `buddy init`; startup also auto-creates if missing)
5. **Built-in defaults**

Legacy compatibility:
- `AGENT_API_KEY`, `AGENT_BASE_URL`, and `AGENT_MODEL` are still accepted.
- `AGENT_API_TIMEOUT_SECS` and `AGENT_FETCH_TIMEOUT_SECS` are also accepted.
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
model = "gpt-5.3-codex-spark"

[models.openrouter-deepseek]
api_base_url = "https://openrouter.ai/api/v1"
api = "completions"
auth = "api-key"
api_key_env = "OPENROUTER_API_KEY"
model = "deepseek/deepseek-v3.2"

[models.openrouter-glm]
api_base_url = "https://openrouter.ai/api/v1"
api = "completions"
auth = "api-key"
api_key_env = "OPENROUTER_API_KEY"
model = "z-ai/glm-5"

[agent]
model = "gpt-codex"                         # active profile key from [models.<name>]
# system_prompt = "Optional additional operator instructions appended to the built-in template."
max_iterations = 20                         # safety cap on tool-use loops
# temperature = 0.7
# top_p = 1.0

[tools]
shell_enabled = true                       # run_shell tool
fetch_enabled = true                       # fetch_url tool
fetch_confirm = false                      # optional confirmation before fetch_url
fetch_allowed_domains = []                 # optional allowlist (empty = no allowlist)
fetch_blocked_domains = ["localhost"]      # optional denylist (exact + subdomain match)
files_enabled = true                       # read_file / write_file tools
files_allowed_paths = []                   # optional write_file allowlist roots
search_enabled = true                      # web_search tool (DuckDuckGo)
shell_confirm = true                       # prompt before running shell commands
shell_denylist = ["rm -rf /", "mkfs"]      # block dangerous run_shell patterns

[network]
api_timeout_secs = 120                     # model API request timeout (seconds)
fetch_timeout_secs = 20                    # fetch_url timeout (seconds)

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
  resume [SESSION_ID]     Resume saved session by id (or use --last)
  login [MODEL_PROFILE]   Login/check/reset provider auth for profile (defaults to [agent].model; shared per provider)
  help                    Print command help

Options:
  -c, --config <CONFIG>      Path to config file
  -m, --model <MODEL>        Override model profile key (if configured) or raw API model id
      --base-url <BASE_URL>  Override API base URL
      --container <ID/NAME>  Run shell/files tools with docker/podman exec in this container
      --ssh <USER@HOST>      Run shell/files tools on this host over persistent ssh
      --tmux [SESSION]       optional tmux session override (tmux-backed execution is the default when shell/files tools are enabled)
      --dangerously-auto-approve
                              in `buddy exec`, bypass run_shell confirmations (dangerous)
      --no-color             Disable color output
  -h, --help                 Print help
  -V, --version              Print version
```

At startup, the system prompt is rendered from one compiled template with runtime placeholders (enabled tools, execution target, and optional `[agent].system_prompt` operator instructions). When `--container` or `--ssh` is set, the rendered prompt includes explicit remote-target guidance.

`buddy exec` is non-interactive. If `tools.shell_confirm=true`, exec fails closed unless you explicitly pass `--dangerously-auto-approve`.

Login credentials are stored in `~/.config/buddy/auth.json` using machine-derived encryption-at-rest with per-record nonces. Use `buddy login --check` to inspect saved login health and `buddy login --reset` to clear saved provider credentials and re-authenticate.

### Built-in tools

| Tool | Description |
|------|-------------|
| `run_shell` | Execute shell commands. Output truncated to 4K chars. Optional user confirmation and denylist guardrails via `tools.shell_denylist`. `wait` can be `true` (default), `false` (tmux-backed targets; return immediately), or a timeout duration string like `10m`. Emits structured tool stream events (`started`, `stdout`, `stderr`, `completed`) to runtime consumers. Respects `--container`, `--ssh`, and `--tmux`. |
| `fetch_url` | HTTP GET a URL, return body as text. Truncated to 8K chars. Uses `[network].fetch_timeout_secs`. Blocks localhost/private/link-local targets by default, with optional tools-domain allow/deny policy. |
| `read_file` | Read a file's contents. Truncated to 8K chars. Respects `--container`, `--ssh`, and `--tmux`. |
| `write_file` | Write content to a file. Creates or overwrites. Respects `--container`, `--ssh`, and `--tmux`. Blocks sensitive directories by default and can be scoped with `tools.files_allowed_paths`. |
| `web_search` | Search DuckDuckGo and return top results with titles, URLs, and snippets. No API key needed. |
| `capture-pane` | Capture tmux pane output (with common `capture-pane` flags and optional delay) to inspect interactive/stuck terminal state. By default it uses tmux screenshot behavior (current visible pane content). |
| `send-keys` | Inject tmux keys/text into a pane (Ctrl-C/Ctrl-Z/Enter/arrows/literal text) for interactive control. |
| `time` | Return harness-recorded current wall-clock time in multiple common formats (epoch + UTC text formats). |

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
