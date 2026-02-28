//! Helpers for translating chat-style requests into `/responses` payloads.

use crate::types::{ChatRequest, Message, Role};
use serde_json::{json, Map, Value};

/// Build the provider payload for `POST /responses`.
pub(super) fn build_responses_payload(
    request: &ChatRequest,
    store_false: bool,
    stream: bool,
) -> Value {
    let mut instructions = Vec::<String>::new();
    let mut input = Vec::<Value>::new();
    for message in &request.messages {
        // `/responses` expects all system content in `instructions`.
        if message.role == Role::System {
            if let Some(content) = message
                .content
                .as_ref()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
            {
                instructions.push(content.to_string());
            }
            continue;
        }
        // Non-system turns are translated to `input` items.
        input.extend(message_to_responses_items(message));
    }

    // Convert tool definitions to the function-tool shape expected by `/responses`.
    let tools = request.tools.as_ref().map(|defs| {
        defs.iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.function.name,
                    "description": tool.function.description,
                    "parameters": tool.function.parameters,
                })
            })
            .collect::<Vec<_>>()
    });

    let mut payload = Map::new();
    payload.insert("model".to_string(), Value::String(request.model.clone()));
    payload.insert("input".to_string(), Value::Array(input));
    if !instructions.is_empty() {
        payload.insert(
            "instructions".to_string(),
            Value::String(instructions.join("\n\n")),
        );
    }
    if let Some(tools) = tools.filter(|items| !items.is_empty()) {
        payload.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(temperature) = request.temperature {
        payload.insert("temperature".to_string(), Value::from(temperature));
    }
    if let Some(top_p) = request.top_p {
        payload.insert("top_p".to_string(), Value::from(top_p));
    }
    if store_false {
        payload.insert("store".to_string(), Value::Bool(false));
    }
    if stream {
        payload.insert("stream".to_string(), Value::Bool(true));
    }
    Value::Object(payload)
}

/// Convert one chat message into zero or more `/responses` input items.
fn message_to_responses_items(message: &Message) -> Vec<Value> {
    let mut out = Vec::new();

    match message.role {
        Role::System | Role::User | Role::Assistant => {
            if let Some(content) = message
                .content
                .as_ref()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
            {
                let content_type = match message.role {
                    Role::Assistant => "output_text",
                    _ => "input_text",
                };
                out.push(json!({
                    "type": "message",
                    "role": role_to_wire(&message.role),
                    "content": [
                        { "type": content_type, "text": content }
                    ]
                }));
            }

            if let Some(tool_calls) = message.tool_calls.as_ref() {
                for tc in tool_calls {
                    // Assistant tool calls become top-level function_call entries.
                    out.push(json!({
                        "type": "function_call",
                        "call_id": tc.id,
                        "name": tc.function.name,
                        "arguments": tc.function.arguments,
                    }));
                }
            }
        }
        Role::Tool => {
            let Some(call_id) = message
                .tool_call_id
                .as_ref()
                .map(|id| id.trim())
                .filter(|id| !id.is_empty())
            else {
                return out;
            };
            // Tool result turns map to function_call_output entries keyed by call id.
            let output = message.content.as_deref().unwrap_or_default();
            out.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
        }
    }

    out
}

/// Map internal role enums to `/responses` role strings.
fn role_to_wire(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDefinition, Message, ToolDefinition};
    use std::collections::BTreeMap;

    // Ensures tool-result turns are emitted as function_call_output items.
    #[test]
    fn responses_payload_maps_tool_result_messages() {
        let request = ChatRequest {
            model: "gpt-5.3-codex".to_string(),
            messages: vec![Message::user("hi"), Message::tool_result("call_1", "ok")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let payload = build_responses_payload(&request, false, false);
        let input = payload["input"].as_array().expect("array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["output"], "ok");
    }

    // Ensures function tool definitions are emitted with the expected shape.
    #[test]
    fn responses_payload_maps_function_tools_shape() {
        let request = ChatRequest {
            model: "gpt-5.3-codex".to_string(),
            messages: vec![Message::user("hi")],
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "run_shell".to_string(),
                    description: "Run shell".to_string(),
                    parameters: json!({"type":"object","properties":{"command":{"type":"string"}}}),
                },
            }]),
            temperature: Some(0.1),
            top_p: Some(0.9),
        };
        let payload = build_responses_payload(&request, false, false);
        assert_eq!(payload["tools"][0]["type"], "function");
        assert_eq!(payload["tools"][0]["name"], "run_shell");
        assert!(payload["tools"][0].get("description").is_some());
    }

    // Ensures system messages are moved into `instructions` and excluded from `input`.
    #[test]
    fn responses_payload_maps_system_messages_to_instructions() {
        let request = ChatRequest {
            model: "gpt-5.3-codex".to_string(),
            messages: vec![Message::system("sys"), Message::user("hi")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let payload = build_responses_payload(&request, false, false);
        assert_eq!(payload["instructions"], "sys");
        let input = payload["input"].as_array().expect("array");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
    }

    // Ensures assistant content uses `output_text` while user content uses `input_text`.
    #[test]
    fn responses_payload_maps_assistant_messages_to_output_text() {
        let request = ChatRequest {
            model: "gpt-5.3-codex".to_string(),
            messages: vec![
                Message::user("u1"),
                Message {
                    role: Role::Assistant,
                    content: Some("a1".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
            ],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let payload = build_responses_payload(&request, false, false);
        let input = payload["input"].as_array().expect("array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
    }

    // Ensures auth policy can force non-persistent responses storage mode.
    #[test]
    fn responses_payload_sets_store_false_when_requested() {
        let request = ChatRequest {
            model: "gpt-5.3-codex".to_string(),
            messages: vec![Message::system("sys"), Message::user("hi")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let payload = build_responses_payload(&request, true, false);
        assert_eq!(payload["store"], Value::Bool(false));
    }

    // Ensures streaming mode is explicitly requested in payloads when enabled.
    #[test]
    fn responses_payload_sets_stream_when_requested() {
        let request = ChatRequest {
            model: "gpt-5.3-codex".to_string(),
            messages: vec![Message::system("sys"), Message::user("hi")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let payload = build_responses_payload(&request, false, true);
        assert_eq!(payload["stream"], Value::Bool(true));
    }
}
