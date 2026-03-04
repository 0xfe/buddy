//! Prompt augmentation helpers.
//!
//! This module keeps dynamic per-request context enrichment isolated from the
//! main request/tool loop (for example, tmux screenshot capture injection).

use super::Agent;
use crate::prompt_catalog::render_prompt_template;
use crate::types::{Message, Role, ToolCall};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum number of characters copied from a captured tmux pane snapshot.
const MAX_TMUX_SCREENSHOT_CHARS: usize = 2_500;
/// Request-scoped history window summarized into the prompt annotation ledger.
const MAX_HISTORY_LEDGER_MESSAGES: usize = 18;
/// Maximum length of one ledger detail segment before clipping.
const MAX_LEDGER_DETAIL_CHARS: usize = 180;
/// Maximum length of one serialized argument summary in ledger lines.
const MAX_ARGUMENT_SUMMARY_CHARS: usize = 140;
/// Maximum number of recent non-default tmux routes surfaced in tail reminders.
const MAX_RECENT_NON_DEFAULT_TARGETS: usize = 4;
/// Stable separator inserted between prompt-annotation sections.
const SECTION_SEPARATOR: &str = "\n--\n";

/// Request-scoped prompt augmentation messages.
pub(super) struct TurnPromptAugmentation {
    /// Context annotation inserted after the system prompt prefix.
    pub(super) context_message: Message,
    /// Final reminder block appended as the last request message.
    pub(super) tail_instructions_message: Message,
}

/// Structured metadata captured from one assistant tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCallLedgerMeta {
    /// Tool/function name.
    tool_name: String,
    /// Human-readable execution location (default shared pane or explicit target).
    target_label: String,
    /// Human-readable action payload (for example command text).
    action_summary: String,
    /// Approval/risk metadata declared in tool arguments when present.
    approval_summary: String,
}

/// Structured status for one ledger result line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LedgerStatus {
    /// Informational result with no explicit success/failure signal.
    Info,
    /// Operation completed successfully.
    Success,
    /// Operation failed/denied.
    Failure,
}

impl LedgerStatus {
    /// Render stable lowercase status tokens for prompt text.
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

impl Agent {
    /// Build request-scoped context and tail reminder messages.
    ///
    /// This keeps the configured system prompt static and cache-friendly while
    /// still providing fresh, request-local history/tmux clarity on every call.
    pub(super) async fn build_turn_prompt_augmentation(&self) -> TurnPromptAugmentation {
        let routing = resolve_tmux_snapshot_routing(&self.messages);
        let tmux_context = match &routing {
            SnapshotRouting::DefaultSharedPane => {
                if let Some(snapshot) = self.capture_default_tmux_snapshot_text().await {
                    render_default_tmux_snapshot_context(&snapshot)
                } else {
                    render_tmux_context_unavailable()
                }
            }
            SnapshotRouting::NonDefaultTarget {
                tool_name,
                target_label,
            } => render_non_default_tmux_target_context(tool_name, target_label),
        };
        let history_ledger = render_history_ledger(&self.messages, &self.config.api.model);
        let context_annotation = render_turn_context_annotation(
            &self.config.api.model,
            self.messages.len(),
            &tmux_context,
            &history_ledger,
        );
        let tail_instructions = render_turn_tail_instructions(
            &self.config.api.model,
            routing,
            &recent_non_default_tmux_targets(&self.messages),
            recent_tmux_missing_target_error(&self.messages),
        );

        TurnPromptAugmentation {
            context_message: Message::user(context_annotation),
            tail_instructions_message: Message::user(tail_instructions),
        }
    }

