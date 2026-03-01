# Context Management

This document describes how Buddy manages message-history growth and preserves
tool-call protocol integrity while compacting long sessions.

## Goals

- Keep requests inside model context limits.
- Preserve enough recent context for continuity.
- Keep assistant/tool protocol-valid message history after compaction.
- Preserve recent failure evidence for debugging.

## Compaction Triggers

Buddy compacts history in two paths:

- automatic compaction in the agent loop when estimated usage exceeds the auto target,
- manual compaction via `/compact` (stronger target, forced mode).

If usage still exceeds the hard context threshold after compaction, Buddy
returns a context-limit error and asks the operator to start or compact a
session.

## Pre/Post Validation

Before and after compaction, Buddy sanitizes and validates conversation history:

- trims empty/whitespace-only content,
- deduplicates malformed/duplicate tool-call ids in assistant messages,
- drops orphan `tool` messages with no live assistant call id,
- removes unmatched assistant tool-call declarations when no result arrived.

This keeps history protocol-valid for providers that require strict
assistant-tool/result pairing.

## Atomic Compaction Units

Compaction works on turn-like units, not individual messages:

- boundaries are aligned to user turns,
- boundaries are delayed when assistant tool calls are still pending,
- assistant tool calls and matching tool results are removed together.

This prevents history states where one side of a tool-call/result pair is
removed while the other survives.

## Summary Format

Removed history is represented by one synthetic system summary message:

- prefix: `[buddy compact summary]`
- structured entries:
  - `op=<operation>`
  - `status=<info|success|failure>`
  - `detail=<key outcome/error>`

Entries are bounded so summary growth remains controlled.

## Failure-Preserving Retention

Buddy always retains the three most-recent failed tool operations verbatim in
message history during compaction. These failures are protected from removal to
preserve exact operator-visible error payloads for follow-up debugging.

## Runtime Observability

Compaction emits runtime session events with:

- pre/post estimated token counts,
- removed message count,
- removed unit/turn count.

When tracing is enabled (`--trace`), these details are captured in the JSONL
event stream for replay and diagnostics.
