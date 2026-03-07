//! Generic trace-event parsing and summarization for `buddy traceui`.
//!
//! This parser intentionally works from raw JSON values instead of the typed
//! runtime schema so the viewer stays useful across trace-format evolution.

use crate::textutil::truncate_with_suffix_by_chars;
use serde_json::{Map, Value};

/// Maximum preview length for collapsed event detail.
pub const PREVIEW_CHAR_LIMIT: usize = 500;

/// Parsed trace event with stable fields for the interactive viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceEvent {
    /// Original 1-based source line number in the JSONL file.
    pub line_no: usize,
    /// Envelope sequence number when present.
    pub seq: Option<u64>,
    /// Unix timestamp in milliseconds when present.
    pub ts_unix_ms: Option<u64>,
    /// Top-level runtime event family (for example `Tool`, `Model`).
    pub family: String,
    /// Variant inside the family (for example `call_requested`).
    pub variant: String,
    /// Short event title shown in the event list.
    pub title: String,
    /// Condensed one-line summary shown in the event list.
    pub summary: String,
    /// Optional task id extracted from nested payloads.
    pub task_id: Option<u64>,
    /// Optional iteration extracted from nested task refs.
    pub iteration: Option<u64>,
    /// Optional session id extracted from nested task refs.
    pub session_id: Option<String>,
    /// Full detail text shown in the detail pane when expanded.
    pub detail_full: String,
    /// Collapsed detail preview text with bounded length.
    pub detail_preview: String,
    /// True when this line could not be parsed as JSON.
    pub parse_error: bool,
}

impl TraceEvent {
    /// Decode one JSONL line into a generic trace event.
    pub fn from_line(line_no: usize, line: &str) -> Self {
        match serde_json::from_str::<Value>(line) {
            Ok(root) => Self::from_value(line_no, root),
            Err(err) => Self::parse_error(line_no, line, err.to_string()),
        }
    }

    fn from_value(line_no: usize, root: Value) -> Self {
        let seq = root.get("seq").and_then(Value::as_u64);
        let ts_unix_ms = root.get("ts_unix_ms").and_then(Value::as_u64);
        let event_value = root.get("event").cloned().unwrap_or(Value::Null);
        let (family, variant, payload) = extract_descriptor(&event_value);
        let task = find_task_ref(&payload);
        let title = build_title(&family, &variant, &payload);
        let summary = build_summary(&family, &variant, &payload);
        let detail_full = build_detail_text(
            seq,
            ts_unix_ms,
            family.as_str(),
            variant.as_str(),
            task.as_ref(),
            &payload,
            None,
        );
        let detail_preview =
            truncate_with_suffix_by_chars(&detail_full, PREVIEW_CHAR_LIMIT, "\n… [truncated]");

        Self {
            line_no,
            seq,
            ts_unix_ms,
            family,
            variant,
            title,
            summary,
            task_id: task.as_ref().and_then(|task| task.task_id),
            iteration: task.as_ref().and_then(|task| task.iteration),
            session_id: task.and_then(|task| task.session_id),
            detail_full,
            detail_preview,
            parse_error: false,
        }
    }

    fn parse_error(line_no: usize, line: &str, error: String) -> Self {
        let detail_full = format!(
            "Event\n  line: {line_no}\n  family: ParseError\n\nParse error\n  {error}\n\nRaw line\n  {}",
            line.trim_end()
        );
        let detail_preview =
            truncate_with_suffix_by_chars(&detail_full, PREVIEW_CHAR_LIMIT, "\n… [truncated]");
        Self {
            line_no,
            seq: None,
            ts_unix_ms: None,
            family: "ParseError".to_string(),
            variant: "invalid_json".to_string(),
            title: "invalid trace line".to_string(),
            summary: truncate_single_line(line.trim_end(), 120),
            task_id: None,
            iteration: None,
            session_id: None,
            detail_full,
            detail_preview,
            parse_error: true,
        }
    }

    /// Compact label used in the event list.
    pub fn list_label(&self) -> String {
        let seq = self
            .seq
            .map(|seq| format!("#{seq}"))
            .unwrap_or_else(|| format!("L{}", self.line_no));
        let family = self.family_variant_label();
        let task = self
            .task_id
            .map(|task| format!(" task#{task}"))
            .unwrap_or_default();
        format!("{seq} {family}{task}")
    }

    /// Combined family/variant label used in headers.
    pub fn family_variant_label(&self) -> String {
        if self.variant.is_empty() {
            self.family.clone()
        } else {
            format!("{}/{}", self.family, self.variant)
        }
    }
}

#[derive(Debug, Clone)]
struct TaskRefView {
    task_id: Option<u64>,
    iteration: Option<u64>,
    session_id: Option<String>,
    correlation_id: Option<String>,
}