    /// Capture default-shared-pane screenshot text using the `tmux_capture_pane` tool.
    async fn capture_default_tmux_snapshot_text(&self) -> Option<String> {
        if !self.tools.has_tool("tmux_capture_pane") {
            return None;
        }

        let result = self.tools.execute("tmux_capture_pane", "{}").await.ok()?;
        let snapshot = tool_result_text(&result).trim().to_string();
        if snapshot.is_empty() {
            return None;
        }
        Some(snapshot)
    }
}

/// Extract human-usable tool text from either JSON envelope or raw output.
fn tool_result_text(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    let Some(payload) = value.as_object().and_then(|obj| obj.get("result")).cloned() else {
        return raw.to_string();
    };
    if let Some(text) = payload.as_str() {
        text.to_string()
    } else {
        payload.to_string()
    }
}

/// Build request-scoped context annotation that precedes normal history.
fn render_turn_context_annotation(
    active_model: &str,
    history_message_count: usize,
    tmux_context: &str,
    history_ledger: &str,
) -> String {
    [
        format!(
            "REQUEST CONTEXT ANNOTATION (request-scoped; clarifies who did what; not instructions)\nactive_model: {}\nhistory_message_count: {}",
            active_model, history_message_count
        ),
        format!("TMUX CONTEXT\n{tmux_context}"),
        format!(
            "HISTORY LEDGER (chronological; most recent {MAX_HISTORY_LEDGER_MESSAGES} messages)\n{history_ledger}"
        ),
    ]
    .join(SECTION_SEPARATOR)
}

/// Build final tail instructions appended to every model request.
fn render_turn_tail_instructions(
    active_model: &str,
    routing: SnapshotRouting,
    recent_non_default_targets: &[String],
    missing_target_observed: bool,
) -> String {
    let active_route = match routing {
        SnapshotRouting::DefaultSharedPane => "default-shared-pane".to_string(),
        SnapshotRouting::NonDefaultTarget {
            tool_name,
            target_label,
        } => format!("non-default via {tool_name} ({target_label})"),
    };
    let recent_targets = if recent_non_default_targets.is_empty() {
        "none observed".to_string()
    } else {
        recent_non_default_targets.join(" | ")
    };
    let missing_target = if missing_target_observed { "yes" } else { "no" };

    [
        format!(
            "FINAL EXECUTION INSTRUCTIONS (must follow for this request)\nactive_model: {}\nactive_tmux_route: {}\nrecent_non_default_targets: {}\nrecent_missing_target_error: {}",
            active_model, active_route, recent_targets, missing_target
        ),
        concat!(
            "Targeting rules:\n",
            "- Default pane execution: omit `target`, `session`, and `pane`.\n",
            "- Non-default pane execution: pass `session`/`pane` only after managed panes exist.\n",
            "- If a tmux tool returns `tmux target not found`, omit selectors to recover on the default shared pane or create a pane first with `tmux_create_pane`."
        )
        .to_string(),
        concat!(
            "Shared-shell safety rules:\n",
            "- Do NOT run `set -e`, `set -o errexit`, or `setopt errexit` directly in the shared shell.\n",
            "- Do NOT run `exit`, `logout`, or `exec ...` that replaces the parent shell.\n",
            "- If strict mode is needed, isolate it in a subshell (for example `bash -lc 'set -e; ...'`)."
        )
        .to_string(),
    ]
    .join(SECTION_SEPARATOR)
}

/// Build one annotated history ledger for recent persisted messages.
fn render_history_ledger(messages: &[Message], active_model: &str) -> String {
    if messages.is_empty() {
        return "history is empty".to_string();
    }

    let tool_lookup = build_tool_call_lookup(messages);
    let start = messages.len().saturating_sub(MAX_HISTORY_LEDGER_MESSAGES);
    let mut lines = Vec::new();

    for (idx, message) in messages.iter().enumerate().skip(start) {
        let seq = idx + 1;
        match message.role {
            Role::System => {
                lines.push(format!(
                    "{seq:03} actor=system action=instruction detail={}",
                    preview_optional_text(message.content.as_deref(), MAX_LEDGER_DETAIL_CHARS)
                ));
            }
            Role::User => {
                lines.push(format!(
                    "{seq:03} actor=user action=input detail={}",
                    preview_optional_text(message.content.as_deref(), MAX_LEDGER_DETAIL_CHARS)
                ));
            }
            Role::Assistant => {
                let content = message.content.as_deref().unwrap_or("").trim();
                if !content.is_empty() {
                    lines.push(format!(
                        "{seq:03} actor=buddy(model={active_model}) action=reply detail={}",
                        preview_text(content, MAX_LEDGER_DETAIL_CHARS)
                    ));
                }
                if let Some(tool_calls) = message.tool_calls.as_ref() {
                    for call in tool_calls {
                        let meta = tool_call_ledger_meta(call);
                        lines.push(clip_ledger_line(format!(
                            "{seq:03} actor=buddy(model={active_model}) action=tool_call tool={} where={} approval={} detail={}",
                            meta.tool_name,
                            meta.target_label,
                            meta.approval_summary,
                            meta.action_summary,
                        )));
                    }
                }
            }
            Role::Tool => {
                let call_id = message
                    .tool_call_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .unwrap_or("unknown");
                let linked = tool_lookup.get(call_id);
                let tool_name = linked
                    .map(|meta| meta.tool_name.as_str())
                    .unwrap_or("unknown");
                let where_label = linked
                    .map(|meta| meta.target_label.as_str())
                    .unwrap_or("unknown");
                let (status, detail) = summarize_tool_result_for_ledger(message.content.as_deref());
                let approval_outcome =
                    approval_outcome_from_tool_result(message.content.as_deref());
                lines.push(clip_ledger_line(format!(
                    "{seq:03} actor=tool action=result call_id={call_id} tool={tool_name} where={where_label} status={} approval={} detail={detail}",
                    status.as_str(),
                    approval_outcome,
                )));
            }
        }
    }

    if lines.is_empty() {
        "no recent messages to summarize".to_string()
    } else {
        lines.join("\n")
    }
}

/// Build a lookup from `tool_call_id` to parsed metadata for result annotation.
fn build_tool_call_lookup(messages: &[Message]) -> BTreeMap<String, ToolCallLedgerMeta> {
    let mut lookup = BTreeMap::new();
    for message in messages {
        let Some(tool_calls) = message.tool_calls.as_ref() else {
            continue;
        };
        for call in tool_calls {
            lookup.insert(call.id.clone(), tool_call_ledger_meta(call));
        }
    }
    lookup
}

/// Parse one assistant tool call into deterministic ledger metadata.
fn tool_call_ledger_meta(call: &ToolCall) -> ToolCallLedgerMeta {
    let args = parse_arguments_object(&call.function.arguments);
    ToolCallLedgerMeta {
        tool_name: call.function.name.clone(),
        target_label: tool_call_target_label(&call.function.name, args.as_ref()),
        action_summary: tool_call_action_summary(
            &call.function.name,
            args.as_ref(),
            &call.function.arguments,
        ),
        approval_summary: tool_call_approval_summary(args.as_ref()),
    }
}

/// Parse JSON tool arguments into an object map when possible.
fn parse_arguments_object(arguments_json: &str) -> Option<serde_json::Map<String, Value>> {
    serde_json::from_str::<Value>(arguments_json)
        .ok()?
        .as_object()
        .cloned()
}

/// Build target/location label for one tool call.
fn tool_call_target_label(
    tool_name: &str,
    args: Option<&serde_json::Map<String, Value>>,
) -> String {
    match tool_name {
        "run_shell" | "tmux_capture_pane" | "tmux_send_keys" => {
            let Some(args) = args else {
                return "default-shared-pane".to_string();
            };

            if let Some(raw_target) = args.get("target").and_then(Value::as_str) {
                let target = raw_target.trim();
                if !target.is_empty() && !is_default_target_alias(target) {
                    return format!("target={target}");
                }
            }

            let session = args
                .get("session")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let pane = args
                .get("pane")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "shared");
            match (session, pane) {
                (Some(session), Some(pane)) => format!("session={session} pane={pane}"),
                (Some(session), None) => format!("session={session}"),
                (None, Some(pane)) => format!("pane={pane}"),
                (None, None) => "default-shared-pane".to_string(),
            }
        }
        "tmux_create_session" | "tmux_kill_session" => args
            .and_then(|obj| obj.get("session"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|session| format!("session={session}"))
            .unwrap_or_else(|| "session=<unspecified>".to_string()),
        "tmux_create_pane" | "tmux_kill_pane" => {
            let session = args
                .and_then(|obj| obj.get("session"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("<default>");
            let pane = args
                .and_then(|obj| obj.get("pane"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("<unspecified>");
            format!("session={session} pane={pane}")
        }
        _ => "n/a".to_string(),
    }
}

/// Build action payload summary for one tool call.
fn tool_call_action_summary(
    tool_name: &str,
    args: Option<&serde_json::Map<String, Value>>,
    raw_arguments: &str,
) -> String {
    let Some(args) = args else {
        return format!(
            "args={}",
            preview_text(raw_arguments, MAX_ARGUMENT_SUMMARY_CHARS)
        );
    };

    match tool_name {
        "run_shell" => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .map(|raw| preview_text(raw, MAX_ARGUMENT_SUMMARY_CHARS))
                .unwrap_or_else(|| "<missing>".to_string());
            let wait = args
                .get("wait")
                .map(wait_value_label)
                .unwrap_or_else(|| "true(default)".to_string());
            format!("command=\"{command}\"; wait={wait}")
        }
        "tmux_send_keys" => {
            let keys = args
                .get("keys")
                .and_then(Value::as_array)
                .map(|items| {
                    let joined = items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(",");
                    if joined.is_empty() {
                        "[]".to_string()
                    } else {
                        format!("[{joined}]")
                    }
                })
                .unwrap_or_else(|| "[]".to_string());
            let literal = args
                .get("literal_text")
                .and_then(Value::as_str)
                .map(|raw| format!("\"{}\"", preview_text(raw, MAX_ARGUMENT_SUMMARY_CHARS)))
                .unwrap_or_else(|| "<none>".to_string());
            let enter = args.get("enter").and_then(Value::as_bool).unwrap_or(false);
            let delay = args
                .get("delay")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("none");
            format!("keys={keys}; literal={literal}; enter={enter}; delay={delay}")
        }
        "tmux_capture_pane" => {
            let start = args
                .get("start")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("tmux-default");
            let end = args
                .get("end")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("tmux-default");
            let delay = args
                .get("delay")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("none");
            format!("capture_range_start={start}; capture_range_end={end}; delay={delay}")
        }
        "tmux_create_session" | "tmux_kill_session" => {
            let session = args
                .get("session")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("<missing>");
            format!("session={session}")
        }
        "tmux_create_pane" | "tmux_kill_pane" => {
            let session = args
                .get("session")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("<default>");
            let pane = args
                .get("pane")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("<missing>");
            format!("session={session}; pane={pane}")
        }
        _ => {
            let compact = serde_json::to_string(args).unwrap_or_else(|_| raw_arguments.to_string());
            format!(
                "args={}",
                preview_text(&compact, MAX_ARGUMENT_SUMMARY_CHARS)
            )
        }
    }
}

/// Build approval metadata summary from tool arguments.
fn tool_call_approval_summary(args: Option<&serde_json::Map<String, Value>>) -> String {
    let Some(args) = args else {
        return "none".to_string();
    };

    let risk = args.get("risk").and_then(Value::as_str).map(str::trim);
    let mutation = args.get("mutation").and_then(Value::as_bool);
    let privesc = args.get("privesc").and_then(Value::as_bool);
    let why = args
        .get("why")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| preview_text(raw, MAX_ARGUMENT_SUMMARY_CHARS));

    if risk.is_none() && mutation.is_none() && privesc.is_none() && why.is_none() {
        return "none".to_string();
    }

    format!(
        "risk={}; mutation={}; privesc={}; why=\"{}\"",
        risk.unwrap_or("unspecified"),
        mutation
            .map(|v| if v { "true" } else { "false" })
            .unwrap_or("unspecified"),
        privesc
            .map(|v| if v { "true" } else { "false" })
            .unwrap_or("unspecified"),
        why.unwrap_or_else(|| "unspecified".to_string())
    )
}

/// Render bool/string wait argument values with stable textual forms.
fn wait_value_label(value: &Value) -> String {
    if let Some(boolean) = value.as_bool() {
        return if boolean {
            "true".to_string()
        } else {
            "false".to_string()
        };
    }
    if let Some(secs) = value.as_u64() {
        return format!("{secs}s");
    }
    if let Some(raw) = value.as_str() {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "unknown".to_string()
}

/// Summarize one tool-result payload into status+detail for ledger lines.
fn summarize_tool_result_for_ledger(content: Option<&str>) -> (LedgerStatus, String) {
    let raw = content.unwrap_or("").trim();
    if raw.is_empty() {
        return (LedgerStatus::Info, "empty tool result content".to_string());
    }
    if let Some(err) = raw.strip_prefix("Tool error:") {
        return (
            LedgerStatus::Failure,
            preview_text(err.trim(), MAX_LEDGER_DETAIL_CHARS),
        );
    }

    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return summarize_tool_result_text(raw);
    };

    let timestamp_prefix = value
        .get("harness_timestamp")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("unix_millis"))
        .and_then(Value::as_u64)
        .map(|ts| format!("harness_unix_ms={ts}; "))
        .unwrap_or_default();

    let payload = value.get("result").unwrap_or(&value);
    if let Some(text) = payload.as_str() {
        let (status, detail) = summarize_tool_result_text(text);
        return (status, format!("{timestamp_prefix}{detail}"));
    }

    if let Some(obj) = payload.as_object() {
        if let Some(exit_code) = obj.get("exit_code").and_then(Value::as_i64) {
            let status = if exit_code == 0 {
                LedgerStatus::Success
            } else {
                LedgerStatus::Failure
            };
            let stdout = obj
                .get("stdout")
                .and_then(Value::as_str)
                .map(|s| preview_text(s, MAX_LEDGER_DETAIL_CHARS))
                .unwrap_or_else(|| "<empty>".to_string());
            let stderr = obj
                .get("stderr")
                .and_then(Value::as_str)
                .map(|s| preview_text(s, MAX_LEDGER_DETAIL_CHARS))
                .unwrap_or_else(|| "<empty>".to_string());
            let notices = obj
                .get("notices")
                .and_then(Value::as_array)
                .map(|items| {
                    if items.is_empty() {
                        "none".to_string()
                    } else {
                        let first = items
                            .first()
                            .and_then(Value::as_str)
                            .map(|s| preview_text(s, MAX_LEDGER_DETAIL_CHARS))
                            .unwrap_or_else(|| "<non-string notice>".to_string());
                        format!("count={}; first={first}", items.len())
                    }
                })
                .unwrap_or_else(|| "none".to_string());
            return (
                status,
                format!(
                    "{timestamp_prefix}exit_code={exit_code}; stdout={stdout}; stderr={stderr}; notices={notices}"
                ),
            );
        }

        if let Some(session) = obj.get("session").and_then(Value::as_str) {
            let pane = obj
                .get("pane_title")
                .or_else(|| obj.get("pane_id"))
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let state = obj.get("created").and_then(Value::as_bool).map(|created| {
                if created {
                    "created"
                } else {
                    "reused"
                }
            });
            let mut detail = format!(
                "{timestamp_prefix}session={}; pane={}",
                preview_text(session, MAX_LEDGER_DETAIL_CHARS),
                preview_text(pane, MAX_LEDGER_DETAIL_CHARS)
            );
            if let Some(state) = state {
                detail.push_str(&format!("; state={state}"));
            }
            return (LedgerStatus::Success, detail);
        }
    }

    let compact = serde_json::to_string(payload).unwrap_or_else(|_| payload.to_string());
    (
        LedgerStatus::Info,
        format!(
            "{timestamp_prefix}{}",
            preview_text(&compact, MAX_LEDGER_DETAIL_CHARS)
        ),
    )
}

/// Infer approval disposition from one tool-result payload.
fn approval_outcome_from_tool_result(content: Option<&str>) -> &'static str {
    let lowered = content.unwrap_or("").to_ascii_lowercase();
    if lowered.contains("denied by user") {
        "denied"
    } else {
        "passed_or_not_required"
    }
}

/// Summarize plain text tool payloads into status+detail.
fn summarize_tool_result_text(raw: &str) -> (LedgerStatus, String) {
    let detail = preview_text(raw, MAX_LEDGER_DETAIL_CHARS);
    let lowered = raw.to_ascii_lowercase();

    let status = if lowered.contains("denied by user")
        || lowered.contains("tool error")
        || lowered.contains(" failed")
        || lowered.starts_with("failed")
        || lowered.contains("execution failed")
    {
        LedgerStatus::Failure
    } else if lowered.contains("success")
        || lowered.contains("completed")
        || lowered.contains("created")
        || lowered.contains("killed")
    {
        LedgerStatus::Success
    } else {
        LedgerStatus::Info
    };

    (status, detail)
}

/// Determine tmux context routing for the next request.
fn resolve_tmux_snapshot_routing(messages: &[Message]) -> SnapshotRouting {
    if recent_tmux_missing_target_error(messages) {
        return SnapshotRouting::DefaultSharedPane;
    }
    messages
        .iter()
        .rev()
        .find_map(resolve_tool_call_routing)
        .unwrap_or(SnapshotRouting::DefaultSharedPane)
}

/// Keep default-shared screenshot injection enabled when recent tool results
/// indicate non-default tmux targets were missing.
fn recent_tmux_missing_target_error(messages: &[Message]) -> bool {
    messages.iter().rev().any(|message| {
        if message.role != Role::Tool {
            return false;
        }
        message.content.as_ref().is_some_and(|content| {
            content
                .to_ascii_lowercase()
                .contains("tmux target not found")
        })
    })
}

/// Collect recent explicit non-default tmux routes from assistant tool calls.
fn recent_non_default_tmux_targets(messages: &[Message]) -> Vec<String> {
    let mut routes = Vec::new();
    let mut seen = BTreeSet::new();

    for message in messages.iter().rev() {
        let Some(tool_calls) = message.tool_calls.as_ref() else {
            continue;
        };
        for call in tool_calls.iter().rev() {
            let Some(label) = non_default_tmux_target_label(&call.function.arguments) else {
                continue;
            };
            let entry = format!("{} ({})", label, call.function.name);
            if seen.insert(entry.clone()) {
                routes.push(preview_text(&entry, MAX_LEDGER_DETAIL_CHARS));
                if routes.len() >= MAX_RECENT_NON_DEFAULT_TARGETS {
                    return routes;
                }
            }
        }
    }

    routes
}

/// Routing mode for this request's tmux context block.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SnapshotRouting {
    /// Use the default managed shared pane and inject a fresh screenshot.
    DefaultSharedPane,
    /// Do not inject default-pane screenshot because model explicitly targeted
    /// a non-default session/pane in a recent tmux-aware tool call.
    NonDefaultTarget {
        tool_name: String,
        target_label: String,
    },
}

