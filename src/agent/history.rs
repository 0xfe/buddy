//! Conversation history compaction.
//!
//! The agent uses this module to collapse older turns into a compact system
//! summary when context pressure is high or when `/session compact` is invoked.

use super::{normalization::sanitize_conversation_history, Agent};
use crate::tokens::TokenTracker;
use crate::types::{Message, Role};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info_span};

/// Minimum number of most-recent turns preserved during compaction.
const CONTEXT_COMPACT_KEEP_RECENT_TURNS: usize = 3;
/// Number of most-recent failed tool operations retained verbatim.
const RETAIN_FAILED_TOOL_OPERATIONS: usize = 3;
/// Max number of summary lines generated from removed messages.
const MAX_COMPACT_SUMMARY_LINES: usize = 24;
/// Prefix marker used to identify synthetic compaction summary messages.
pub(super) const COMPACT_SUMMARY_PREFIX: &str = "[buddy compact summary]";

/// Details about one history-compaction operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCompactionReport {
    /// Estimated token count before compaction.
    pub estimated_before: u64,
    /// Estimated token count after compaction.
    pub estimated_after: u64,
    /// Number of individual messages removed from history.
    pub removed_messages: usize,
    /// Number of turn groups removed from history.
    pub removed_turns: usize,
}

/// Half-open index range for one compaction unit in `messages`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompactionUnit {
    /// Inclusive start index of the unit.
    start: usize,
    /// Exclusive end index of the unit.
    end: usize,
    /// Number of failed tool operations present in this unit.
    failed_tool_operations: usize,
}

/// Structured status values emitted in compact summary lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SummaryStatus {
    /// Informational line (non-success/failure operation).
    Info,
    /// Operation completed successfully.
    Success,
    /// Operation failed.
    Failure,
}

impl SummaryStatus {
    /// Return lowercase serialized status token used in summary lines.
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// One structured compaction summary line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SummaryEntry {
    /// Operation name.
    operation: String,
    /// Outcome status.
    status: SummaryStatus,
    /// Key detail/outcome/error text.
    detail: String,
}

impl Agent {
    /// Compact older conversation turns into a synthesized summary message.
    ///
    /// This is used by `/compact` and can also be triggered automatically
    /// before request submission when context pressure is high.
    pub fn compact_history(&mut self) -> Option<HistoryCompactionReport> {
        compact_history_with_budget(
            &mut self.messages,
            self.tracker.context_limit,
            super::CONTEXT_MANUAL_COMPACT_TARGET_FRACTION,
            true,
        )
    }
}

