//! `buddy trace` command handlers.
//!
//! This module reads runtime JSONL trace files and renders operator-facing
//! diagnostics for:
//! - high-level summaries,
//! - single-turn replay,
//! - context/token/cost evolution timelines.

use crate::cli::TraceCommand;
use buddy::runtime::{
    MetricsEvent, ModelEvent, RuntimeEvent, RuntimeEventEnvelope, SessionEvent, TaskEvent,
    ToolEvent,
};
use buddy::ui::render::RenderSink;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Aggregated trace-level counters used by `trace summary`.
#[derive(Debug, Clone, PartialEq)]
struct TraceSummary {
    /// Total envelope count parsed from file.
    event_count: usize,
    /// Number of prompt turns observed (`Task.queued(kind=prompt)`).
    turn_count: usize,
    /// Sum of per-request prompt tokens.
    total_prompt_tokens: u64,
    /// Sum of per-request completion tokens.
    total_completion_tokens: u64,
    /// Last observed running session token total.
    session_total_tokens: u64,
    /// Sum of request-level cost estimates from metrics events.
    total_cost_usd: f64,
    /// Last observed running session cost estimate.
    session_total_cost_usd: f64,
    /// Number of session-compaction operations.
    compaction_count: usize,
    /// Number of warnings surfaced in trace.
    warning_count: usize,
    /// Number of explicit runtime error events.
    error_count: usize,
    /// Number of tool result payloads containing a tool-error sentinel.
    tool_error_count: usize,
    /// Tool call frequencies by tool name.
    tool_call_counts: BTreeMap<String, usize>,
    /// Model request counts by model id.
    model_request_counts: BTreeMap<String, usize>,
}

/// Condensed replay view for one prompt turn.
#[derive(Debug, Clone, PartialEq)]
struct TurnReplay {
    /// 1-based prompt turn index in trace order.
    turn: usize,
    /// Runtime task id for this turn.
    task_id: u64,
    /// Queue details from the prompt submission.
    queued_details: String,
    /// Model request summaries observed for this task.
    request_summaries: Vec<RequestSummaryRow>,
    /// Model response summaries observed for this task.
    response_summaries: Vec<ResponseSummaryRow>,
    /// Tool call names observed for this task.
    tool_calls: Vec<String>,
    /// Tool result statuses observed for this task.
    tool_results: Vec<String>,
    /// Final assistant message payload when present.
    final_message: Option<String>,
    /// Warning messages for this task.
    warnings: Vec<String>,
    /// Error messages for this task.
    errors: Vec<String>,
}

/// Flattened request-summary row for replay rendering.
#[derive(Debug, Clone, PartialEq)]
struct RequestSummaryRow {
    /// Model id used for request.
    model: String,
    /// Message count sent in this request.
    message_count: u64,
    /// Tool schema count attached to request.
    tool_count: u64,
    /// Estimated token footprint before sending request.
    estimated_tokens: u64,
}

/// Flattened response-summary row for replay rendering.
#[derive(Debug, Clone, PartialEq)]
struct ResponseSummaryRow {
    /// Provider finish reason when supplied.
    finish_reason: Option<String>,
    /// Tool call count declared by assistant message.
    tool_call_count: u64,
    /// Whether final assistant message had non-empty content.
    has_content: bool,
    /// Prompt token usage from provider telemetry.
    prompt_tokens: Option<u64>,
    /// Completion token usage from provider telemetry.
    completion_tokens: Option<u64>,
}

/// One timeline point for `trace context-evolution`.
#[derive(Debug, Clone, PartialEq)]
struct ContextTimelinePoint {
    /// Relative timestamp from first trace event in seconds.
    t_seconds: f64,
    /// Human-readable event category (`context`, `tokens`, `cost`, `compaction`).
    category: String,
    /// Human-readable details payload.
    detail: String,
}

