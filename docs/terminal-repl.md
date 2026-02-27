# The Terminal REPL

The interactive mode is a full-featured terminal UI built on top of `crossterm`.
It gives the user a readline-style editor, real-time status feedback, background
task management, and inline command approval — all while keeping background
agent work running concurrently.

---

## High-Level Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                      main.rs (REPL loop)                     │
│                                                              │
│  ┌────────────────────┐    ┌───────────────────────────────┐ │
│  │   read_repl_line   │    │   Background task pool        │ │
│  │   with_interrupt() │    │                               │ │
│  │                    │    │  task 1: agent.send(prompt_a) │ │
│  │  keyboard events   │    │  task 2: agent.send(prompt_b) │ │
│  │  slash commands    │◄───│                               │ │
│  │  approval prompts  │    │  events ──► mpsc channel      │ │
│  └────────────────────┘    └───────────────────────────────┘ │
│           │                                                   │
│           ▼                                                   │
│  ┌──────────────────────────────────────────────────────┐    │
│  │                   Renderer                           │    │
│  │  stdout ← final assistant responses                 │    │
│  │  stderr ← tool calls, reasoning, tokens, spinners  │    │
│  └──────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────┘
```

The REPL loop and the agent tasks are deliberately separated:

- The **REPL loop** owns the terminal and all input/output; it runs in the
  main async task.
- **Agent tasks** run as background Tokio tasks. They communicate with the REPL
  loop exclusively through channels.
- **stdout** carries final assistant responses (clean for piping).
- **stderr** carries all status chrome: tool calls, reasoning traces, spinners,
  token counts.

---

## The Read Loop — `src/tui/input.rs`

`read_repl_line_with_interrupt` is the single entry point for reading user
input. It has two modes:

**Interactive mode** (when stdin/stderr are both TTYs): enters raw mode,
handles keystrokes one at a time, and renders a live editor surface.

**Fallback mode** (pipes, redirects, CI): writes the prompt to stderr and reads
a plain line from stdin via `read_line`.

### Keyboard Shortcuts (interactive mode)

| Key | Action |
|-----|--------|
| `Enter` | Submit the current input |
| `Alt+Enter` | Insert a newline (multiline mode) |
| `Ctrl-A` / `Home` | Move cursor to line start |
| `Ctrl-E` / `End` | Move cursor to line end |
| `Ctrl-B` / `←` | Move cursor left one character |
| `Ctrl-F` / `→` | Move cursor right one character |
| `Ctrl-W` | Delete word before cursor |
| `Ctrl-K` | Delete from cursor to end of line |
| `Ctrl-U` | Delete from cursor to start of line |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character at cursor |
| `↑` / `Ctrl-P` | Previous history entry |
| `↓` / `Ctrl-N` | Next history entry |
| `Tab` | Cycle through slash command autocomplete suggestions |
| `Ctrl-C` | Cancel / clear current input |
| `Ctrl-D` | EOF (exit if buffer is empty) |

### Autocomplete

When the input buffer starts with `/`, the editor shows a list of matching
slash commands below the input line:

```
> /se
  · /session   Session ops: list, resume, create.
  ▶ /status    Show model, endpoint, tools, and session details.
