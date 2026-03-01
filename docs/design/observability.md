# Observability

Buddy supports optional runtime-event tracing for debugging and replay.

## Runtime Trace File

- Enable with `--trace <path>` or `BUDDY_TRACE_FILE=<path>`.
- Output format is JSON Lines (one `RuntimeEventEnvelope` per line).
- Tracing is best-effort:
  - startup open failures are warnings (runtime continues),
  - write failures disable tracing and emit one warning.

## Record Shape

Each line is a serialized `RuntimeEventEnvelope`:

```json
{
  "seq": 42,
  "ts_unix_ms": 1762051123000,
  "event": {
    "type": "Task",
    "payload": {
      "started": {
        "task": { "task_id": 1 }
      }
    }
  }
}
```

`seq` is monotonic per runtime stream and `ts_unix_ms` is wall-clock capture time.

Task-scoped events include enriched `TaskRef` metadata when available:

- `task_id`
- `session_id`
- `iteration` (model/tool loop iteration for agent-emitted events)
- `correlation_id` (stable per submitted prompt)

## High-Value Trace Events

Milestone-1 runtime traces include:

- request lifecycle:
  - `Model.RequestStarted`
  - `Model.RequestSummary` (`message_count`, `tool_count`, `estimated_tokens`)
  - `Metrics.PhaseDuration` (`phase = "model_request"`)
- response lifecycle:
  - `Model.ResponseSummary` (`finish_reason`, tool-call count, content presence, usage)
  - `Model.MessageFinal` when a final assistant response is produced
- tool lifecycle:
  - `Tool.CallRequested`
  - `Tool.Result`
  - `Metrics.PhaseDuration` (`phase = "tool:<name>"`)
- compaction lifecycle:
  - `Session.Compacted` with pre/post token estimate fields and removal counts

## Redaction Policy

Before writing a trace record, Buddy redacts obvious sensitive content:

- Secret-shaped key names (`api_key`, `password`, `secret`, `access_token`, `refresh_token`)
- Secret markers in free-form strings (for example `Bearer ...`, `sk-...`, private key headers)

Redaction is heuristic and conservative; traces are intended for local operator use.

## Operational Notes

- Trace records follow runtime event ordering.
- Duplicate sequence IDs are skipped by the trace writer.
- Recommended for incident debugging, tool-flow audits, and model-behavior analysis.
