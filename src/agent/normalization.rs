//! Message normalization and reasoning extraction.
//!
//! Providers can emit malformed/empty assistant messages and a wide range of
//! reasoning payload formats. This module centralizes cleanup/extraction rules
//! so the main agent loop stays focused on control flow.

use crate::types::{Message, Role};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Extract normalized `(field, trace)` reasoning tuples from a message payload.
pub(super) fn reasoning_traces(message: &Message) -> Vec<(String, String)> {
    message
        .extra
        .iter()
        .filter_map(|(key, value)| {
            if !is_reasoning_key(key) {
                return None;
            }
            reasoning_value_to_text(value).map(|text| (key.clone(), text))
        })
        .collect()
}

/// Return true when a key likely contains provider reasoning/thinking content.
pub(super) fn is_reasoning_key(key: &str) -> bool {
    let k = key.to_lowercase();
    k.contains("reasoning") || k.contains("thinking") || k.contains("thought")
}

/// Sanitize all messages in-place and drop entries that carry no useful signal.
pub(super) fn sanitize_conversation_history(
    messages: &mut Vec<Message>,
) -> ToolPairValidationReport {
    for message in messages.iter_mut() {
        sanitize_message(message);
    }
    let report = repair_tool_call_message_pairs(messages);
    messages.retain(should_keep_message);
    report
}

/// Normalize one message by trimming content and pruning empty metadata fields.
pub(super) fn sanitize_message(message: &mut Message) {
    if let Some(content) = message.content.as_mut() {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            message.content = None;
        } else if trimmed.len() != content.len() {
            *content = trimmed.to_string();
        }
    }

    if let Some(tool_calls) = message.tool_calls.as_mut() {
        for call in tool_calls.iter_mut() {
            call.id = call.id.trim().to_string();
            call.function.name = call.function.name.trim().to_string();
            call.function.arguments = call.function.arguments.trim().to_string();
        }
        let mut seen_call_ids = HashSet::<String>::new();
        tool_calls.retain(|tc| {
            !tc.id.trim().is_empty()
                && !tc.function.name.trim().is_empty()
                && !tc.function.arguments.trim().is_empty()
                && seen_call_ids.insert(tc.id.clone())
        });
        if tool_calls.is_empty() {
            message.tool_calls = None;
        }
    }

    if let Some(tool_call_id) = message.tool_call_id.as_mut() {
        let trimmed = tool_call_id.trim();
        if trimmed.is_empty() {
            message.tool_call_id = None;
        } else if trimmed.len() != tool_call_id.len() {
            *tool_call_id = trimmed.to_string();
        }
    }

    message
        .extra
        .retain(|_, value| !value.is_null() && !is_empty_json_string(value));
}

/// Repair assistant/tool-call history so every tool result references a live
/// assistant-declared tool call and unresolved calls are removed.
///
/// This keeps provider message history protocol-valid after model/tool errors,
/// cancellation, or malformed provider payloads.
pub(super) fn repair_tool_call_message_pairs(
    messages: &mut Vec<Message>,
) -> ToolPairValidationReport {
    let mut repaired = ToolPairValidationReport::default();
    let mut kept = Vec::<Message>::with_capacity(messages.len());
    let mut pending_calls_by_assistant = HashMap::<usize, HashSet<String>>::new();
    let mut pending_call_to_assistant = HashMap::<String, usize>::new();

    for message in messages.drain(..) {
        match message.role {
            Role::Assistant => {
                finalize_unmatched_assistant_calls(
                    &mut kept,
                    &mut pending_calls_by_assistant,
                    &mut pending_call_to_assistant,
                    &mut repaired,
                );

                let assistant_idx = kept.len();
                let mut pending_for_message = HashSet::<String>::new();
                if let Some(tool_calls) = message.tool_calls.as_ref() {
                    for call in tool_calls {
                        let inserted = pending_call_to_assistant
                            .insert(call.id.clone(), assistant_idx)
                            .is_none();
                        if inserted {
                            pending_for_message.insert(call.id.clone());
                        }
                    }
                }
                if !pending_for_message.is_empty() {
                    pending_calls_by_assistant.insert(assistant_idx, pending_for_message);
                }
                kept.push(message);
            }
            Role::Tool => {
                let Some(call_id) = message.tool_call_id.as_ref().map(|id| id.to_string()) else {
                    repaired.dropped_orphan_tool_results += 1;
                    continue;
                };
                if let Some(assistant_idx) = pending_call_to_assistant.remove(&call_id) {
                    if let Some(unresolved) = pending_calls_by_assistant.get_mut(&assistant_idx) {
                        unresolved.remove(&call_id);
                        if unresolved.is_empty() {
                            pending_calls_by_assistant.remove(&assistant_idx);
                        }
                    }
                    kept.push(message);
                } else {
                    repaired.dropped_orphan_tool_results += 1;
                }
            }
            Role::System | Role::User => {
                finalize_unmatched_assistant_calls(
                    &mut kept,
                    &mut pending_calls_by_assistant,
                    &mut pending_call_to_assistant,
                    &mut repaired,
                );
                kept.push(message);
            }
        }
    }

    finalize_unmatched_assistant_calls(
        &mut kept,
        &mut pending_calls_by_assistant,
        &mut pending_call_to_assistant,
        &mut repaired,
    );

    kept.retain(should_keep_message);
    *messages = kept;
    repaired
}