/// Compact message history to a target context fraction.
///
/// `force=true` removes old turns aggressively (used for manual compaction).
/// `force=false` compacts only when current estimate exceeds `target_fraction`.
pub(super) fn compact_history_with_budget(
    messages: &mut Vec<Message>,
    context_limit: usize,
    target_fraction: f64,
    force: bool,
) -> Option<HistoryCompactionReport> {
    let _compaction_span = info_span!(
        "agent.history.compaction",
        context_limit,
        target_fraction,
        force,
        message_count_before = messages.len() as u64
    )
    .entered();
    // Guard rails: if we cannot estimate a useful budget, skip compaction.
    if context_limit == 0 || messages.is_empty() {
        debug!("skipping compaction: empty history or zero context limit");
        return None;
    }

    // Repair malformed assistant/tool message pairs before evaluating budgets
    // so compaction starts from protocol-valid history.
    let pre_repair = sanitize_conversation_history(messages);
    if pre_repair.dropped_orphan_tool_results > 0 || pre_repair.repaired_unmatched_tool_calls > 0 {
        debug!(
            dropped_orphan_tool_results = pre_repair.dropped_orphan_tool_results,
            repaired_unmatched_tool_calls = pre_repair.repaired_unmatched_tool_calls,
            "repaired malformed tool history before compaction"
        );
    }

    let estimated_before = TokenTracker::estimate_messages(messages);
    let target_tokens = ((context_limit as f64) * target_fraction).floor().max(1.0) as usize;
    if !force && estimated_before <= target_tokens {
        debug!(
            estimated_before,
            target_tokens, "skipping automatic compaction; target already satisfied"
        );
        return None;
    }

    // Reuse a previous compact summary if one is already present directly
    // after leading system prompts. This prevents stacked summary messages.
    let mut insertion_index = leading_system_count(messages);
    let mut previous_summary = None;
    if insertion_index > 0
        && messages
            .get(insertion_index - 1)
            .is_some_and(is_compact_summary_message)
    {
        if let Some(removed) = messages.get(insertion_index - 1).cloned() {
            previous_summary = removed.content;
        }
        messages.remove(insertion_index - 1);
        insertion_index -= 1;
    }

    let mut removed_messages = Vec::new();
    let mut removed_turns = 0usize;

    loop {
        let estimated_now = TokenTracker::estimate_messages(messages);
        let units = collect_compaction_units(messages, insertion_index);
        if units.len() <= CONTEXT_COMPACT_KEEP_RECENT_TURNS {
            break;
        }

        // Forced mode trims while more than the recent-turn window remains.
        // Automatic mode trims only while usage exceeds target budget.
        let should_remove = if force {
            units.len() > CONTEXT_COMPACT_KEEP_RECENT_TURNS || estimated_now > target_tokens
        } else {
            estimated_now > target_tokens
        };
        if !should_remove {
            break;
        }

        // Protect the most recent failed tool operations so operators keep
        // exact error payloads after compaction.
        let protected = protected_failed_units(&units);
        let oldest_removal_boundary = units
            .len()
            .saturating_sub(CONTEXT_COMPACT_KEEP_RECENT_TURNS);
        let Some(removal_idx) = (0..oldest_removal_boundary).find(|idx| !protected.contains(idx))
        else {
            debug!(
                protected_units = protected.len(),
                "cannot remove additional units because only protected/recent units remain"
            );
            break;
        };

        let unit = units[removal_idx];
        removed_messages.extend(messages.drain(unit.start..unit.end));
        removed_turns = removed_turns.saturating_add(1);
    }

    if removed_messages.is_empty() && previous_summary.is_none() {
        debug!("compaction produced no removable content");
        return None;
    }

    let summary = build_compact_summary(previous_summary.as_deref(), &removed_messages);
    messages.insert(insertion_index, Message::system(summary));

    let mut estimated_after = TokenTracker::estimate_messages(messages);
    // If the generated summary does not reduce estimated size, fall back to a
    // minimal summary and then remove it entirely if still not helpful.
    if estimated_after >= estimated_before {
        messages[insertion_index] = Message::system(format!(
            "{COMPACT_SUMMARY_PREFIX}\n- op=summary; status=info; detail=Older turns were compacted."
        ));
        estimated_after = TokenTracker::estimate_messages(messages);
        if estimated_after >= estimated_before {
            messages.remove(insertion_index);
        }
    }

    // Post-compaction repair pass removes any stale orphan tool metadata if
    // malformed turns were encountered.
    let post_repair = sanitize_conversation_history(messages);
    if post_repair.dropped_orphan_tool_results > 0 || post_repair.repaired_unmatched_tool_calls > 0
    {
        debug!(
            dropped_orphan_tool_results = post_repair.dropped_orphan_tool_results,
            repaired_unmatched_tool_calls = post_repair.repaired_unmatched_tool_calls,
            "repaired malformed tool history after compaction"
        );
    }
    estimated_after = TokenTracker::estimate_messages(messages);

    Some(HistoryCompactionReport {
        estimated_before: estimated_before as u64,
        estimated_after: estimated_after as u64,
        removed_messages: removed_messages.len(),
        removed_turns,
    })
    .map(|report| {
        debug!(
            estimated_before = report.estimated_before,
            estimated_after = report.estimated_after,
            removed_messages = report.removed_messages,
            removed_turns = report.removed_turns,
            "history compaction completed"
        );
        report
    })
}

/// Count contiguous leading `system` messages.
fn leading_system_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .take_while(|message| message.role == Role::System)
        .count()
}

