# Runtime UX, Sessions, Tmux Default, and Model Picker Plan (2026-02-27)

## Scope

Implement and verify the following user-requested behavior changes:

1. Fix terminal flicker and long approval prompt redraw corruption.
2. Replace `/models` with a single `/model` command that opens interactive arrow-key picker (Esc cancels).
3. Update default provider model entries:
   - Replace qwen-based deepseek default with DeepSeek V3.2 profile.
   - Rename profile to `models.openrouter-deepseek`.
   - Add GLM profile entry.
4. Change session identity/lifecycle:
   - Use generated unique hex session IDs (not named `default`).
   - New REPL starts with a fresh session ID.
   - Add `buddy resume <session-id>` and `buddy resume --last`.
5. Make tmux mode default for local operation (without requiring `--tmux`).
6. Collapse and restyle startup message output to requested format.

## Approach

1. Reproduce + diagnose terminal rendering issues.
2. Implement rendering/input/progress fixes with tests.
3. Implement `/model` interactive picker UX and remove `/models`.
4. Update config template models and defaults.
5. Implement session ID generation + CLI resume subcommand behavior.
6. Update tmux default activation logic and startup messaging.
7. Update docs (`README.md`, `DESIGN.md`, `ai-state.md`) and run full tests.
8. Commit with detailed message.

## Progress Log

- [x] Created implementation plan file.
- [x] Baseline diagnosis notes for flicker + long prompt redraw.
- [x] Rendering/input/progress fixes merged with tests.
- [x] `/model` interactive picker completed; `/models` removed.
- [x] Config template/profile updates completed (DeepSeek V3.2 + GLM).
- [x] Session ID lifecycle + `resume` subcommand completed.
- [x] Tmux default execution and startup message refresh completed.
- [x] Docs updated.
- [x] Full test suite passed.
- [x] Changes committed with detailed commit message.

## Baseline Diagnosis Notes

- Interactive input currently re-renders on every poll loop iteration, even when nothing visible changed, which causes avoidable terminal churn and visible flicker.
- Background liveness status text can become very long and wrap; the renderer then tracks multi-row status prefix movement while also clearing `FromCursorDown`, which is fragile and likely source of upward drift/line-clearing corruption with long approval prompts.
- Spinner-like status updates are embedded in the input redraw path and currently force frequent redraw cycles.