/// Structured report produced while repairing tool-call/result pairing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ToolPairValidationReport {
    /// Tool result messages dropped because no matching assistant tool-call was
    /// found in valid pending state.
    pub dropped_orphan_tool_results: usize,
    /// Assistant tool-call declarations removed because no corresponding tool
    /// result message arrived before the turn advanced.
    pub repaired_unmatched_tool_calls: usize,
}

/// Remove any unresolved assistant tool calls from in-flight assistant
/// messages before history advances to a new non-tool segment.
fn finalize_unmatched_assistant_calls(
    kept: &mut [Message],
    pending_calls_by_assistant: &mut HashMap<usize, HashSet<String>>,
    pending_call_to_assistant: &mut HashMap<String, usize>,
    repaired: &mut ToolPairValidationReport,
) {
    if pending_calls_by_assistant.is_empty() {
        return;
    }
    let pending = std::mem::take(pending_calls_by_assistant);
    for (assistant_idx, unresolved_ids) in pending {
        for id in &unresolved_ids {
            pending_call_to_assistant.remove(id);
        }
        let Some(message) = kept.get_mut(assistant_idx) else {
            continue;
        };
        let Some(tool_calls) = message.tool_calls.as_mut() else {
            continue;
        };
        let before = tool_calls.len();
        tool_calls.retain(|call| !unresolved_ids.contains(&call.id));
        repaired.repaired_unmatched_tool_calls += before.saturating_sub(tool_calls.len());
        if tool_calls.is_empty() {
            message.tool_calls = None;
        }
    }
}

/// Decide whether a sanitized message should stay in history.
pub(super) fn should_keep_message(message: &Message) -> bool {
    match message.role {
        Role::System | Role::User => message.content.is_some(),
        Role::Assistant => {
            message.content.is_some()
                || message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
        }
        Role::Tool => message.tool_call_id.is_some(),
    }
}

/// Render an arbitrary reasoning JSON payload into a compact text block.
pub(super) fn reasoning_value_to_text(value: &Value) -> Option<String> {
    let mut lines = Vec::<String>::new();
    collect_reasoning_strings(value, None, &mut lines);
    let mut unique = Vec::<String>::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if unique.iter().any(|existing| existing == trimmed) {
            continue;
        }
        unique.push(trimmed.to_string());
    }
    if unique.is_empty() {
        None
    } else {
        Some(unique.join("\n"))
    }
}