/// Split message history into compaction units starting from `start_index`.
///
/// Units are turn-like groups bounded by the next `user` message only when
/// there are no pending tool calls, ensuring assistant tool-call/result pairs
/// stay atomic.
fn collect_compaction_units(messages: &[Message], start_index: usize) -> Vec<CompactionUnit> {
    let mut units = Vec::<CompactionUnit>::new();
    let mut unit_start: Option<usize> = None;
    let mut pending_tool_calls = HashSet::<String>::new();

    for idx in start_index..messages.len() {
        if unit_start.is_none() {
            unit_start = Some(idx);
        }
        if let Some(calls) = messages[idx].tool_calls.as_ref() {
            for call in calls {
                pending_tool_calls.insert(call.id.clone());
            }
        }
        if messages[idx].role == Role::Tool {
            if let Some(call_id) = messages[idx].tool_call_id.as_ref() {
                pending_tool_calls.remove(call_id);
            }
        }

        let next_is_user = messages
            .get(idx + 1)
            .is_some_and(|next| next.role == Role::User);
        let at_end = idx + 1 == messages.len();
        let should_close = at_end || (next_is_user && pending_tool_calls.is_empty());
        if !should_close {
            continue;
        }

        let start = unit_start.take().expect("unit start should exist");
        let end = idx + 1;
        units.push(CompactionUnit {
            start,
            end,
            failed_tool_operations: count_failed_tool_operations(&messages[start..end]),
        });
        pending_tool_calls.clear();
    }

    units
}

/// Return compaction-unit indices that should not be removed because they carry
/// one of the most recent failed tool operations.
fn protected_failed_units(units: &[CompactionUnit]) -> HashSet<usize> {
    if RETAIN_FAILED_TOOL_OPERATIONS == 0 {
        return HashSet::new();
    }
    let mut protected = HashSet::<usize>::new();
    let mut retained = 0usize;
    for (idx, unit) in units.iter().enumerate().rev() {
        if unit.failed_tool_operations == 0 {
            continue;
        }
        protected.insert(idx);
        retained = retained.saturating_add(unit.failed_tool_operations);
        if retained >= RETAIN_FAILED_TOOL_OPERATIONS {
            break;
        }
    }
    protected
}

/// Return true if message is a synthetic compaction summary.
fn is_compact_summary_message(message: &Message) -> bool {
    message.role == Role::System
        && message
            .content
            .as_deref()
            .is_some_and(|text| text.starts_with(COMPACT_SUMMARY_PREFIX))
}

/// Build a new summary text from removed messages and prior summary body.
fn build_compact_summary(previous_summary: Option<&str>, removed_messages: &[Message]) -> String {
    let mut lines = Vec::<String>::new();
    lines.push(COMPACT_SUMMARY_PREFIX.to_string());
    lines.push(
        "- op=summary; status=info; detail=Older turns were compacted to preserve room for newer context."
            .to_string(),
    );

    if let Some(previous) = previous_summary.and_then(compact_summary_body) {
        if !previous.is_empty() {
            lines.push(format!(
                "- op=summary.previous; status=info; detail={}",
                truncate_summary_preview(&previous)
            ));
        }
    }

    let mut emitted = 0usize;
    for entry in compact_summary_entries(removed_messages) {
        if emitted >= MAX_COMPACT_SUMMARY_LINES {
            break;
        }
        lines.push(format_summary_entry(&entry));
        emitted += 1;
    }

    if emitted == 0 {
        lines.push(
            "- op=summary; status=info; detail=No detailed entries were available from removed messages."
                .to_string(),
        );
    }
    if removed_messages.len() > emitted {
        lines.push(format!(
            "- op=summary; status=info; detail={} additional compacted message(s) omitted.",
            removed_messages.len() - emitted
        ));
    }

    lines.join("\n")
}

