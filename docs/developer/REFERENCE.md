# Reference

This page centralizes command, flag, model, theme, config, and tool reference material.

## CLI commands

- `buddy`: start interactive REPL mode.
- `buddy exec <prompt>`: run one prompt and exit.
- `buddy resume <session-id>`: resume a saved session.
- `buddy resume --last`: resume the last session in the current directory.
- `buddy init [--force]`: guided init flow for `~/.config/buddy/buddy.toml` (update existing config, overwrite with backup, or cancel).
- `buddy login [model] [--check] [--reset]`: login/check/reset provider credentials for a model profile.

Login soft-fail behavior:

- If the active profile uses `auth = "login"` and no saved credentials exist, startup/model switching stays available.
- Buddy surfaces a warning with exact recovery commands: `/login <profile>` or `buddy login <profile>`.

## Global flags

| Flag | Description |
|------|-------------|
| `-V`, `--version` | Print version/commit/build metadata. |
| `-c`, `--config <path>` | Use an explicit config file. |
| `-m`, `--model <profile-or-model-id>` | Override active model profile key or raw model id. |
| `--base-url <url>` | Override API base URL. |
| `--container <name>` | Execute shell/file tools inside a running container. |
| `--ssh <user@host>` | Execute shell/file tools over SSH. |
| `--tmux [session]` | Use tmux-backed execution; optional custom session name. |
| `--no-color` | Disable ANSI colors. |
| `--dangerously-auto-approve` | In `exec` mode, bypass shell approvals. |

## REPL slash commands

| Command | Description |
|---------|-------------|
| `/status` | Show current model, base URL, enabled tools, and session counters. |
| `/model [name\|index]` | Switch configured model profile (`/model` with no args opens picker). |
| `/theme [name\|index]` | Switch terminal theme (`/theme` with no args opens picker), persist config, and render preview blocks. |
| `/login [name\|index]` | Check/start login flow for a configured profile. |
| `/context` | Show estimated context usage and token stats. |
| `/compact` | Summarize and trim older turns to reclaim context budget. |
| `/ps` | Show running background tasks with IDs and elapsed time. |
| `/kill <id>` | Cancel a running background task by ID. |
| `/timeout <duration> [id]` | Set timeout for a background task. |
| `/approve ask|all|none|<duration>` | Configure shell approval policy for this REPL session. |
| `/session` | List saved sessions ordered by last use. |
| `/session resume <session-id\|last>` | Resume a session by ID or most recent. |
| `/session new` | Create and switch to a new generated session ID. |
| `/help` | Show slash command help (only when no tasks are running). |
| `/quit` `/exit` `/q` | Exit interactive mode (only when no tasks are running). |

## Model profiles

Default bundled profiles in `src/templates/buddy.toml`:

- `gpt-codex` => `gpt-5.3-codex` (OpenAI, `api = "responses"`, `auth = "login"`)
- `gpt-spark` => `gpt-5.3-codex-spark` (OpenAI, `api = "responses"`, `auth = "login"`)
- `openrouter-deepseek` => `deepseek/deepseek-v3.2` (OpenRouter, `api = "completions"`, `auth = "api-key"`)
- `openrouter-glm` => `z-ai/glm-5` (OpenRouter, `api = "completions"`, `auth = "api-key"`)
- `kimi` => `kimi-k2.5` (Moonshot, `api = "completions"`, `auth = "api-key"`)

Context-limit defaults come from `src/templates/models.toml` (compiled into the binary), with fallback `8192` for unknown models.

## Theme reference

Built-in themes:

- `dark` (default)
- `light`

Custom themes are defined under `[themes.<name>]` and can override semantic tokens.
The complete token key list is defined in `src/ui/theme/mod.rs` (`ThemeToken::key()`).

Commonly overridden tokens:

- `warning`
- `error`
- `block_tool_bg`
- `block_reasoning_bg`
- `block_approval_bg`
- `block_assistant_bg`
- `markdown_heading`
- `markdown_quote`
- `markdown_code`
- `risk_low`
- `risk_medium`
- `risk_high`

## Config loading and precedence

Highest precedence wins:

1. CLI flags (`--config`, `--model`, `--base-url`, `--container`, `--ssh`, `--tmux`, `--no-color`, `--dangerously-auto-approve`)
2. Environment variables (`BUDDY_API_KEY`, `BUDDY_BASE_URL`, `BUDDY_MODEL`, `BUDDY_API_TIMEOUT_SECS`, `BUDDY_FETCH_TIMEOUT_SECS`)
3. Local config (`./buddy.toml`)
4. Global config (`~/.config/buddy/buddy.toml`)
5. Built-in defaults

