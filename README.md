# buddy -- a sysadmin AI assistant

A terminal AI agent written in Rust with native `tmux` support. It is designed for sysadmin workflows across local hosts, remote SSH targets, and local containers.

Buddy works with OpenAI-compatible APIs and supports both `/v1/chat/completions` and `/v1/responses`.

## Pre-requisites

- **Rust** 1.70+ (install via [rustup](https://rustup.rs/))
- `tmux` on the machine(s) where buddy runs commands
- An OpenAI-compatible provider endpoint (OpenAI, OpenRouter, Moonshot, Ollama, LM Studio, vLLM, etc.)

## Models

The default config template (`src/templates/buddy.toml`) includes tested profiles:

- `gpt-codex` (`gpt-5.3-codex`, OpenAI Responses API)
- `gpt-spark` (`gpt-5.3-codex-spark`, OpenAI Responses API)
- `openrouter-deepseek` (`deepseek/deepseek-v3.2`, OpenRouter Completions API)
- `openrouter-glm` (`z-ai/glm-5`, OpenRouter Completions API)
- `kimi` (`kimi-k2.5`, Moonshot Completions API)

## Quickstart

```bash
# Option A: install from release (curl-style)
curl -fsSL https://raw.githubusercontent.com/0xfe/buddy/main/scripts/install.sh | bash

# Option B: build from source and install
make build
make install

# First run auto-starts guided init when no config exists.
# You can run init again later to update/overwrite settings.
buddy
buddy init
$EDITOR ~/.config/buddy/buddy.toml

# Optional: login flow for auth = "login" profiles
buddy login

# Start buddy operating on the local host (and connect to the tmux session)
buddy
tmux attach -t buddy # on a separate terminal

# Operate a remote host (in tmux on the remote host)
buddy --ssh user@host

# Operate a docker container
buddy --container my-container

# Other handy commands
buddy exec <prompt>
buddy resume <session-id>
buddy resume --last
```

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

## Configuration

See config reference in [docs/developer/REFERENCE.md](docs/developer/REFERENCE.md).

## Login auth behavior

- Profiles using `auth = "login"` no longer hard-fail startup when credentials are missing.
- Buddy starts normally and shows guidance to run `/login <profile>` (or `buddy login <profile>`).
- Requests on that profile still require valid saved login credentials.

## Build and release

Keep `make` as the primary workflow:

```bash
make check
make release-artifacts
```

Install/distribution details are in [docs/developer/install.md](docs/developer/install.md).  
Full build, test, versioning, and release CI details are in [docs/developer/BUILD.md](docs/developer/BUILD.md).

## Documentation map

- [docs/design/DESIGN.md](docs/design/DESIGN.md): high-level architecture and current feature inventory.
- [docs/developer/REFERENCE.md](docs/developer/REFERENCE.md): CLI flags/commands, REPL slash commands, config, models, themes, and tool references.
- [docs/developer/BUILD.md](docs/developer/BUILD.md): build/test commands, release process, build metadata, and GitHub Actions release flow.
- [docs/developer/install.md](docs/developer/install.md): curl installer, offline install mode, and troubleshooting.
- [docs/developer/DEVELOPER.md](docs/developer/DEVELOPER.md): library embedding, custom tools, extension points, and developer integration notes.
- [docs/design/](docs/design): detailed design breakdown (feature catalog, module map, runtime/protocols, tools/execution).
- [docs/design/architecture.md](docs/design/architecture.md): module boundaries and extension points.
- [docs/design/tools.md](docs/design/tools.md): tool schemas, guardrails, and runtime behavior.
- [docs/design/remote-execution.md](docs/design/remote-execution.md): local/container/ssh/tmux execution model.
- [docs/design/terminal-repl.md](docs/design/terminal-repl.md): REPL input/rendering/runtime UX details.
- [docs/developer/testing-ui.md](docs/developer/testing-ui.md): tmux-based opt-in UI regression harness.
- [docs/developer/model-regression-tests.md](docs/developer/model-regression-tests.md): live provider regression suite.
- [docs/tips/](docs/tips): short tactical notes for contributors and AI agents.