fn extract_descriptor(event: &Value) -> (String, String, Value) {
    let Some(obj) = event.as_object() else {
        return ("Unknown".to_string(), "event".to_string(), event.clone());
    };

    let family = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
        .to_string();
    let payload = obj.get("payload").cloned().unwrap_or(Value::Null);

    match payload {
        Value::String(name) => (family, name, Value::Null),
        Value::Object(map) if map.len() == 1 => {
            let (variant, detail) = map.iter().next().expect("single-entry payload map");
            (family, variant.clone(), detail.clone())
        }
        other => (family, "payload".to_string(), other),
    }
}

fn build_title(family: &str, variant: &str, payload: &Value) -> String {
    match (family, variant) {
        ("Tool", "call_requested") | ("Tool", "call_started") | ("Tool", "completed") => payload
            .get("name")
            .and_then(Value::as_str)
            .map(|name| format!("tool {name}"))
            .unwrap_or_else(|| format!("tool {variant}")),
        ("Tool", "result") => payload
            .get("name")
            .and_then(Value::as_str)
            .map(|name| format!("tool result {name}"))
            .unwrap_or_else(|| "tool result".to_string()),
        ("Model", "request_started") | ("Model", "request_summary") => payload
            .get("model")
            .and_then(Value::as_str)
            .map(|model| format!("model {model}"))
            .unwrap_or_else(|| format!("model {variant}")),
        ("Task", "queued") => payload
            .get("kind")
            .and_then(Value::as_str)
            .map(|kind| format!("task queued {kind}"))
            .unwrap_or_else(|| "task queued".to_string()),
        ("Warning", _) => "warning".to_string(),
        ("Error", _) => "error".to_string(),
        _ => format!(
            "{} {}",
            family.to_ascii_lowercase(),
            variant.replace('_', " ")
        ),
    }
}

fn build_summary(family: &str, variant: &str, payload: &Value) -> String {
    match (family, variant) {
        ("Lifecycle", name) => name.replace('_', " "),
        ("Session", "created") | ("Session", "resumed") | ("Session", "saved") => payload
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("session")
            .to_string(),
        ("Session", "compacted") => format!(
            "before={} after={} removed_messages={} removed_turns={}",
            format_optional_number(payload.get("estimated_before")),
            format_optional_number(payload.get("estimated_after")),
            format_optional_number(payload.get("removed_messages")),
            format_optional_number(payload.get("removed_turns"))
        ),
        ("Task", "queued") => payload
            .get("details")
            .and_then(Value::as_str)
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        ("Task", "waiting_approval") => payload
            .get("command")
            .and_then(Value::as_str)
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        ("Task", "failed") => payload
            .get("message")
            .and_then(Value::as_str)
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        ("Task", _) => payload_summary(payload),
        ("Model", "request_started") => payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ("Model", "request_summary") => format!(
            "messages={} tools={} est_tokens={}",
            format_optional_number(payload.get("message_count")),
            format_optional_number(payload.get("tool_count")),
            format_optional_number(payload.get("estimated_tokens"))
        ),
        ("Model", "response_summary") => format!(
            "finish={} tool_calls={} content={} usage=({}, {})",
            payload
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("n/a"),
            format_optional_number(payload.get("tool_call_count")),
            payload
                .get("has_content")
                .and_then(Value::as_bool)
                .map(|value| if value { "yes" } else { "no" })
                .unwrap_or("n/a"),
            format_optional_number(payload.get("prompt_tokens")),
            format_optional_number(payload.get("completion_tokens"))
        ),
        ("Model", "text_delta") | ("Model", "reasoning_delta") => payload
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        ("Model", "message_final") => payload
            .get("content")
            .and_then(Value::as_str)
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        ("Tool", "call_requested") => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = payload
                .get("arguments_json")
                .and_then(Value::as_str)
                .map(|text| truncate_single_line(text, 80))
                .unwrap_or_else(|| "{}".to_string());
            format!("{name}({args})")
        }
        ("Tool", "result") => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let result = payload
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status = if result.contains("Tool error:") {
                "error"
            } else {
                "ok"
            };
            let preview = truncate_single_line(result, 80);
            format!("{name} -> {status} {preview}")
        }
        ("Tool", "stdout_chunk") | ("Tool", "stderr_chunk") => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let key = if variant == "stdout_chunk" {
                "chunk"
            } else {
                "chunk"
            };
            let chunk = payload
                .get(key)
                .and_then(Value::as_str)
                .map(|text| truncate_single_line(text, 100))
                .unwrap_or_default();
            format!("{name}: {chunk}")
        }
        ("Tool", _) => payload
            .get("detail")
            .and_then(Value::as_str)
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        ("Metrics", "token_usage") => format!(
            "prompt={} completion={} session={}",
            format_optional_number(payload.get("prompt_tokens")),
            format_optional_number(payload.get("completion_tokens")),
            format_optional_number(payload.get("session_total_tokens"))
        ),
        ("Metrics", "context_usage") => format!(
            "est={} limit={} used={}%%",
            format_optional_number(payload.get("estimated_tokens")),
            format_optional_number(payload.get("context_limit")),
            format_optional_float(payload.get("used_percent"))
        ),
        ("Metrics", "phase_duration") => format!(
            "{} {}ms",
            payload
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("phase"),
            format_optional_number(payload.get("elapsed_ms"))
        ),
        ("Metrics", "cost") => format!(
            "model={} request=${} session=${}",
            payload
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("n/a"),
            format_optional_float(payload.get("request_total_usd")),
            format_optional_float(payload.get("session_total_cost_usd"))
        ),
        ("Warning", _) | ("Error", _) => payload
            .get("message")
            .and_then(Value::as_str)
            .map(|text| truncate_single_line(text, 120))
            .unwrap_or_else(|| payload_summary(payload)),
        _ => payload_summary(payload),
    }
}

