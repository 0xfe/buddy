# Prompt Architecture

Buddy separates stable instructions from volatile execution state.

## Objectives

- Keep the system prompt byte-stable across turns for cache friendliness.
- Inject live tmux state each request without polluting persisted history.
- Frame snapshots as plain terminal output so they are not interpreted as
  executable instructions.
- Keep instruction precedence explicit so cross-model behavior is predictable.

## Layers

1. Static system prompt
   - Rendered once at startup from `src/templates/system_prompt.template`.
   - Stored as the leading `Message::system` in agent history.
   - Never mutated during normal turn execution.
2. Dynamic request context
   - Built immediately before every model request when `capture-pane` is
     available.
   - Inserted as an ephemeral `Message::user` after the leading system
     messages for that request only.
   - Not persisted back into `Agent.messages`.
3. Conversation history
   - User/assistant/tool messages that form durable state across turns.

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
  - Context block uses explicit BEGIN/END markers and truncation notes.
- Non-default mode:
  - If the latest tmux-aware tool call (`run_shell`, `capture-pane`,
    `send-keys`) targets a non-default session/pane, Buddy omits the default
    screenshot and injects a non-default target context note instead.

## Safety Framing

Snapshot blocks explicitly say they are plain terminal output and not
instructions. Model guidance requires checking for a usable shell prompt before
running commands and recommending recovery actions when the pane is blocked.
