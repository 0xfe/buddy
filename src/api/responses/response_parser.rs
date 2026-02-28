//! Parser for non-streaming `/responses` JSON payloads.

use crate::error::ApiError;
use crate::types::{ChatResponse, Choice, FunctionCall, Message, Role, ToolCall, Usage};
use serde_json::Value;
use std::collections::BTreeMap;

/// Parse one non-streaming `/responses` payload into `ChatResponse`.
pub(crate) fn parse_responses_payload(payload: &Value) -> Result<ChatResponse, ApiError> {
    // Preserve a stable fallback id when providers omit it.
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("responses-unknown")
        .to_string();

    let mut assistant_text = Vec::<String>::new();
    let mut tool_calls = Vec::<ToolCall>::new();
    let mut reasoning_items = Vec::<Value>::new();

    if let Some(output) = payload.get("output").and_then(Value::as_array) {
        for item in output {
            let Some(kind) = item.get("type").and_then(Value::as_str) else {
                continue;
            };
            // Normalize only assistant-visible text, tool calls, and reasoning.
            match kind {
                "message" => {
                    parse_output_message_text(item, &mut assistant_text);
                }
                "function_call" => {
                    if let Some(tool_call) = parse_output_function_call(item, tool_calls.len()) {
                        tool_calls.push(tool_call);
                    }
                }
                "reasoning" => {
                    reasoning_items.push(item.clone());
                }
                _ => {}
            }
        }
    }

    if assistant_text.is_empty() {
        if let Some(text) = payload
            .get("output_text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            assistant_text.push(text.to_string());
        }
    }

    let mut assistant = Message {
        role: Role::Assistant,
        content: (!assistant_text.is_empty()).then(|| assistant_text.join("\n")),
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        tool_call_id: None,
        name: None,
        extra: BTreeMap::new(),
    };
    if !reasoning_items.is_empty() {
        assistant
            .extra
            .insert("reasoning".to_string(), Value::Array(reasoning_items));
    }

    let finish_reason = payload
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string);

    let usage = payload.get("usage").and_then(parse_usage);

    Ok(ChatResponse {
        id,
        choices: vec![Choice {
            index: 0,
            message: assistant,
            finish_reason,
        }],
        usage,
    })
}

/// Extract assistant text segments from a `/responses` message item.
fn parse_output_message_text(item: &Value, out: &mut Vec<String>) {
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return;
    };
    for part in content {
        let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
        let Some(text) = part
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        match part_type {
            // Some providers use `text` or `input_text` interchangeably.
            "output_text" | "input_text" | "text" => out.push(text.to_string()),
            _ => {}
        }
    }
}

/// Parse one `function_call` output item into the normalized tool-call shape.
fn parse_output_function_call(item: &Value, index: usize) -> Option<ToolCall> {
    let name = item.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let id = item
        .get("call_id")
        .and_then(Value::as_str)
        .or_else(|| item.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("call_{index}"));
    let args = match item.get("arguments") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => serde_json::to_string(other).ok()?,
        None => "{}".to_string(),
    };
    Some(ToolCall {
        id,
        call_type: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: args,
        },
    })
}

/// Parse usage totals from either completions-style or responses-style keys.
fn parse_usage(usage: &Value) -> Option<Usage> {
    let prompt_tokens = read_u64(usage, &["prompt_tokens", "input_tokens"])?;
    let completion_tokens = read_u64(usage, &["completion_tokens", "output_tokens"])?;
    let total_tokens =
        read_u64(usage, &["total_tokens"]).unwrap_or(prompt_tokens + completion_tokens);
    Some(Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

/// Read the first valid unsigned integer from a set of candidate keys.
fn read_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| match v {
            Value::Number(num) => num.as_u64(),
            Value::String(text) => text.trim().parse::<u64>().ok(),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Ensures parser extracts text, tool calls, reasoning blobs, and usage totals.
    #[test]
    fn parse_responses_payload_extracts_text_tool_calls_and_usage() {
        let raw = json!({
            "id": "resp_123",
            "status": "completed",
            "output": [
                { "type": "reasoning", "summary": [ { "type":"summary_text", "text":"step" } ] },
                { "type": "function_call", "call_id": "call_1", "name": "run_shell", "arguments": "{\"command\":\"ls\"}" },
                { "type": "message", "role": "assistant", "content": [ { "type": "output_text", "text": "done" } ] }
            ],
            "usage": { "input_tokens": 12, "output_tokens": 3, "total_tokens": 15 }
        });

        let parsed = parse_responses_payload(&raw).expect("parse");
        assert_eq!(parsed.id, "resp_123");
        assert_eq!(parsed.choices.len(), 1);
        let msg = &parsed.choices[0].message;
        assert_eq!(msg.content.as_deref(), Some("done"));
        assert_eq!(msg.tool_calls.as_ref().map(|x| x.len()), Some(1));
        assert!(msg.extra.contains_key("reasoning"));
        assert_eq!(parsed.usage.as_ref().map(|u| u.total_tokens), Some(15));
    }
}
