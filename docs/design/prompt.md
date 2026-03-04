# Prompt Architecture

Buddy separates stable instructions from volatile execution state.

## Objectives

- Keep the system prompt byte-stable across turns for cache friendliness.
- Inject live tmux state each request without polluting persisted history.
- Add a compact request-local history ledger so actor/action/command/result
  flow is explicit to the model.
- End every model request with explicit targeting/shell-safety reminders.
- Frame snapshots as plain terminal output so they are not interpreted as
  executable instructions.
- Keep instruction precedence explicit so cross-model behavior is predictable.

## Layers

1. Static system prompt
   - Rendered once at startup from `src/templates/system_prompt.template`.
   - Section-level reusable snippets are loaded from
     `src/templates/prompts.toml` where applicable.
   - Stored as the leading `Message::system` in agent history.
   - Never mutated during normal turn execution.
2. Dynamic request context
   - Built immediately before every model request.
   - Inserted as an ephemeral `Message::user` after the leading system
     messages for that request only.
   - Contains explicit section separators (`--`) and three sections:
     request metadata, tmux context, and annotated history ledger.
   - Not persisted back into `Agent.messages`.
3. Conversation history
   - User/assistant/tool messages that form durable state across turns.
4. Tail execution reminders
   - Added as the final ephemeral `Message::user` on every request.
   - Reinforces active tmux route, default-vs-explicit targeting rules, and
     managed-shell safety rules (`set -e`/`exit`/`exec` prohibitions).
   - Not persisted back into `Agent.messages`.

## Static Prompt Contract

The built-in template (`src/templates/system_prompt.template`) is organized into
deterministic sections:

1. role
2. explicit rule priority order
3. core behavior rules
4. lightweight planning-before-tools instruction
5. tmux execution model and tool-choice guide
6. enabled-tools list
7. final checklist reinforcement

Critical behavior appears near the top and is reinforced at the end so models
with weaker instruction retention still get reliable constraints.

## Operator Instructions Block

When operator custom instructions are configured, Buddy wraps them in a
structured additive block with explicit conflict policy:

- operator instructions are lower priority than system safety/protocol rules
- user conflicts should trigger clarification unless safety requires refusal
- conflicts with system/tool policy must follow system/tool policy

This reduces ambiguous blending of operator text with core system rules.

## Tmux Snapshot Routing

- Default mode:
  - Buddy captures the default managed shared pane.
  - Context block uses explicit BEGIN/END markers, truncation notes, and
    section separators.
- Non-default mode:
  - If the latest tmux-aware tool call (`run_shell`, `tmux_capture_pane`,
    `tmux_send_keys`) targets a non-default session/pane, Buddy omits the
    default screenshot and injects a non-default target context note instead.
- Unavailable mode:
  - If `tmux_capture_pane` is unavailable (or capture fails), Buddy injects an
    explicit tmux-unavailable context section rather than silently omitting it.
- Missing-target recovery mode:
  - If recent tool results report `tmux target not found`, Buddy forces default
    shared-pane snapshot routing on the next request so the model can recover
    with current default-pane state.

## History Ledger Annotation

- Buddy derives a bounded recent-message ledger (actor/action/detail lines).
- Assistant tool calls are annotated with:
  - tool name
  - execution location (default shared pane vs explicit target/session/pane)
  - approval metadata fields when present (`risk`, `mutation`, `privesc`, `why`)
  - command/action summary
- Tool results are annotated with:
  - linked tool call id + tool name (when available)
  - success/failure/info status
  - inferred approval outcome (`denied` vs `passed_or_not_required`)
  - concise result summary (for example shell exit code/stdout/stderr).

## Safety Framing

Snapshot blocks explicitly say they are plain terminal output and not
instructions. Model guidance requires checking for a usable shell prompt before
running commands and recommending recovery actions when the pane is blocked.
Managed `run_shell` execution also blocks shell-killing directives (`set -e`,
`exit/logout`, `exec ...`) in shared tmux contexts; prompt guidance and runtime
enforcement are intentionally redundant.