/// Render a stable turn-context block from the current default tmux pane snapshot.
fn render_default_tmux_snapshot_context(snapshot: &str) -> String {
    let mut clipped: String = snapshot.chars().take(MAX_TMUX_SCREENSHOT_CHARS).collect();
    if snapshot.chars().count() > MAX_TMUX_SCREENSHOT_CHARS {
        clipped.push_str("\n...[truncated]");
    }
    render_prompt_template(
        "dynamic_default_tmux_snapshot_context",
        &[("SNAPSHOT", &clipped)],
    )
}

/// Render request context when tmux snapshot capture is unavailable.
fn render_tmux_context_unavailable() -> String {
    render_prompt_template("dynamic_tmux_context_unavailable", &[])
}

/// Render request context when the active tmux target is non-default.
fn render_non_default_tmux_target_context(tool_name: &str, target_label: &str) -> String {
    render_prompt_template(
        "dynamic_non_default_tmux_target_context",
        &[("TOOL_NAME", tool_name), ("TARGET_LABEL", target_label)],
    )
}

/// Resolve routing information from a single message's latest tmux-aware tool call.
fn resolve_tool_call_routing(message: &Message) -> Option<SnapshotRouting> {
    message
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.iter().rev().find_map(routing_from_tool_call))
}

/// Resolve routing mode from one tool call, when it is tmux-aware.
fn routing_from_tool_call(call: &ToolCall) -> Option<SnapshotRouting> {
    match call.function.name.as_str() {
        "tmux_capture_pane" | "tmux_send_keys" | "run_shell" => {}
        _ => return None,
    }
    let Some(target_label) = non_default_tmux_target_label(&call.function.arguments) else {
        return Some(SnapshotRouting::DefaultSharedPane);
    };
    Some(SnapshotRouting::NonDefaultTarget {
        tool_name: call.function.name.clone(),
        target_label,
    })
}