/// Dispatch `buddy trace` commands.
pub(crate) fn run_trace_command(
    renderer: &impl RenderSink,
    command: &TraceCommand,
) -> Result<(), String> {
    match command {
        TraceCommand::Summary { file } => run_trace_summary(renderer, file),
        TraceCommand::Replay { file, turn } => run_trace_replay(renderer, file.as_str(), *turn),
        TraceCommand::ContextEvolution { file } => run_trace_context_evolution(renderer, file),
    }
}

/// Execute `buddy trace summary <file>`.
fn run_trace_summary(renderer: &impl RenderSink, file: &str) -> Result<(), String> {
    let events = load_trace_file(file)?;
    let summary = summarize_trace(&events);

    renderer.section("trace summary");
    renderer.field("file", file);
    renderer.field("events", &summary.event_count.to_string());
    renderer.field("turns", &summary.turn_count.to_string());
    renderer.field(
        "tokens",
        &format!(
            "prompt:{} completion:{} session:{}",
            summary.total_prompt_tokens,
            summary.total_completion_tokens,
            summary.session_total_tokens
        ),
    );
    if summary.total_cost_usd > 0.0 || summary.session_total_cost_usd > 0.0 {
        renderer.field(
            "cost",
            &format!(
                "request_total:${:.6} session:${:.6}",
                summary.total_cost_usd, summary.session_total_cost_usd
            ),
        );
    } else {
        renderer.field("cost", "n/a (no pricing metrics in trace)");
    }
    renderer.field("compactions", &summary.compaction_count.to_string());
    renderer.field(
        "errors",
        &format!(
            "runtime:{} tool:{} warnings:{}",
            summary.error_count, summary.tool_error_count, summary.warning_count
        ),
    );

    if !summary.model_request_counts.is_empty() {
        renderer.detail("model requests:");
        for (model, count) in &summary.model_request_counts {
            renderer.detail(&format!("  {model}: {count}"));
        }
    }
    if !summary.tool_call_counts.is_empty() {
        renderer.detail("tool call counts:");
        for (tool, count) in &summary.tool_call_counts {
            renderer.detail(&format!("  {tool}: {count}"));
        }
    }
    Ok(())
}

/// Execute `buddy trace replay <file> --turn N`.
fn run_trace_replay(renderer: &impl RenderSink, file: &str, turn: usize) -> Result<(), String> {
    if turn == 0 {
        return Err("turn must be >= 1".to_string());
    }
    let events = load_trace_file(file)?;
    let replay = replay_turn(&events, turn)?;

    renderer.section("trace replay");
    renderer.field("file", file);
    renderer.field("turn", &replay.turn.to_string());
    renderer.field("task", &format!("#{}", replay.task_id));
    renderer.field("prompt", &replay.queued_details);

    if !replay.request_summaries.is_empty() {
        renderer.detail("request summaries:");
        for (idx, row) in replay.request_summaries.iter().enumerate() {
            renderer.detail(&format!(
                "  {}. model={} messages={} tools={} est_tokens={}",
                idx + 1,
                row.model,
                row.message_count,
                row.tool_count,
                row.estimated_tokens
            ));
        }
    }
    if !replay.response_summaries.is_empty() {
        renderer.detail("response summaries:");
        for (idx, row) in replay.response_summaries.iter().enumerate() {
            renderer.detail(&format!(
                "  {}. finish={:?} tool_calls={} has_content={} usage=({:?},{:?})",
                idx + 1,
                row.finish_reason,
                row.tool_call_count,
                row.has_content,
                row.prompt_tokens,
                row.completion_tokens
            ));
        }
    }
    if !replay.tool_calls.is_empty() {
        renderer.detail("tools:");
        for name in &replay.tool_calls {
            renderer.detail(&format!("  call: {name}"));
        }
        for status in &replay.tool_results {
            renderer.detail(&format!("  result: {status}"));
        }
    }
    if let Some(content) = replay.final_message {
        renderer.detail("assistant final:");
        renderer.command_output_block(content.trim());
    }
    if !replay.warnings.is_empty() {
        renderer.detail("warnings:");
        for warning in &replay.warnings {
            renderer.detail(&format!("  - {warning}"));
        }
    }
    if !replay.errors.is_empty() {
        renderer.detail("errors:");
        for error in &replay.errors {
            renderer.detail(&format!("  - {error}"));
        }
    }
    Ok(())
}

