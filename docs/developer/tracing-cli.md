# Tracing CLI

Buddy can write runtime events to JSONL (`--trace <path>` / `BUDDY_TRACE_FILE`) and then analyze the trace offline with `buddy trace`.

## Commands

- `buddy trace summary <file>`
  - High-level counters: turns, token totals, cost totals, tool frequencies, warning/error counts.
- `buddy trace replay <file> --turn <n>`
  - Reconstructs one prompt turn (queue details, request/response summaries, tools, warnings/errors, final assistant output when present).
- `buddy trace context-evolution <file>`
  - Timeline view of context usage, token usage, cost metrics, and compaction events.

## Example

```bash
buddy --trace /tmp/buddy.trace.jsonl
buddy trace summary /tmp/buddy.trace.jsonl
buddy trace replay /tmp/buddy.trace.jsonl --turn 3
buddy trace context-evolution /tmp/buddy.trace.jsonl
```

For repeatable real-model prompt comparisons, pair this with:

```bash
make prompt-eval MODEL=<profile> PROMPTS=<file>
```

Each probe stores per-run trace JSONL files that can be inspected with these
trace commands.

## Cost Metrics

When model pricing metadata exists for the active model in `src/templates/models.toml`, Buddy emits per-request/session cost metrics into runtime events:

- input cost
- output cost
- cache-read cost (if available)
- request total
- session total

If pricing metadata is missing, trace output still works and cost fields are shown as unavailable.

## Notes

- Trace parsing is strict JSONL; malformed lines fail with file+line diagnostics.
- Turn numbers are 1-based and correspond to prompt task order (`Task.queued(kind=prompt)`).
- `trace replay` is task/event reconstruction, not a byte-for-byte raw API payload dump.
