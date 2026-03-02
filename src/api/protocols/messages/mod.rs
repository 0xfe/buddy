//! Native Anthropic `/v1/messages` protocol support.
//!
//! Anthropic is not wire-compatible with OpenAI chat/responses payloads, so we
//! translate between Buddy's normalized chat/tool model and Anthropic content
//! block semantics.

use crate::api::parse_retry_after_secs;
use crate::error::ApiError;
use crate::types::{
    ChatRequest, ChatResponse, Choice, FunctionCall, Message, Role, ToolCall, Usage,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Required API version header for Anthropic Messages API.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Conservative default completion cap for one Anthropic message request.
const DEFAULT_MAX_TOKENS: u64 = 4096;

/// Send one Anthropic Messages request and parse into normalized response shape.
pub(crate) async fn request(
    http: &reqwest::Client,
    base_url: &str,
    request: &ChatRequest,
    api_key: Option<&str>,
) -> Result<ChatResponse, ApiError> {
    let url = format!("{base_url}/messages");
    let payload = build_payload(request);
    let mut req = http
        .post(&url)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&payload);
    if let Some(key) = api_key.filter(|value| !value.trim().is_empty()) {
        req = req.header("x-api-key", key);
    }

    let response = req.send().await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let retry_after_secs = parse_retry_after_secs(response.headers());
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::status(status, body, retry_after_secs));
    }

    let payload = response.json::<Value>().await?;
    parse_payload(&payload)
}

/// Build Anthropic Messages API payload from Buddy's normalized chat request.
fn build_payload(request: &ChatRequest) -> Value {
    let mut system_lines = Vec::<String>::new();
    let mut messages = Vec::<Value>::new();
    for message in &request.messages {
        match message.role {
            Role::System => {
                if let Some(text) = message
                    .content
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    system_lines.push(text.to_string());
                }
            }
            Role::User => {
                if let Some(text) = message
                    .content
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    messages.push(json!({
                        "role": "user",
                        "content": text
                    }));
                }
            }
            Role::Assistant => {
                let mut content_blocks = Vec::<Value>::new();
                if let Some(text) = message
                    .content
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": text
                    }));
                }
                if let Some(tool_calls) = message.tool_calls.as_ref() {
                    for tool_call in tool_calls {
                        let input = serde_json::from_str::<Value>(&tool_call.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": tool_call.id,
                            "name": tool_call.function.name,
                            "input": input
                        }));
                    }
                }
                if !content_blocks.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": content_blocks
                    }));
                }
            }
            Role::Tool => {
                let Some(tool_use_id) = message
                    .tool_call_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let content = message.content.as_deref().unwrap_or_default();
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content
                    }]
                }));
            }
        }
    }

    let mut payload = serde_json::Map::new();
    payload.insert("model".to_string(), Value::String(request.model.clone()));
    payload.insert("max_tokens".to_string(), Value::from(DEFAULT_MAX_TOKENS));
    payload.insert("messages".to_string(), Value::Array(messages));
    if !system_lines.is_empty() {
        payload.insert(
            "system".to_string(),
            Value::String(system_lines.join("\n\n")),
        );
    }
    if let Some(tools) = request
        .tools
        .as_ref()
        .map(|defs| {
            defs.iter()
                .map(|tool| {
                    json!({
                        "name": tool.function.name,
                        "description": tool.function.description,
                        "input_schema": tool.function.parameters,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|list| !list.is_empty())
    {
        payload.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(temperature) = request.temperature {
        payload.insert("temperature".to_string(), Value::from(temperature));
    }
    if let Some(top_p) = request.top_p {
        payload.insert("top_p".to_string(), Value::from(top_p));
    }
    Value::Object(payload)
}

/// Parse Anthropic Messages API response into normalized chat response shape.
fn parse_payload(payload: &Value) -> Result<ChatResponse, ApiError> {
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("anthropic-unknown")
        .to_string();
    let mut assistant_text = Vec::<String>::new();
    let mut tool_calls = Vec::<ToolCall>::new();
    let mut reasoning = Vec::<Value>::new();

    if let Some(content) = payload.get("content").and_then(Value::as_array) {
        for item in content {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
            match kind {
                "text" => {
                    if let Some(text) = item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        assistant_text.push(text.to_string());
                    }
                }
                "tool_use" => {
                    let Some(name) = item
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    else {
                        continue;
                    };
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("tool_use");
                    let arguments = item
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                        .to_string();
                    tool_calls.push(ToolCall {
                        id: id.to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: name.to_string(),
                            arguments,
                        },
                    });
                }
                "thinking" | "redacted_thinking" => reasoning.push(item.clone()),
                _ => {}
            }
        }
    }

    let mut extra = BTreeMap::new();
    if !reasoning.is_empty() {
        extra.insert("reasoning".to_string(), Value::Array(reasoning));
    }
    let assistant = Message {
        role: Role::Assistant,
        content: (!assistant_text.is_empty()).then(|| assistant_text.join("\n")),
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        tool_call_id: None,
        name: None,
        extra,
    };

    let usage = payload.get("usage").and_then(|usage| {
        let prompt_tokens = usage.get("input_tokens").and_then(Value::as_u64)?;
        let completion_tokens = usage.get("output_tokens").and_then(Value::as_u64)?;
        Some(Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        })
    });
    let finish_reason = payload
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(str::to_string);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDefinition, ToolDefinition};

    // Ensures request translation preserves system text, function tools, and tool results.
    #[test]
    fn build_payload_maps_tools_and_tool_results() {
        let request = ChatRequest {
            model: "claude-sonnet-4-5".to_string(),
            messages: vec![
                Message::system("sys"),
                Message::user("u1"),
                Message::tool_result("tool_1", "ok"),
            ],
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "run_shell".to_string(),
                    description: "Run command".to_string(),
                    parameters: json!({"type":"object","properties":{"command":{"type":"string"}}}),
                },
            }]),
            temperature: Some(0.2),
            top_p: Some(0.9),
        };
        let payload = build_payload(&request);
        assert_eq!(payload["model"], "claude-sonnet-4-5");
        assert_eq!(payload["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(payload["system"], "sys");
        assert_eq!(payload["tools"][0]["name"], "run_shell");
        assert_eq!(payload["messages"][1]["content"][0]["type"], "tool_result");
    }

    // Ensures response translation preserves text, tool calls, usage, and stop reason.
    #[test]
    fn parse_payload_maps_text_and_tool_use() {
        let payload = json!({
            "id":"msg_123",
            "content":[
                {"type":"text","text":"hello"},
                {"type":"tool_use","id":"toolu_1","name":"run_shell","input":{"command":"ls"}}
            ],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":10,"output_tokens":4}
        });
        let parsed = parse_payload(&payload).expect("parse");
        assert_eq!(parsed.id, "msg_123");
        assert_eq!(parsed.choices[0].message.content.as_deref(), Some("hello"));
        assert_eq!(
            parsed.choices[0]
                .message
                .tool_calls
                .as_ref()
                .map(|calls| calls.len()),
            Some(1)
        );
        assert_eq!(parsed.choices[0].finish_reason.as_deref(), Some("tool_use"));
        assert_eq!(parsed.usage.as_ref().map(|u| u.total_tokens), Some(14));
    }
}