/// Execute `buddy trace context-evolution <file>`.
fn run_trace_context_evolution(renderer: &impl RenderSink, file: &str) -> Result<(), String> {
    let events = load_trace_file(file)?;
    let points = context_evolution_points(&events);

    renderer.section("context evolution");
    renderer.field("file", file);
    renderer.field("points", &points.len().to_string());
    if points.is_empty() {
        renderer.detail("no context/token/cost/compaction metrics found");
        return Ok(());
    }
    for point in points {
        renderer.detail(&format!(
            "  t={:.3}s [{}] {}",
            point.t_seconds, point.category, point.detail
        ));
    }
    Ok(())
}

/// Read and decode one runtime JSONL trace file.
fn load_trace_file(path: &str) -> Result<Vec<RuntimeEventEnvelope>, String> {
    let display = Path::new(path).display().to_string();
    let file =
        File::open(path).map_err(|err| format!("failed to open trace file {display}: {err}"))?;
    let reader = BufReader::new(file);

    let mut events = Vec::new();
    for (line_idx, line_result) in reader.lines().enumerate() {
        let line_no = line_idx + 1;
        let line = line_result
            .map_err(|err| format!("failed reading trace file {display} line {line_no}: {err}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let envelope: RuntimeEventEnvelope = serde_json::from_str(trimmed)
            .map_err(|err| format!("invalid trace JSON in {display} line {line_no}: {err}"))?;
        events.push(envelope);
    }
    Ok(events)
}

/// Reduce a full event stream into one summary struct.
fn summarize_trace(events: &[RuntimeEventEnvelope]) -> TraceSummary {
    let mut summary = TraceSummary {
        event_count: events.len(),
        turn_count: 0,
        total_prompt_tokens: 0,
        total_completion_tokens: 0,
        session_total_tokens: 0,
        total_cost_usd: 0.0,
        session_total_cost_usd: 0.0,
        compaction_count: 0,
        warning_count: 0,
        error_count: 0,
        tool_error_count: 0,
        tool_call_counts: BTreeMap::new(),
        model_request_counts: BTreeMap::new(),
    };

    for envelope in events {
        match &envelope.event {
            RuntimeEvent::Task(TaskEvent::Queued { kind, .. }) if kind == "prompt" => {
                summary.turn_count += 1;
            }
            RuntimeEvent::Model(ModelEvent::RequestStarted { model, .. }) => {
                *summary
                    .model_request_counts
                    .entry(model.clone())
                    .or_insert(0) += 1;
            }
            RuntimeEvent::Tool(ToolEvent::CallRequested { name, .. }) => {
                *summary.tool_call_counts.entry(name.clone()).or_insert(0) += 1;
            }
            RuntimeEvent::Tool(ToolEvent::Result { result, .. }) => {
                if result.contains("Tool error:") {
                    summary.tool_error_count += 1;
                }
            }
            RuntimeEvent::Session(SessionEvent::Compacted { .. }) => {
                summary.compaction_count += 1;
            }
            RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
                prompt_tokens,
                completion_tokens,
                session_total_tokens,
                ..
            }) => {
                summary.total_prompt_tokens =
                    summary.total_prompt_tokens.saturating_add(*prompt_tokens);
                summary.total_completion_tokens = summary
                    .total_completion_tokens
                    .saturating_add(*completion_tokens);
                summary.session_total_tokens = *session_total_tokens;
            }
            RuntimeEvent::Metrics(MetricsEvent::Cost {
                request_total_usd,
                session_total_cost_usd,
                ..
            }) => {
                summary.total_cost_usd += *request_total_usd;
                summary.session_total_cost_usd = *session_total_cost_usd;
            }
            RuntimeEvent::Warning(_) => {
                summary.warning_count += 1;
            }
            RuntimeEvent::Error(_) => {
                summary.error_count += 1;
            }
            _ => {}
        }
    }

    summary
}