/// Return a human-readable label when arguments target a non-default tmux location.
fn non_default_tmux_target_label(arguments_json: &str) -> Option<String> {
    let Ok(value) = serde_json::from_str::<Value>(arguments_json) else {
        return None;
    };
    let object = value.as_object()?;

    if let Some(raw_target) = object.get("target").and_then(Value::as_str) {
        let target = raw_target.trim();
        if !target.is_empty() && !is_default_target_alias(target) {
            return Some(format!("target={target}"));
        }
    }

    let session = object
        .get("session")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let pane = object
        .get("pane")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "shared");
    match (session, pane) {
        (Some(session), Some(pane)) => Some(format!("session={session} pane={pane}")),
        (Some(session), None) => Some(format!("session={session}")),
        (None, Some(pane)) => Some(format!("pane={pane}")),
        (None, None) => None,
    }
}

fn is_default_target_alias(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "default" | "shared" | "shared-pane" | "default-pane"
    )
}

/// Collapse text to one line and clip it to `max_chars` characters.
fn preview_text(raw: &str, max_chars: usize) -> String {
    let collapsed = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if collapsed.is_empty() {
        return "<empty>".to_string();
    }

    let total_chars = collapsed.chars().count();
    if total_chars <= max_chars {
        return collapsed;
    }

    let mut clipped: String = collapsed.chars().take(max_chars).collect();
    clipped.push_str("...[truncated]");
    clipped
}