/// Extract and normalize the body section from a prior summary message.
fn compact_summary_body(summary: &str) -> Option<String> {
    let mut lines = summary.lines();
    let first = lines.next()?.trim();
    if first != COMPACT_SUMMARY_PREFIX {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join(" ");
    let body = body.trim();
    if body.is_empty() {
        None
    } else {
        Some(truncate_summary_preview(body))
    }
}

/// Build structured summary entries from removed messages.
fn compact_summary_entries(removed_messages: &[Message]) -> Vec<SummaryEntry> {
    let mut entries = Vec::<SummaryEntry>::new();
    let mut tool_name_by_id = HashMap::<String, String>::new();

    for message in removed_messages {
        match message.role {
            Role::System => {}
            Role::User => {
                if let Some(content) = message.content.as_deref() {
                    entries.push(SummaryEntry {
                        operation: "user.input".to_string(),
                        status: SummaryStatus::Info,
                        detail: truncate_summary_preview(content),
                    });
                }
            }
            Role::Assistant => {
                if let Some(content) = message.content.as_deref() {
                    if !content.trim().is_empty() {
                        entries.push(SummaryEntry {
                            operation: "assistant.output".to_string(),
                            status: SummaryStatus::Info,
                            detail: truncate_summary_preview(content),
                        });
                    }
                }
                if let Some(tool_calls) = message.tool_calls.as_ref() {
                    for call in tool_calls {
                        tool_name_by_id.insert(call.id.clone(), call.function.name.clone());
                        entries.push(SummaryEntry {
                            operation: format!("tool.{}.request", call.function.name),
                            status: SummaryStatus::Info,
                            detail: truncate_summary_preview(&format!(
                                "tool call requested (id={})",
                                call.id
                            )),
                        });
                    }
                }
            }
            Role::Tool => {
                let call_id = message
                    .tool_call_id
                    .as_deref()
                    .unwrap_or("<unknown>")
                    .to_string();
                let tool_name = tool_name_by_id
                    .get(&call_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                let (status, detail) = summarize_tool_result(message.content.as_deref());
                entries.push(SummaryEntry {
                    operation: format!("tool.{tool_name}.result"),
                    status,
                    detail,
                });
            }
        }
    }

    entries
}

/// Convert one structured summary entry into its serialized line format.
fn format_summary_entry(entry: &SummaryEntry) -> String {
    format!(
        "- op={}; status={}; detail={}",
        entry.operation,
        entry.status.as_str(),
        truncate_summary_preview(&entry.detail)
    )
}

/// Count failed tool-result messages in a slice.
fn count_failed_tool_operations(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .filter(|message| {
            summarize_tool_result(message.content.as_deref()).0 == SummaryStatus::Failure
        })
        .count()
}

/// Summarize one tool result payload into `(status, key detail)` for summaries.
fn summarize_tool_result(content: Option<&str>) -> (SummaryStatus, String) {
    let Some(raw) = content.map(str::trim) else {
        return (
            SummaryStatus::Failure,
            "missing tool result content".to_string(),
        );
    };
    if raw.is_empty() {
        return (
            SummaryStatus::Failure,
            "empty tool result content".to_string(),
        );
    }

    // Parse JSON envelope/object payloads first.
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        let payload = value.get("result").unwrap_or(&value);
        if let Some(summary) = summarize_tool_result_value(payload, raw) {
            return summary;
        }
    }

    summarize_tool_result_text(raw)
}

/// Summarize tool result payloads represented as JSON values.
fn summarize_tool_result_value(
    value: &Value,
    raw_fallback: &str,
) -> Option<(SummaryStatus, String)> {
    if let Some(text) = value.as_str() {
        return Some(summarize_tool_result_text(text));
    }
    if let Some(obj) = value.as_object() {
        if let Some(exit_code) = obj.get("exit_code").and_then(Value::as_i64) {
            let stdout = obj
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let stderr = obj
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if exit_code == 0 {
                let detail = first_non_empty_line(stdout)
                    .unwrap_or_else(|| "shell command completed successfully".to_string());
                return Some((SummaryStatus::Success, detail));
            }
            let detail = first_non_empty_line(stderr)
                .or_else(|| first_non_empty_line(stdout))
                .unwrap_or_else(|| format!("shell command failed with exit code {exit_code}"));
            return Some((SummaryStatus::Failure, detail));
        }

        if let Some(ok) = obj.get("ok").and_then(Value::as_bool) {
            let detail = obj
                .get("error")
                .and_then(json_text)
                .or_else(|| obj.get("message").and_then(json_text))
                .or_else(|| obj.get("detail").and_then(json_text))
                .unwrap_or_else(|| {
                    if ok {
                        "operation completed".to_string()
                    } else {
                        "operation reported failure".to_string()
                    }
                });
            return Some((
                if ok {
                    SummaryStatus::Success
                } else {
                    SummaryStatus::Failure
                },
                detail,
            ));
        }

        if let Some(err) = obj
            .get("error")
            .and_then(json_text)
            .or_else(|| obj.get("errors").and_then(json_text))
        {
            return Some((SummaryStatus::Failure, err));
        }

        if let Some(message) = obj
            .get("message")
            .and_then(json_text)
            .or_else(|| obj.get("detail").and_then(json_text))
            .or_else(|| obj.get("output").and_then(json_text))
        {
            return Some((SummaryStatus::Success, message));
        }
    }
    if let Some(text) = json_text(value) {
        return Some(summarize_tool_result_text(&text));
    }
    Some(summarize_tool_result_text(raw_fallback))
}

/// Summarize plain-text tool output for structured compaction entries.
fn summarize_tool_result_text(text: &str) -> (SummaryStatus, String) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (
            SummaryStatus::Failure,
            "empty tool result content".to_string(),
        );
    }
    if let Some(err) = trimmed.strip_prefix("Tool error:") {
        let detail = err.trim();
        return (
            SummaryStatus::Failure,
            if detail.is_empty() {
                "tool error reported".to_string()
            } else {
                detail.to_string()
            },
        );
    }
    if let Some((status, detail)) = summarize_legacy_shell_payload(trimmed) {
        return (status, detail);
    }
    (SummaryStatus::Success, trimmed.to_string())
}