/// Reconstruct one prompt turn from task-scoped events.
fn replay_turn(events: &[RuntimeEventEnvelope], turn: usize) -> Result<TurnReplay, String> {
    let mut prompt_turns = Vec::<(u64, String)>::new();
    for envelope in events {
        if let RuntimeEvent::Task(TaskEvent::Queued {
            task,
            kind,
            details,
        }) = &envelope.event
        {
            if kind == "prompt" {
                prompt_turns.push((task.task_id, details.clone()));
            }
        }
    }

    let Some((task_id, queued_details)) = prompt_turns.get(turn - 1).cloned() else {
        return Err(format!(
            "turn {turn} not found in trace (available turns: {})",
            prompt_turns.len()
        ));
    };

    let mut replay = TurnReplay {
        turn,
        task_id,
        queued_details,
        request_summaries: Vec::new(),
        response_summaries: Vec::new(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        final_message: None,
        warnings: Vec::new(),
        errors: Vec::new(),
    };

    for envelope in events {
        let belongs_to_turn = event_task_id(&envelope.event).is_some_and(|id| id == task_id);
        if !belongs_to_turn {
            continue;
        }

        match &envelope.event {
            RuntimeEvent::Model(ModelEvent::RequestSummary {
                model,
                message_count,
                tool_count,
                estimated_tokens,
                ..
            }) => replay.request_summaries.push(RequestSummaryRow {
                model: model.clone(),
                message_count: *message_count,
                tool_count: *tool_count,
                estimated_tokens: *estimated_tokens,
            }),
            RuntimeEvent::Model(ModelEvent::ResponseSummary {
                finish_reason,
                tool_call_count,
                has_content,
                prompt_tokens,
                completion_tokens,
                ..
            }) => replay.response_summaries.push(ResponseSummaryRow {
                finish_reason: finish_reason.clone(),
                tool_call_count: *tool_call_count,
                has_content: *has_content,
                prompt_tokens: *prompt_tokens,
                completion_tokens: *completion_tokens,
            }),
            RuntimeEvent::Tool(ToolEvent::CallRequested { name, .. }) => {
                replay.tool_calls.push(name.clone());
            }
            RuntimeEvent::Tool(ToolEvent::Result { name, result, .. }) => {
                let status = if result.contains("Tool error:") {
                    format!("{name} -> error")
                } else {
                    format!("{name} -> ok")
                };
                replay.tool_results.push(status);
            }
            RuntimeEvent::Model(ModelEvent::MessageFinal { content, .. }) => {
                replay.final_message = Some(content.clone());
            }
            RuntimeEvent::Warning(warning) => replay.warnings.push(warning.message.clone()),
            RuntimeEvent::Error(error) => replay.errors.push(error.message.clone()),
            _ => {}
        }
    }

    Ok(replay)
}

/// Build a compact timeline from context/token/cost/compaction trace events.
fn context_evolution_points(events: &[RuntimeEventEnvelope]) -> Vec<ContextTimelinePoint> {
    let base_ts = events
        .first()
        .map(|event| event.ts_unix_ms)
        .unwrap_or_default();
    let mut points = Vec::new();
    for envelope in events {
        let t_seconds = if envelope.ts_unix_ms >= base_ts {
            (envelope.ts_unix_ms - base_ts) as f64 / 1000.0
        } else {
            0.0
        };

        match &envelope.event {
            RuntimeEvent::Metrics(MetricsEvent::ContextUsage {
                task,
                estimated_tokens,
                context_limit,
                used_percent,
            }) => points.push(ContextTimelinePoint {
                t_seconds,
                category: "context".to_string(),
                detail: format!(
                    "task #{} est={} limit={} used={:.1}%",
                    task.task_id, estimated_tokens, context_limit, used_percent
                ),
            }),
            RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
                task,
                prompt_tokens,
                completion_tokens,
                session_total_tokens,
            }) => points.push(ContextTimelinePoint {
                t_seconds,
                category: "tokens".to_string(),
                detail: format!(
                    "task #{} prompt={} completion={} session={}",
                    task.task_id, prompt_tokens, completion_tokens, session_total_tokens
                ),
            }),
            RuntimeEvent::Metrics(MetricsEvent::Cost {
                task,
                model,
                request_total_usd,
                session_total_cost_usd,
                ..
            }) => points.push(ContextTimelinePoint {
                t_seconds,
                category: "cost".to_string(),
                detail: format!(
                    "task #{} model={} request=${:.6} session=${:.6}",
                    task.task_id, model, request_total_usd, session_total_cost_usd
                ),
            }),
            RuntimeEvent::Session(SessionEvent::Compacted {
                session_id,
                estimated_before,
                estimated_after,
                removed_messages,
                removed_turns,
            }) => points.push(ContextTimelinePoint {
                t_seconds,
                category: "compaction".to_string(),
                detail: format!(
                    "session={} before={:?} after={:?} removed_messages={:?} removed_turns={:?}",
                    session_id, estimated_before, estimated_after, removed_messages, removed_turns
                ),
            }),
            _ => {}
        }
    }
    points
}

