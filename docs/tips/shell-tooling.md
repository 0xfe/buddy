# Shell and Tooling Tips

## Fast repo navigation
- Use `rg --files` for file discovery and `rg <pattern>` for text search.
- Use `sed -n 'start,endp' <file>` for bounded reads.
- Keep commands small and composable; avoid giant pipelines when debugging.

## Safe git workflow in shared trees
```bash
git status --short
git diff -- <path>
git add <scoped-files>
git commit -m "short summary" -m "details"
```
- Stage only files relevant to your task.
- Never revert unrelated dirty files unless explicitly asked.
- Prefer non-interactive git commands.

## Useful one-liners
```bash
# count lines in candidate files
wc -l AGENTS.md ai-state.md docs/tips/*.md

# verify only intended docs changed
git diff --name-only
```

## Tooling discipline
- Keep edits ASCII unless file already requires Unicode.
- Prefer targeted checks over full expensive runs while iterating.
- When output is long, summarize essentials rather than dumping raw logs.