/// Summarize the historical run-shell plaintext payload format.
fn summarize_legacy_shell_payload(text: &str) -> Option<(SummaryStatus, String)> {
    let (exit_line, remainder) = text.split_once("\nstdout:\n")?;
    let exit_code = exit_line
        .trim()
        .strip_prefix("exit code: ")?
        .trim()
        .parse::<i64>()
        .ok()?;
    let (stdout, stderr) = remainder
        .split_once("\nstderr:\n")
        .unwrap_or((remainder, ""));
    if exit_code == 0 {
        let detail = first_non_empty_line(stdout)
            .unwrap_or_else(|| "shell command completed successfully".to_string());
        Some((SummaryStatus::Success, detail))
    } else {
        let detail = first_non_empty_line(stderr)
            .or_else(|| first_non_empty_line(stdout))
            .unwrap_or_else(|| format!("shell command failed with exit code {exit_code}"));
        Some((SummaryStatus::Failure, detail))
    }
}

/// Extract one concise text representation from a JSON value.
fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(json_text)
                .collect::<Vec<_>>()
                .join("; ");
            (!text.trim().is_empty()).then_some(text)
        }
        Value::Object(map) => map
            .get("message")
            .and_then(json_text)
            .or_else(|| map.get("detail").and_then(json_text))
            .or_else(|| map.get("error").and_then(json_text))
            .or_else(|| Some(truncate_summary_preview(&value.to_string()))),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(n) => Some(n.to_string()),
    }
}

/// Return the first non-empty line from multiline text.
fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_string())
    })
}