/// Extract task id from task-scoped events.
fn event_task_id(event: &RuntimeEvent) -> Option<u64> {
    match event {
        RuntimeEvent::Task(TaskEvent::Queued { task, .. })
        | RuntimeEvent::Task(TaskEvent::Started { task })
        | RuntimeEvent::Task(TaskEvent::WaitingApproval { task, .. })
        | RuntimeEvent::Task(TaskEvent::Cancelling { task })
        | RuntimeEvent::Task(TaskEvent::Completed { task })
        | RuntimeEvent::Task(TaskEvent::Failed { task, .. })
        | RuntimeEvent::Model(ModelEvent::RequestStarted { task, .. })
        | RuntimeEvent::Model(ModelEvent::RequestSummary { task, .. })
        | RuntimeEvent::Model(ModelEvent::TextDelta { task, .. })
        | RuntimeEvent::Model(ModelEvent::ReasoningDelta { task, .. })
        | RuntimeEvent::Model(ModelEvent::MessageFinal { task, .. })
        | RuntimeEvent::Model(ModelEvent::ResponseSummary { task, .. })
        | RuntimeEvent::Tool(ToolEvent::CallRequested { task, .. })
        | RuntimeEvent::Tool(ToolEvent::CallStarted { task, .. })
        | RuntimeEvent::Tool(ToolEvent::StdoutChunk { task, .. })
        | RuntimeEvent::Tool(ToolEvent::StderrChunk { task, .. })
        | RuntimeEvent::Tool(ToolEvent::Info { task, .. })
        | RuntimeEvent::Tool(ToolEvent::Completed { task, .. })
        | RuntimeEvent::Tool(ToolEvent::Result { task, .. })
        | RuntimeEvent::Metrics(MetricsEvent::TokenUsage { task, .. })
        | RuntimeEvent::Metrics(MetricsEvent::ContextUsage { task, .. })
        | RuntimeEvent::Metrics(MetricsEvent::PhaseDuration { task, .. })
        | RuntimeEvent::Metrics(MetricsEvent::Cost { task, .. }) => Some(task.task_id),
        RuntimeEvent::Warning(warning) => warning.task.as_ref().map(|task| task.task_id),
        RuntimeEvent::Error(error) => error.task.as_ref().map(|task| task.task_id),
        RuntimeEvent::Lifecycle(_)
        | RuntimeEvent::Session(_)
        | RuntimeEvent::Model(ModelEvent::ProfileSwitched { .. }) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{context_evolution_points, replay_turn, summarize_trace};
    use buddy::runtime::{
        ErrorEvent, MetricsEvent, ModelEvent, RuntimeEvent, RuntimeEventEnvelope, SessionEvent,
        TaskEvent, TaskRef, ToolEvent, WarningEvent,
    };

    /// Build a deterministic envelope for compact test fixtures.
    fn envelope(seq: u64, ts_unix_ms: u64, event: RuntimeEvent) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            seq,
            ts_unix_ms,
            event,
        }
    }

    /// Build a task reference with only `task_id` to keep fixtures concise.
    fn task(task_id: u64) -> TaskRef {
        TaskRef::from_task_id(task_id)
    }

    // Verifies summary aggregation computes counts, tokens, costs, and errors.
    #[test]
    fn summarize_trace_aggregates_core_metrics() {
        let events = vec![
            envelope(
                1,
                1_000,
                RuntimeEvent::Task(TaskEvent::Queued {
                    task: task(1),
                    kind: "prompt".to_string(),
                    details: "list files".to_string(),
                }),
            ),
            envelope(
                2,
                1_100,
                RuntimeEvent::Model(ModelEvent::RequestStarted {
                    task: task(1),
                    model: "gpt-spark".to_string(),
                }),
            ),
            envelope(
                3,
                1_200,
                RuntimeEvent::Tool(ToolEvent::CallRequested {
                    task: task(1),
                    name: "run_shell".to_string(),
                    arguments_json: "{\"command\":\"ls\"}".to_string(),
                }),
            ),
            envelope(
                4,
                1_300,
                RuntimeEvent::Tool(ToolEvent::Result {
                    task: task(1),
                    name: "run_shell".to_string(),
                    arguments_json: "{}".to_string(),
                    result: "Tool error: denied".to_string(),
                }),
            ),
            envelope(
                5,
                1_400,
                RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
                    task: task(1),
                    prompt_tokens: 100,
                    completion_tokens: 25,
                    session_total_tokens: 125,
                }),
            ),
            envelope(
                6,
                1_500,
                RuntimeEvent::Metrics(MetricsEvent::Cost {
                    task: task(1),
                    model: "gpt-spark".to_string(),
                    prompt_tokens: 100,
                    completion_tokens: 25,
                    cached_tokens: None,
                    request_input_cost_usd: 0.001,
                    request_output_cost_usd: 0.002,
                    request_cache_read_cost_usd: 0.0,
                    request_total_usd: 0.003,
                    session_total_cost_usd: 0.003,
                }),
            ),
            envelope(
                7,
                1_550,
                RuntimeEvent::Session(SessionEvent::Compacted {
                    session_id: "abcd".to_string(),
                    estimated_before: Some(7000),
                    estimated_after: Some(4500),
                    removed_messages: Some(10),
                    removed_turns: Some(3),
                }),
            ),
            envelope(
                8,
                1_560,
                RuntimeEvent::Warning(WarningEvent {
                    task: Some(task(1)),
                    message: "warn".to_string(),
                }),
            ),
            envelope(
                9,
                1_570,
                RuntimeEvent::Error(ErrorEvent {
                    task: Some(task(1)),
                    message: "err".to_string(),
                }),
            ),
        ];

        let summary = summarize_trace(&events);
        assert_eq!(summary.event_count, 9);
        assert_eq!(summary.turn_count, 1);
        assert_eq!(summary.total_prompt_tokens, 100);
        assert_eq!(summary.total_completion_tokens, 25);
        assert_eq!(summary.session_total_tokens, 125);
        assert!((summary.total_cost_usd - 0.003).abs() < 1e-12);
        assert!((summary.session_total_cost_usd - 0.003).abs() < 1e-12);
        assert_eq!(summary.compaction_count, 1);
        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.tool_error_count, 1);
        assert_eq!(summary.tool_call_counts.get("run_shell").copied(), Some(1));
        assert_eq!(
            summary.model_request_counts.get("gpt-spark").copied(),
            Some(1)
        );
    }

    // Verifies replay selects the requested prompt turn and collects task-scoped details.
    #[test]
    fn replay_turn_collects_task_timeline() {
        let events = vec![
            envelope(
                1,
                1_000,
                RuntimeEvent::Task(TaskEvent::Queued {
                    task: task(1),
                    kind: "prompt".to_string(),
                    details: "first".to_string(),
                }),
            ),
            envelope(
                2,
                1_100,
                RuntimeEvent::Task(TaskEvent::Queued {
                    task: task(2),
                    kind: "prompt".to_string(),
                    details: "second".to_string(),
                }),
            ),
            envelope(
                3,
                1_200,
                RuntimeEvent::Model(ModelEvent::RequestSummary {
                    task: task(2),
                    model: "gpt-spark".to_string(),
                    message_count: 5,
                    tool_count: 2,
                    estimated_tokens: 400,
                }),
            ),
            envelope(
                4,
                1_300,
                RuntimeEvent::Model(ModelEvent::ResponseSummary {
                    task: task(2),
                    finish_reason: Some("stop".to_string()),
                    tool_call_count: 0,
                    has_content: true,
                    prompt_tokens: Some(123),
                    completion_tokens: Some(45),
                    total_tokens: Some(168),
                }),
            ),
            envelope(
                5,
                1_400,
                RuntimeEvent::Model(ModelEvent::MessageFinal {
                    task: task(2),
                    content: "hello".to_string(),
                }),
            ),
        ];

        let replay = replay_turn(&events, 2).expect("turn");
        assert_eq!(replay.turn, 2);
        assert_eq!(replay.task_id, 2);
        assert_eq!(replay.queued_details, "second");
        assert_eq!(replay.request_summaries.len(), 1);
        assert_eq!(replay.response_summaries.len(), 1);
        assert_eq!(replay.final_message.as_deref(), Some("hello"));
    }

    // Verifies context evolution captures context/token/cost and compaction points.
    #[test]
    fn context_evolution_points_collects_expected_categories() {
        let events = vec![
            envelope(
                1,
                1_000,
                RuntimeEvent::Metrics(MetricsEvent::ContextUsage {
                    task: task(1),
                    estimated_tokens: 100,
                    context_limit: 1_000,
                    used_percent: 10.0,
                }),
            ),
            envelope(
                2,
                2_000,
                RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
                    task: task(1),
                    prompt_tokens: 10,
                    completion_tokens: 4,
                    session_total_tokens: 14,
                }),
            ),
            envelope(
                3,
                3_000,
                RuntimeEvent::Metrics(MetricsEvent::Cost {
                    task: task(1),
                    model: "gpt-spark".to_string(),
                    prompt_tokens: 10,
                    completion_tokens: 4,
                    cached_tokens: None,
                    request_input_cost_usd: 0.001,
                    request_output_cost_usd: 0.002,
                    request_cache_read_cost_usd: 0.0,
                    request_total_usd: 0.003,
                    session_total_cost_usd: 0.003,
                }),
            ),
            envelope(
                4,
                4_000,
                RuntimeEvent::Session(SessionEvent::Compacted {
                    session_id: "sid".to_string(),
                    estimated_before: Some(1000),
                    estimated_after: Some(700),
                    removed_messages: Some(4),
                    removed_turns: Some(1),
                }),
            ),
        ];
        let points = context_evolution_points(&events);
        assert_eq!(points.len(), 4);
        assert_eq!(
            points
                .iter()
                .map(|p| p.category.as_str())
                .collect::<Vec<_>>(),
            vec!["context", "tokens", "cost", "compaction"]
        );
        assert_eq!(points[0].t_seconds, 0.0);
        assert!((points[1].t_seconds - 1.0).abs() < 0.001);
    }

    // Verifies replay command rejects out-of-range turns with actionable error text.
    #[test]
    fn replay_turn_errors_for_out_of_range_turn() {
        let events = vec![envelope(
            1,
            1_000,
            RuntimeEvent::Task(TaskEvent::Queued {
                task: task(1),
                kind: "prompt".to_string(),
                details: "only".to_string(),
            }),
        )];
        let err = replay_turn(&events, 2).expect_err("missing turn should fail");
        assert!(err.contains("available turns: 1"));
    }
}