First-run bootstrap:

- When no local or global config exists, `buddy` automatically starts the guided init flow.
- In non-interactive terminals, buddy creates the default global config and continues with defaults.

Legacy compatibility:

- `AGENT_*` env vars are still accepted.
- `agent.toml` legacy config filenames are still accepted.
- `.agentx` legacy session/auth paths are still accepted with warnings.
- See [docs/developer/deprecations.md](deprecations.md) for removal timelines.

## Full config reference

```toml
[models.gpt-codex]
api_base_url = "https://api.openai.com/v1"
api = "responses"                           # responses | completions
auth = "login"                              # login | api-key
# Only one may be set: api_key, api_key_env, api_key_file.
# api_key_env = "OPENAI_API_KEY"
# api_key = "sk-..."
# api_key_file = "/path/to/key.txt"
model = "gpt-5.3-codex"
# context_limit = 128000

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

[models.kimi]
api_base_url = "https://api.moonshot.ai/v1"
api = "completions"
auth = "api-key"
api_key_env = "MOONSHOT_API_KEY"
model = "kimi-k2.5"

[agent]
name = "agent-mo"                           # tmux session prefix: buddy-<name>
model = "gpt-spark"                         # active profile key from [models.<name>]
# system_prompt = "Optional additional operator instructions."
max_iterations = 20
# temperature = 0.7
# top_p = 1.0

[tools]
shell_enabled = true
fetch_enabled = true
fetch_confirm = false
fetch_allowed_domains = []
fetch_blocked_domains = ["localhost"]
files_enabled = true
files_allowed_paths = []
search_enabled = true
shell_confirm = true
shell_denylist = ["rm -rf /", "mkfs"]

[network]
api_timeout_secs = 120
fetch_timeout_secs = 20

[display]
color = true
theme = "dark"
show_tokens = false
show_tool_calls = true
persist_history = true

# Optional custom theme overrides:
# [themes.my-theme]
# warning = "#ffb454"
# block_assistant_bg = "#13302a"

[tmux]
max_sessions = 1
max_panes = 5
```

## Built-in tools

| Tool | Description |
|------|-------------|
| `run_shell` | Execute shell commands (4K truncation). Requires `risk`, `mutation`, `privesc`, and `why`. Supports optional tmux `session`/`pane` selectors. |
| `fetch_url` | HTTP GET and return text (8K truncation). Uses `[network].fetch_timeout_secs` and host safety policy. |
| `read_file` | Read files (8K truncation). Respects local/container/ssh execution context. |
| `write_file` | Create/overwrite files with path safety policies and optional allowlist roots. |
| `web_search` | DuckDuckGo search and return top results. |
| `capture-pane` | Capture tmux pane output (optionally delayed) for terminal-state inspection. |
| `send-keys` | Send keys/text to tmux panes for interactive control. Requires `risk`, `mutation`, `privesc`, and `why`. |
| `time` | Return harness-recorded wall clock time in multiple formats. |

All tool responses return a JSON envelope with `result` and `harness_timestamp`.

## Provider examples

OpenAI (API key):

```bash
export BUDDY_API_KEY="sk-..."
buddy
```

OpenAI (login auth profile):

```bash
buddy login gpt-codex
buddy
```

Ollama:

```bash
ollama serve
export BUDDY_BASE_URL="http://localhost:11434/v1"
export BUDDY_MODEL="llama3.2"
buddy
```

OpenRouter:

```bash
export BUDDY_BASE_URL="https://openrouter.ai/api/v1"
export BUDDY_API_KEY="sk-or-..."
export BUDDY_MODEL="anthropic/claude-3.5-sonnet"
buddy
```

## External protocol references

- [OpenAI Chat Completions API](https://platform.openai.com/docs/api-reference/chat)
- [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses/create)
- [OpenAI Function Calling guide](https://platform.openai.com/docs/guides/function-calling)
- [Ollama OpenAI compatibility](https://ollama.ai/blog/openai-compatibility)
- [OpenRouter docs](https://openrouter.ai/docs)
- [OpenRouter Models API](https://openrouter.ai/docs/api-reference/list-available-models)
