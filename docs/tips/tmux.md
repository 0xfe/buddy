# tmux Tips for AI Agents

## Quick checks
```bash
tmux ls
tmux list-windows -t <session>
tmux list-panes -a -F '#S:#I.#P #{pane_current_command} #{pane_title}'
```

## Attach/detach safely
```bash
tmux attach -t <session>
# detach: Ctrl-b d
```

## Capture output without attaching
```bash
tmux capture-pane -p -t <session>:<window>.<pane> | tail -n 200
```

## Send commands to a pane
```bash
tmux send-keys -t <session>:<window>.<pane> 'pwd' Enter
```

## Practical conventions
- Prefer querying/capturing pane state before sending keys.
- Keep automated `send-keys` explicit and reversible.
- Avoid running buddy inside the same managed pane/window it controls.
- Legacy managed window names may still exist (`buddy-shared`); newer setups use `shared`.

## Common recovery
```bash
# stale pane/session discovery
tmux ls || true

# create a clean scratch session
tmux new-session -d -s scratch
```