/// Render optional text payload with stable empty markers.
fn preview_optional_text(raw: Option<&str>, max_chars: usize) -> String {
    raw.map(|text| preview_text(text, max_chars))
        .unwrap_or_else(|| "<none>".to_string())
}

/// Ensure each ledger line remains bounded for prompt-budget stability.
fn clip_ledger_line(line: String) -> String {
    preview_text(&line, MAX_LEDGER_DETAIL_CHARS * 3)
}

#[cfg(test)]
mod tests {
    use super::{
        non_default_tmux_target_label, render_default_tmux_snapshot_context, render_history_ledger,
        render_non_default_tmux_target_context, render_tmux_context_unavailable,
        render_turn_context_annotation, render_turn_tail_instructions,
        resolve_tmux_snapshot_routing, summarize_tool_result_for_ledger, tool_result_text,
        LedgerStatus, SnapshotRouting,
    };
    use crate::types::{FunctionCall, Message, Role, ToolCall};
    use std::collections::BTreeMap;

    /// Verifies JSON tool envelopes prefer the `result` field text.
    #[test]
    fn tool_result_text_prefers_result_field() {
        let raw = r#"{"result":"hello"}"#;
        assert_eq!(tool_result_text(raw), "hello");
    }

    /// Verifies invalid JSON returns the original raw payload unchanged.
    #[test]
    fn tool_result_text_falls_back_to_raw_payload() {
        let raw = "not json";
        assert_eq!(tool_result_text(raw), "not json");
    }

