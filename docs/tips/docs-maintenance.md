# Docs Maintenance Tips

## Source of truth split
- `docs/DESIGN.md`: complete feature behavior inventory.
- `README.md`: user-facing overview and quickstart.
- `docs/`: architecture/usage/reference docs.
- `docs/tips/`: tactical, practical AI-agent notes.
- `ai-state.md`: compact onboarding cache for next AI run.

## ai-state maintenance pattern
- Keep it short and high-signal.
- Replace stale sections; do not append historical timelines.
- Link out to stable docs instead of duplicating detailed explanations.

## Updating docs with code changes
1. Update behavior docs in the same change as code.
2. Update `docs/DESIGN.md` features section when behavior/flags/tools change.
3. Add a tip doc when you uncover non-obvious workflow knowledge.
4. Keep examples executable and paths current.

## Pre-commit doc hygiene checks
```bash
rg -n "TODO|TBD|FIXME" AGENTS.md ai-state.md docs/tips
rg -n "docs/tips" AGENTS.md ai-state.md
```