```

`Tab` cycles through suggestions. The selected suggestion is highlighted with
`▶`; others use `·`. Pressing `Tab` again wraps around. Typing any non-Tab
character applies the selection and exits autocomplete mode.

Suggestions are filtered by prefix and capped at 6 entries.

### Poll Callback

`read_repl_line_with_interrupt` accepts a polling closure that the read loop
calls every 80 ms while waiting for keyboard events. The closure returns a
`ReadPoll`:

```rust
pub struct ReadPoll {
    pub interrupt: bool,       // if true, abort the current read
    pub status_line: Option<String>,  // live status above the prompt
}
```

The main REPL loop uses this to:

1. Render a live liveness line above the prompt showing task count, spinner,
   and elapsed time.
2. Interrupt the editor when an approval request arrives from a background task.

`ReadOutcome::Interrupted` is returned when `interrupt = true`, causing the
REPL to re-render the approval prompt instead of resuming normal input.

### ReplState

Editor state persists between calls in `ReplState`:

```rust
pub struct ReplState {
    history: Vec<String>,    // submitted inputs
    // cursor position, draft text, autocomplete selection, ...
}
```

Input history is in-memory only; it is not written to disk.

---

## Prompt Rendering — `src/tui/prompt.rs`

### Normal mode

```
> your input here
```

When connected to a remote SSH target:

```
(ssh user@host)> your input here
```

For multiline input, subsequent lines use a continuation prompt:

```
> first line
...... second line
...... third line
```

### Approval mode

When a background task issues a shell confirmation request, the editor surface
is replaced with an inline approval prompt:

```
local$ rm -rf /tmp/work -- approve? [y/N]
```

The `<actor>` prefix is color-coded and changes based on the execution target:

| Target | Actor prefix |
|--------|-------------|
| Local | `local` |
| SSH | `ssh user@host` |
| Container | `container:name` |

The user types `y` or `yes` to approve, or leaves blank / types `n` to deny.
The tool's `ShellApprovalRequest` oneshot channel is resolved immediately.

---

## Slash Commands — `src/tui/commands.rs`

Slash commands are parsed from any input line that starts with `/`. They
are processed by the REPL loop before being sent to the agent.

| Command | Description |
|---------|-------------|
| `/status` | Show model name, base URL, enabled tools, and session token counts |
| `/model <name\|index>` | Switch active configured model profile |
| `/models` | List configured model profiles and pick one |
| `/login [name\|index]` | Start login flow for a profile (opens browser when available) |
| `/context` | Show estimated context window fill % and message counts |
| `/ps` | List all running background tasks with IDs and elapsed time |
| `/kill <id>` | Cooperatively cancel a background task |
| `/timeout <dur> [id]` | Set a deadline for one or all tasks (`30s`, `10m`, `1h`, `2d`) |
| `/approve ask\|all\|none\|<dur>` | Change the shell approval policy |
| `/session` | List all saved sessions |
| `/session resume <name\|last>` | Restore a saved session into the agent |
| `/session new <name>` | Start a fresh session with a new name |
| `/help` | Print all slash commands with descriptions |
| `/quit`, `/exit`, `/q` | Exit interactive mode |

Commands blocked while tasks are running: `/help`, `/quit`, `/exit`, `/q`, `/model`, `/models`, `/login`, `/session`.
The REPL prints a message asking the user to `/kill` tasks first.

### Approval Policy

`/approve` controls whether `run_shell` commands require user confirmation:

| Policy | Behaviour |
|--------|-----------|
| `ask` | Prompt for every shell command (default when `shell_confirm=true`) |
| `all` | Auto-approve all commands for this session |
| `none` | Auto-deny all commands for this session |
| `30s`, `5m`, ... | Auto-approve for a duration, then revert to `ask` |

---

## Background Task Management

Every user prompt is spawned as a numbered background task:

```rust
let task_id: u64 = next_task_id();
tokio::spawn(async move {
    agent.set_live_output_suppressed(true);
    agent.set_live_output_sink(Some((task_id, event_tx)));
    agent.set_cancellation_receiver(Some(cancel_rx));
    let result = agent.send(input).await;
    // send final result via channel
});
```

The REPL loop runs a tight poll (every 80 ms) processing events from all tasks:

```
AgentUiEvent::ToolCall       → render tool call snippet to stderr
AgentUiEvent::ToolResult     → render result snippet to stderr
AgentUiEvent::ReasoningTrace → render reasoning block to stderr
AgentUiEvent::TokenUsage     → render token line to stderr
AgentUiEvent::Warning        → render warning to stderr
```

Final responses arrive on a separate `oneshot` channel and are printed to
stdout.

### Liveness Line

While any task is running, a status line is rendered above the prompt on each
poll tick:

```
⣾ [1] 12s  buddy working  |  [2] 5s  waiting approval
```

This uses a four-frame braille spinner (`|`, `/`, `-`, `\`) and shows elapsed
time per task.

### Cancellation

`/kill <id>` signals the corresponding `watch::Sender<bool>` to `true`. The
agent's loop checks this signal at the API call and each tool execution
(via `tokio::select!`). Cancellation is cooperative — the task completes its
current atomic operation before stopping.

### Timeouts

`/timeout <dur> [id]` schedules a deadline for a task. When elapsed, the REPL
automatically triggers cancellation as if `/kill` was called. Duration formats:
`500ms`, `30s`, `10m`, `2h`, `1d`.

---

## Rendering — `src/tui/renderer.rs`

The `Renderer` struct centralises all terminal output. The key design is:

- **Stdout** = assistant responses (clean Markdown text)
- **Stderr** = all status chrome (spinners, tool calls, tokens, warnings)

This lets one-shot mode users pipe the assistant's answer cleanly:

```bash
buddy exec "summarise this file" < input.txt | pandoc -o output.pdf
```

### Block Rendering

Status output is rendered in styled rectangular blocks. Each block has a tone:

| Tone | Background | Use |
|------|-----------|-----|
| Tool | Dark green | Tool call inputs/outputs |
| Reasoning | Blue-grey | Model reasoning traces |
| Approval | Dark red | Shell confirmation context |
| Assistant | Green | Final assistant response |

Block layout:

```
  ▶ run_shell                          ← tool name (yellow)
  du -sh /var                          ← args (grey) padded to terminal width