    /// Verifies oversized snapshots are clipped and marked as truncated.
    #[test]
    fn tmux_snapshot_block_truncates_large_snapshots() {
        let text = "x".repeat(3_000);
        let rendered = render_default_tmux_snapshot_context(&text);
        assert!(rendered.contains("...[truncated]"));
    }

    /// Verifies explicit pane/session targeting returns readable non-default labels.
    #[test]
    fn target_label_detection_marks_non_default() {
        assert_eq!(
            non_default_tmux_target_label(r#"{"session":"build","pane":"worker"}"#).as_deref(),
            Some("session=build pane=worker")
        );
        assert_eq!(
            non_default_tmux_target_label(r#"{"pane":"shared","risk":"low"}"#),
            None
        );
        assert_eq!(
            non_default_tmux_target_label(r#"{"target":"default","risk":"low"}"#),
            None
        );
    }

    /// Verifies routing switches to non-default mode after explicit non-default tmux calls.
    #[test]
    fn snapshot_routing_switches_for_non_default_tmux_target() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: Some(String::new()),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "tmux_capture_pane".to_string(),
                    arguments: r#"{"session":"build","pane":"worker"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
            extra: BTreeMap::new(),
        }];
        assert_eq!(
            resolve_tmux_snapshot_routing(&messages),
            SnapshotRouting::NonDefaultTarget {
                tool_name: "tmux_capture_pane".to_string(),
                target_label: "session=build pane=worker".to_string(),
            }
        );
    }