fn payload_summary(payload: &Value) -> String {
    if payload.is_null() {
        return String::new();
    }
    truncate_single_line(&value_to_inline_string(payload), 120)
}

fn format_optional_number(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_optional_float(value: Option<&Value>) -> String {
    if let Some(value) = value.and_then(Value::as_f64) {
        if value.fract() == 0.0 {
            format!("{value:.0}")
        } else {
            format!("{value:.3}")
        }
    } else {
        "n/a".to_string()
    }
}

fn build_detail_text(
    seq: Option<u64>,
    ts_unix_ms: Option<u64>,
    family: &str,
    variant: &str,
    task: Option<&TaskRefView>,
    payload: &Value,
    parse_error: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("Event\n");
    out.push_str(&format!("  family: {family}\n"));
    out.push_str(&format!("  variant: {variant}\n"));
    out.push_str(&format!(
        "  seq: {}\n",
        seq.map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    ));
    out.push_str(&format!(
        "  timestamp_ms: {}\n",
        ts_unix_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    ));
    if let Some(task) = task {
        out.push_str("  task:\n");
        if let Some(task_id) = task.task_id {
            out.push_str(&format!("    id: {task_id}\n"));
        }
        if let Some(iteration) = task.iteration {
            out.push_str(&format!("    iteration: {iteration}\n"));
        }
        if let Some(session_id) = task.session_id.as_deref() {
            out.push_str(&format!("    session: {session_id}\n"));
        }
        if let Some(correlation_id) = task.correlation_id.as_deref() {
            out.push_str(&format!("    correlation_id: {correlation_id}\n"));
        }
    }
    if let Some(error) = parse_error {
        out.push_str("\nParse error\n");
        out.push_str(&format!("  {error}\n"));
    }
    out.push_str("\nPayload\n");
    render_value_block(&mut out, payload, 1, None);
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn render_value_block(out: &mut String, value: &Value, indent: usize, key: Option<&str>) {
    let prefix = "  ".repeat(indent);
    match value {
        Value::Object(map) => render_object_block(out, map, indent, key),
        Value::Array(items) => render_array_block(out, items, indent, key),
        Value::String(text) => {
            if let Some(decoded) = parse_embedded_json(text) {
                let label = key
                    .map(|key| format!("{key} (json)"))
                    .unwrap_or_else(|| "decoded_json".to_string());
                render_value_block(out, &decoded, indent, Some(label.as_str()));
            } else {
                let label = key.map(|key| format!("{key}: ")).unwrap_or_default();
                out.push_str(&prefix);
                out.push_str(&label);
                out.push_str(&scalar_to_string(value));
                out.push('\n');
            }
        }
        _ => {
            let label = key.map(|key| format!("{key}: ")).unwrap_or_default();
            out.push_str(&prefix);
            out.push_str(&label);
            out.push_str(&scalar_to_string(value));
            out.push('\n');
        }
    }
}

fn render_object_block(
    out: &mut String,
    map: &Map<String, Value>,
    indent: usize,
    key: Option<&str>,
) {
    let prefix = "  ".repeat(indent);
    if let Some(key) = key {
        out.push_str(&prefix);
        out.push_str(key);
        out.push_str(":\n");
    } else if map.is_empty() {
        out.push_str(&prefix);
        out.push_str("{}\n");
        return;
    }

    if map.is_empty() {
        out.push_str(&prefix);
        out.push_str("  {}\n");
        return;
    }

    for (child_key, child_value) in map {
        render_value_block(out, child_value, indent + 1, Some(child_key));
    }
}

fn render_array_block(out: &mut String, items: &[Value], indent: usize, key: Option<&str>) {
    let prefix = "  ".repeat(indent);
    if let Some(key) = key {
        out.push_str(&prefix);
        out.push_str(key);
        out.push_str(":\n");
    }
    if items.is_empty() {
        out.push_str(&prefix);
        out.push_str("  []\n");
        return;
    }

    for item in items {
        match item {
            Value::Object(_) | Value::Array(_) => {
                out.push_str(&prefix);
                out.push_str("  -\n");
                render_value_block(out, item, indent + 2, None);
            }
            _ => {
                out.push_str(&prefix);
                out.push_str("  - ");
                out.push_str(&scalar_to_string(item));
                out.push('\n');
            }
        }
    }
}

fn scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => serde_json::to_string(text).unwrap_or_else(|_| text.to_string()),
        Value::Object(_) | Value::Array(_) => value_to_inline_string(value),
    }
}