```

```
  ← run_shell                          ← result glyph
  exit code: 0                         ← result content, clipped at edge
  stdout:
  512M    /var
```

Blocks are padded to the terminal width minus a 2-character right margin, and
indented with a 2-space left margin.

### Snippet Previews

Shell output and file reads are shown as a preview (first 10 lines), with a
`...N more lines...` marker if truncated. File reads get syntax highlighting
via `syntect` based on the file extension.

### Markdown Rendering

Final assistant responses are parsed and rendered with `pulldown-cmark`.
Headings, bold, italic, inline code, code fences, bullet lists, and
blockquotes are all styled. Code fence blocks get syntax highlighting via
`syntect` when the language tag matches a known grammar.

### Spacing State Machine

A global `StreamSpacingState` tracks whether the last line written to stdout
or stderr was blank. This prevents stacked blank lines between consecutive
blocks.

### Progress Spinners — `src/tui/progress.rs`

Spinners are RAII: `ProgressHandle` clears the line on drop. They are TTY-only
and update in place using `\r\x1b[2K` to clear and rewrite. A 100 ms tick
cycles through `|`, `/`, `-`, `\`.

In background task mode, spinners are globally disabled by calling
`set_progress_enabled(false)` so they don't interfere with the REPL's own
status line rendering.

---

## Session Persistence — `src/session.rs`

Sessions are saved as JSON files under `.buddyx/sessions/`. Each file contains:

```json
{
  "version": 1,
  "name": "default",
  "updated_at_millis": 1708900000000,
  "state": {
    "messages": [...],
    "tracker": {
      "context_limit": 128000,
      "total_prompt_tokens": 4200,
      "total_completion_tokens": 800,
      ...
    }
  }
}
```

**Auto-save:** After each completed prompt, the REPL calls
`session_store.save("default", &agent.snapshot_session())`.

**Atomic writes:** Sessions are written to a `.json.tmp` file and then renamed
into place to prevent corruption on crash.

**Listing:** `SessionSummary` entries are sorted by `updated_at_millis`
descending, so `/session resume last` always resumes the most recently
active session.

**Session name validation:** Names must be non-empty and contain only
alphanumeric characters, hyphens, and underscores.

---

## Text Utilities — `src/tui/text.rs`

| Function | Purpose |
|----------|---------|
| `visible_width(s)` | Character width excluding ANSI escape codes |
| `truncate_single_line(s, w)` | Clip a single line to `w` visible chars |
| `wrap_for_block(s, w, indent)` | Word-wrap text for a styled block |
| `snippet_preview(s, n)` | First N lines with `...M more lines...` |
| `clip_to_width(s, w)` | Hard clip to terminal width |

These handle ANSI codes correctly so that colorised output doesn't break width
calculations.

---

## Module Map

```
src/tui/
├── mod.rs            Public re-exports
├── commands.rs       Slash command registry and parser
├── highlight.rs      syntect syntax highlighting wrapper
├── input.rs          read_repl_line_with_interrupt + ReadOutcome
├── input_buffer.rs   Edit primitives: cursor movement, history, word ops
├── input_layout.rs   Terminal column/row wrapping math
├── markdown.rs       pulldown-cmark → terminal rendering
├── progress.rs       Spinner + RAII ProgressHandle
├── prompt.rs         Prompt text rendering (normal and approval modes)
├── renderer.rs       All terminal output: blocks, snippets, markdown
├── settings.rs       Hardcoded constants: colors, glyphs, margins
└── text.rs           ANSI-aware truncation and wrapping helpers
```
