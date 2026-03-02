
- Update regression test to test all the features we use, and work with the tmux testbed.
- The markdown renderer does not really render some of the titles well -- see if you can make the titels bold, and use terminal support for italics etc. If not, render just the titles as plain text with the "#" prefixes, so we know which ones are titles.
- Some models have built-in tools for things like python, websearch, etc. Make sure we're sending the right system and tool prompts so the models are being fully utilized correctly. Search the relevant documentation for the model for the precise details. Start with the OpenAI models. Then add support for the Claude Sonnet and Haiku models to buddy.toml and make sure we have full support for its APIs and tools. Do not support auth=login for Claude models.

--------

- When calling buddy login I see all this:

```
➜   ~/.local/bin/buddy login gpt-spark
• login health
  provider: openai
  saved_credentials: no

• login
  profile: gpt-spark
  url: https://auth.openai.com/codex/device
  code: 9YHW-1RCJC
  browser: opened

[|] waiting for authorization (0.0s)Opening in existing browser session.
• login successful
  profile: gpt-spark
  provider: openai
```

If already logged in, it should just say so, and mention that /logout can be used to log out (add tthat command.)

For the login flow, it should simply look like this:

```
• logging you into openai via https://auth.openai.com/codex/device
  device code: 9YHW-1RCJC
  (open your browser and go to the url above, and enter the device code)
```

Also, one does not login to a model, they login to a provider. So rationalize the CLI and config around that. I'd like to type in "buddy login openai" or "buddy login kimi" etc.

----------

- Look for duplicated code across ssh/container/local code paths and give me a summary.


Multi-host plan:
 - Ability to ssh to and manage conversations across multiple hosts
 - should be able to dynamically ssh to hosts or connect to containers
 - max-hosts, allowlists/denylists

Tool scripts:
 - Multiple tool calls in a single script

Parallelism:
 - Models can request multiple tool calls/scripts to be run in parallel
 - Models can request


-- DONE

 Docs:

- Refactor DESIGN.md - keep it high level, and move details into separate documents in docs/
- Create a docs/tips directory, with short documents useful to AI agents working on this codebase, say tmux tips, or shell tips, or testing tips, etc. These are just examples. These can be referred to in ai-state.md and AGENTS.md.
- Review docs/ (except docs/plans/*) and make sure all docs are current and upto date.
- Clean up ai-state.md -- it's too big and has a lot of old unecessary cruft, keep it focused on what an AI agent needs to know to get working on this codebase quickly. Move relevant bits to docs/tips if needed.


Next plan:

- Implement a themable color system, with a default dark theme, and a light theme. It should have a small palette of colors, and allow for customizing the colors. All colors used by buddy should come from that palette only. Search online for terminal color palette ideas -- typically solarized dark and light work pretty well. Implement this as a library, and try and use generic names for colors (e.g., error_bg, error_fg, code_bg, etc.), and try to make it composable. As part of this, also build a simple buddy theme explorer that allows you to try out different themes -- it shows you an example buddy REPL with thinking, code, apporval, output and other blocks, and allows you to switch themes and see the effect. Also add support for /theme to select a theme.

- Add support for versioning, build date, commit hash etc., and build them into the binary. Have the CLI display them on start. The Makefiles should now be our first class build system (it calls cargo build, cargo test, etc.) Create some make targets for building releases, bumping versions, etc. Plumb this into github actions so we're automatically building releases when we push a new release tag.

- Improve buddy init:
    - make it a pretty TUI Q/A flow asking to select model, login, etc.
    - should prompt for things like overwriting configs etc.
    - should be able to read current config and allow updating it
    - should be called on first "buddy" command

- Add support for packaging and distribution, curl style install. It should run on most mac and linux distros, install into ~/.local/bin, and call buddy init to initialize the config.

- If using auth=login mode, and the user is not logged in, don't exit buddy with error. Instead, just mention that the user needs to login using the /login slash command.

- Make it possible for the model to use tmux freely. First class tmux tools, create session, create pane, send keys, capture pane, kill pane, kill session etc. buddy can only manage sessions and panes that it created. All sessions and panes must be prefixed using our buddy/name system. There must be good logging and approvals for session and pane management. Update the shell, capture pane, send keys, tools so that the model can provide an optional tmux session/pane name to perform the action on. We still always have our default shared pane, which should be used if no session/pane is specified. In the systemprompt where we show the snapshot of the default pane, make it excruciatingly clear that it's the default pane. If capture-pane is used on a different pane, then don't show the default pane in the system prompt. There must be a configurable max-sessions and max-panes limit, defaulting to 1 session and 5 panes (includes the default session/pane.)