/// Clip summary preview text so compaction output stays bounded.
fn truncate_summary_preview(text: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 180;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = compact.trim();
    if trimmed.chars().count() <= MAX_PREVIEW_CHARS {
        return trimmed.to_string();
    }
    let prefix = trimmed
        .chars()
        .take(MAX_PREVIEW_CHARS.saturating_sub(3))
        .collect::<String>();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionCall, ToolCall};
    use std::collections::{BTreeMap, HashSet};

    /// Build an assistant tool-call message fixture.
    fn assistant_with_tool_call(call_id: &str, name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: call_id.to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
            extra: BTreeMap::new(),
        }
    }

    /// Build a tool result message fixture.
    fn tool_result(call_id: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
            name: None,
            extra: BTreeMap::new(),
        }
    }

    /// Assert that every tool result maps to one assistant-declared tool call.
    fn assert_tool_history_integrity(messages: &[Message]) {
        let mut pending = HashSet::<String>::new();
        for message in messages {
            if let Some(tool_calls) = message.tool_calls.as_ref() {
                for call in tool_calls {
                    pending.insert(call.id.clone());
                }
            }
            if message.role == Role::Tool {
                let id = message
                    .tool_call_id
                    .as_ref()
                    .expect("tool message should carry tool_call_id");
                assert!(
                    pending.remove(id),
                    "tool result with id `{id}` had no matching assistant tool call"
                );
            }
            if message.role == Role::User {
                assert!(
                    pending.is_empty(),
                    "pending tool calls crossed a user-turn boundary: {pending:?}"
                );
            }
        }
    }

    #[test]
    fn compact_history_preserves_tool_pair_integrity() {
        // Compaction should keep assistant tool-call/result relationships valid
        // even after removing old units.
        let mut messages = vec![Message::system("system prompt")];
        for idx in 0..8 {
            let call_id = format!("call-{idx}");
            messages.push(Message::user(format!("user turn {idx}")));
            messages.push(assistant_with_tool_call(&call_id, "run_shell"));
            messages.push(tool_result(
                &call_id,
                &format!(r#"{{"result":{{"exit_code":0,"stdout":"ok {idx}","stderr":""}}}}"#),
            ));
            messages.push(Message {
                role: Role::Assistant,
                content: Some(format!("assistant turn {idx}")),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            });
        }

        let report = compact_history_with_budget(&mut messages, 260, 0.45, true)
            .expect("history should compact");
        assert!(report.removed_messages > 0);
        assert!(report.removed_turns > 0);
        assert_tool_history_integrity(&messages);
    }

    #[test]
    fn compact_history_retains_last_failed_tool_operations() {
        // Last N failed operations should remain as verbatim tool messages.
        let mut messages = vec![Message::system("system prompt")];
        let mut failure_markers = Vec::<String>::new();
        for idx in 0..6 {
            let call_id = format!("call-{idx}");
            messages.push(Message::user(format!("user turn {idx}")));
            messages.push(assistant_with_tool_call(&call_id, "run_shell"));
            if idx >= 2 {
                let marker = format!("failure marker {idx}");
                failure_markers.push(marker.clone());
                messages.push(tool_result(
                    &call_id,
                    &format!("Tool error: command failed ({marker})"),
                ));
            } else {
                messages.push(tool_result(
                    &call_id,
                    r#"{"result":{"exit_code":0,"stdout":"ok","stderr":""}}"#,
                ));
            }
            messages.push(Message {
                role: Role::Assistant,
                content: Some(format!("assistant turn {idx}")),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            });
        }

        let _ = compact_history_with_budget(&mut messages, 240, 0.42, true)
            .expect("history should compact");

        let retained_tool_text = messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .filter_map(|message| message.content.as_deref())
            .collect::<Vec<_>>()
            .join("\n");
        // Keep the three most-recent failure markers verbatim.
        for marker in failure_markers
            .iter()
            .rev()
            .take(RETAIN_FAILED_TOOL_OPERATIONS)
        {
            assert!(
                retained_tool_text.contains(marker),
                "expected retained failure marker `{marker}` in surviving tool messages"
            );
        }
    }

    #[test]
    fn compact_summary_is_structured_and_status_aware() {
        // Summary lines should follow the structured op/status/detail format and
        // include success/failure state for tool results.
        let removed = vec![
            Message::user("list files"),
            assistant_with_tool_call("call-1", "run_shell"),
            tool_result(
                "call-1",
                r#"{"result":{"exit_code":0,"stdout":"ok","stderr":""}}"#,
            ),
            assistant_with_tool_call("call-2", "run_shell"),
            tool_result("call-2", "Tool error: permission denied"),
        ];
        let summary = build_compact_summary(None, &removed);
        assert!(summary.starts_with(COMPACT_SUMMARY_PREFIX));
        assert!(summary.contains("op=tool.run_shell.request"));
        assert!(summary.contains("op=tool.run_shell.result"));
        assert!(summary.contains("status=success"));
        assert!(summary.contains("status=failure"));
        assert!(summary.contains("detail="));
    }

    #[test]
    fn compaction_repairs_orphaned_tool_messages() {
        // Even when no messages are removable, pre-compaction validation should
        // repair malformed assistant/tool history in-place.
        let mut messages = vec![
            Message::system("system"),
            assistant_with_tool_call("call-1", "run_shell"),
            Message::user("next"),
            tool_result("orphan", "Tool error: orphan"),
        ];
        let _ = compact_history_with_budget(&mut messages, 8_000, 0.9, false);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
    }
}