/// Recursively collect reasoning-like text snippets from nested JSON values.
fn collect_reasoning_strings(value: &Value, key: Option<&str>, out: &mut Vec<String>) {
    match value {
        Value::Null => {}
        Value::String(text) => {
            if key.is_none_or(is_reasoning_text_key) {
                if let Some(normalized) = normalize_reasoning_leaf_text(text, key) {
                    out.push(normalized);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_reasoning_strings(item, key, out);
            }
        }
        Value::Object(map) => {
            for (child_key, child_value) in map {
                collect_reasoning_strings(child_value, Some(child_key.as_str()), out);
            }
        }
        Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Allowlist of common reasoning text keys seen across providers.
fn is_reasoning_text_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "reasoning"
            | "reasoning_text"
            | "reasoning_content"
            | "reasoning_stream"
            | "thinking"
            | "thought"
            | "summary"
            | "summary_text"
            | "text"
            | "content"
            | "content_text"
            | "output_text"
            | "input_text"
            | "details"
            | "analysis"
            | "explanation"
    )
}

/// Normalize one reasoning text leaf and filter placeholder/noise values.
fn normalize_reasoning_leaf_text(text: &str, key: Option<&str>) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || is_placeholder_reasoning_text(trimmed) {
        return None;
    }

    // Some providers embed JSON-encoded reasoning arrays/objects inside strings.
    // Parse and recursively extract only human-readable reasoning text.
    if looks_like_json_container(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            let mut nested = Vec::<String>::new();
            collect_reasoning_strings(&parsed, key, &mut nested);
            if nested.is_empty() {
                return None;
            }
            return Some(nested.join("\n"));
        }
    }

    Some(trimmed.to_string())
}

/// Return true for placeholder string values that should never be rendered.
fn is_placeholder_reasoning_text(text: &str) -> bool {
    matches!(
        text.to_ascii_lowercase().as_str(),
        "null" | "none" | "n/a" | "na" | "[]" | "{}"
    )
}

/// Quick check before attempting JSON parse on a string leaf.
fn looks_like_json_container(text: &str) -> bool {
    (text.starts_with('{') && text.ends_with('}')) || (text.starts_with('[') && text.ends_with(']'))
}

/// Return true for JSON strings that are present but effectively empty.
fn is_empty_json_string(value: &Value) -> bool {
    value
        .as_str()
        .map(|text| {
            let trimmed = text.trim();
            trimmed.is_empty() || is_placeholder_reasoning_text(trimmed)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionCall, ToolCall};
    use std::collections::BTreeMap;

    /// Build a minimal assistant tool-call fixture for normalization tests.
    fn assistant_with_tool_call(id: &str, name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: id.to_string(),
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

    /// Build a minimal tool-result fixture linked to one call id.
    fn tool_result(id: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            name: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn sanitize_history_drops_orphan_tool_results() {
        // Orphan tool results should never survive sanitation because providers
        // expect each tool result to match a declared assistant tool call.
        let mut messages = vec![Message::system("sys"), tool_result("missing", "ok")];
        let report = sanitize_conversation_history(&mut messages);
        assert_eq!(report.dropped_orphan_tool_results, 1);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
    }

    #[test]
    fn sanitize_history_repairs_unmatched_assistant_tool_calls() {
        // Assistant tool calls with no matching tool result should be removed
        // so request history remains provider-protocol valid.
        let mut messages = vec![
            Message::system("sys"),
            assistant_with_tool_call("call_1", "run_shell"),
            Message::user("next turn"),
        ];
        let report = sanitize_conversation_history(&mut messages);
        assert_eq!(report.repaired_unmatched_tool_calls, 1);
        // Assistant message had no content and lost its only tool-call, so it
        // should be removed entirely by keep-message filtering.
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
    }

    #[test]
    fn sanitize_history_keeps_well_formed_tool_call_pairs() {
        // Well-formed assistant tool-call + tool result pairs should survive
        // normalization unchanged.
        let mut messages = vec![
            Message::system("sys"),
            assistant_with_tool_call("call_1", "run_shell"),
            tool_result("call_1", "ok"),
            Message {
                role: Role::Assistant,
                content: Some("done".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            },
        ];
        let report = sanitize_conversation_history(&mut messages);
        assert_eq!(report.dropped_orphan_tool_results, 0);
        assert_eq!(report.repaired_unmatched_tool_calls, 0);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].role, Role::Tool);
    }
}
