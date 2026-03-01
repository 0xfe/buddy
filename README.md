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
# Build and install
make build
make install

# Initialize config under ~/.config/buddy
buddy init
$EDITOR ~/.config/buddy/buddy.toml

# Optional: login flow for auth = "login" profiles
buddy login

# Start REPL (tmux-backed by default)
buddy
```

## Build and release

Keep `make` as the primary workflow:

```bash
make check
make release-artifacts
```

Full build, test, versioning, and release CI details are in [docs/BUILD.md](docs/BUILD.md).

## Documentation map

- [docs/DESIGN.md](docs/DESIGN.md): high-level architecture and current feature inventory.
- [docs/REFERENCE.md](docs/REFERENCE.md): CLI flags/commands, REPL slash commands, config, models, themes, and tool references.
- [docs/BUILD.md](docs/BUILD.md): build/test commands, release process, build metadata, and GitHub Actions release flow.
- [docs/DEVELOPER.md](docs/DEVELOPER.md): library embedding, custom tools, extension points, and developer integration notes.
- [docs/design/](docs/design): detailed design breakdown (feature catalog, module map, runtime/protocols, tools/execution).
- [docs/architecture.md](docs/architecture.md): module boundaries and extension points.
- [docs/tools.md](docs/tools.md): tool schemas, guardrails, and runtime behavior.
- [docs/remote-execution.md](docs/remote-execution.md): local/container/ssh/tmux execution model.
- [docs/terminal-repl.md](docs/terminal-repl.md): REPL input/rendering/runtime UX details.
- [docs/testing-ui.md](docs/testing-ui.md): tmux-based opt-in UI regression harness.
- [docs/model-regression-tests.md](docs/model-regression-tests.md): live provider regression suite.
- [docs/tips/](docs/tips): short tactical notes for contributors and AI agents.
