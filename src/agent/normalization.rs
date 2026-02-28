//! Message normalization and reasoning extraction.
//!
//! Providers can emit malformed/empty assistant messages and a wide range of
//! reasoning payload formats. This module centralizes cleanup/extraction rules
//! so the main agent loop stays focused on control flow.

use crate::types::{Message, Role};
use serde_json::Value;

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
pub(super) fn sanitize_conversation_history(messages: &mut Vec<Message>) {
    for message in messages.iter_mut() {
        sanitize_message(message);
    }
    messages.retain(should_keep_message);
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
        tool_calls.retain(|tc| {
            !tc.id.trim().is_empty()
                && !tc.function.name.trim().is_empty()
                && !tc.function.arguments.trim().is_empty()
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
                out.push(text.clone());
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

/// Return true for JSON strings that are present but effectively empty.
fn is_empty_json_string(value: &Value) -> bool {
    value
        .as_str()
        .map(|text| text.trim().is_empty())
        .unwrap_or(false)
}