/// Decode JSON-looking string payloads so nested structured fields render as
/// readable blocks instead of opaque inline blobs.
fn parse_embedded_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let value = serde_json::from_str::<Value>(trimmed).ok()?;
    if matches!(value, Value::Object(_) | Value::Array(_)) {
        Some(value)
    } else {
        None
    }
}

fn value_to_inline_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.to_string(),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let flattened = text.replace('\n', " ");
    truncate_with_suffix_by_chars(flattened.trim(), max_chars, "...")
}

fn find_task_ref(value: &Value) -> Option<TaskRefView> {
    let Some(candidate) = find_object_by_key(value, "task") else {
        return None;
    };
    let obj = candidate.as_object()?;
    Some(TaskRefView {
        task_id: obj.get("task_id").and_then(Value::as_u64),
        iteration: obj.get("iteration").and_then(Value::as_u64),
        session_id: obj
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        correlation_id: obj
            .get("correlation_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn find_object_by_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            map.values()
                .find_map(|value| find_object_by_key(value, key))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|value| find_object_by_key(value, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::TraceEvent;

    #[test]
    fn parses_known_tool_event_shape() {
        let line = r#"{"seq":11,"ts_unix_ms":1772721367105,"event":{"type":"Tool","payload":{"call_requested":{"name":"tmux_capture_pane","arguments_json":"{\"target\":\"\"}","task":{"task_id":1,"iteration":2,"session_id":"abc"}}}}}"#;
        let event = TraceEvent::from_line(1, line);
        assert_eq!(event.seq, Some(11));
        assert_eq!(event.family, "Tool");
        assert_eq!(event.variant, "call_requested");
        assert_eq!(event.task_id, Some(1));
        assert_eq!(event.iteration, Some(2));
        assert_eq!(event.session_id.as_deref(), Some("abc"));
        assert!(event.summary.contains("tmux_capture_pane"));
    }

    #[test]
    fn falls_back_cleanly_for_unknown_payload_shapes() {
        let line =
            r#"{"seq":1,"event":{"type":"Weird","payload":{"alpha":1,"beta":[true,false]}}}"#;
        let event = TraceEvent::from_line(3, line);
        assert_eq!(event.family, "Weird");
        assert_eq!(event.variant, "payload");
        assert!(event.detail_full.contains("alpha"));
        assert!(event.detail_full.contains("beta"));
    }

    #[test]
    fn turns_invalid_json_into_synthetic_parse_event() {
        let event = TraceEvent::from_line(5, "{not-json}");
        assert!(event.parse_error);
        assert_eq!(event.family, "ParseError");
        assert!(event.detail_full.contains("Parse error"));
    }

    #[test]
    fn preview_is_truncated_with_expand_hint() {
        let long_message = "x".repeat(700);
        let line = format!(
            "{{\"seq\":2,\"event\":{{\"type\":\"Warning\",\"payload\":{{\"message\":\"{long_message}\"}}}}}}"
        );
        let event = TraceEvent::from_line(1, &line);
        assert!(event.detail_preview.len() < event.detail_full.len());
        assert!(event.detail_preview.contains("[truncated]"));
    }

    #[test]
    fn expands_json_encoded_string_fields_in_detail() {
        let line = r#"{"seq":11,"event":{"type":"Tool","payload":{"call_requested":{"name":"tmux_capture_pane","arguments_json":"{\"target\":\"pane-1\",\"lines\":[\"a\",\"b\"]}"}}}}"#;
        let event = TraceEvent::from_line(1, line);
        assert!(event.detail_full.contains("arguments_json (json):"));
        assert!(event.detail_full.contains("target: \"pane-1\""));
        assert!(event.detail_full.contains("- \"a\""));
    }
}