    /// Verifies non-default context block clearly labels the latest explicit target.
    #[test]
    fn non_default_tmux_target_context_labels_source() {
        let rendered = render_non_default_tmux_target_context("tmux_capture_pane", "pane=worker");
        assert!(rendered.contains("default shared-pane snapshot is intentionally omitted"));
        assert!(rendered.contains("last_tool: tmux_capture_pane"));
        assert!(rendered.contains("last_target: pane=worker"));
    }

    /// Verifies unavailable tmux block keeps explicit section framing.
    #[test]
    fn unavailable_tmux_context_has_section_frame() {
        let rendered = render_tmux_context_unavailable();
        assert!(rendered.contains("tmux context unavailable"));
        assert!(rendered.contains("--"));
    }

    /// Verifies missing-target tool errors force default shared snapshot routing.
    #[test]
    fn snapshot_routing_defaults_after_missing_target_tool_error() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: Some(String::new()),
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "tmux_capture_pane".to_string(),
                        arguments: r#"{"session":"build","pane":"worker"}"#.to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            },
            Message::tool_result(
                "call-1",
                "Tool error: execution failed: failed to resolve managed tmux target: tmux target not found",
            ),
        ];
        assert_eq!(
            resolve_tmux_snapshot_routing(&messages),
            SnapshotRouting::DefaultSharedPane
        );
    }

    /// Verifies result summarizer captures shell exit codes as success/failure.
    #[test]
    fn summarize_tool_result_detects_shell_exit_status() {
        let success = r#"{"harness_timestamp":{"source":"harness","unix_millis":7},"result":{"exit_code":0,"stdout":"ok","stderr":"","notices":[]}}"#;
        let fail = r#"{"result":{"exit_code":2,"stdout":"","stderr":"nope","notices":[]}}"#;

        let success_summary = summarize_tool_result_for_ledger(Some(success));
        let fail_summary = summarize_tool_result_for_ledger(Some(fail));

        assert_eq!(success_summary.0, LedgerStatus::Success);
        assert!(success_summary.1.contains("exit_code=0"));
        assert_eq!(fail_summary.0, LedgerStatus::Failure);
        assert!(fail_summary.1.contains("exit_code=2"));
    }

    /// Verifies tail reminders keep shell safety directives explicit.
    #[test]
    fn tail_instructions_include_shell_safety_rules() {
        let rendered = render_turn_tail_instructions(
            "gpt-5.3-codex",
            SnapshotRouting::DefaultSharedPane,
            &["session=build pane=worker (run_shell)".to_string()],
            true,
        );
        assert!(rendered.contains("FINAL EXECUTION INSTRUCTIONS"));
        assert!(rendered.contains("set -e"));
        assert!(rendered.contains("active_model: gpt-5.3-codex"));
        assert!(rendered.contains("recent_missing_target_error: yes"));
        assert!(rendered.contains("--"));
    }

    /// Verifies context annotation uses explicit section separators.
    #[test]
    fn context_annotation_renders_with_section_separators() {
        let rendered = render_turn_context_annotation(
            "gpt-5.3-codex",
            12,
            "tmux-context-block",
            "history-ledger-block",
        );
        assert!(rendered.contains("REQUEST CONTEXT ANNOTATION"));
        assert!(rendered.contains("TMUX CONTEXT"));
        assert!(rendered.contains("HISTORY LEDGER"));
        assert!(rendered.contains("\n--\n"));
    }

    /// Verifies ledger annotation includes actor/action + command/result/approval fields.
    #[test]
    fn history_ledger_annotates_command_result_and_approval() {
        let messages = vec![
            Message::user("list files"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "run_shell".to_string(),
                        arguments: r#"{"command":"ls -la","risk":"low","mutation":false,"privesc":false,"why":"inspect files"}"#.to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            },
            Message::tool_result(
                "call-1",
                r#"{"harness_timestamp":{"source":"harness","unix_millis":42},"result":{"exit_code":0,"stdout":"ok","stderr":"","notices":[]}}"#,
            ),
        ];

        let ledger = render_history_ledger(&messages, "gpt-5.3-codex");
        assert!(ledger.contains("actor=user action=input"));
        assert!(ledger.contains("actor=buddy(model=gpt-5.3-codex) action=tool_call tool=run_shell"));
        assert!(ledger.contains("command=\"ls -la\""));
        assert!(ledger.contains("approval=risk=low"));
        assert!(ledger.contains("actor=tool action=result"));
        assert!(ledger.contains("status=success"));
        assert!(ledger.contains("approval=passed_or_not_required"));
    }
}
