use crate::api::parse_retry_after_secs;
use crate::error::ApiError;
use crate::types::{
    ChatRequest, ChatResponse, Choice, FunctionCall, Message, Role, ToolCall, Usage,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ResponsesRequestOptions {
    pub(crate) store_false: bool,
    pub(crate) stream: bool,
}

pub(crate) async fn request(
    http: &reqwest::Client,
    base_url: &str,
    request: &ChatRequest,
    bearer: Option<&str>,
    options: ResponsesRequestOptions,
) -> Result<ChatResponse, ApiError> {
    let url = format!("{base_url}/responses");
    let payload = build_responses_payload(request, options.store_false, options.stream);
    let mut req = http.post(&url).json(&payload);
    if let Some(token) = bearer.filter(|value| !value.trim().is_empty()) {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let response = req.send().await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let retry_after_secs = parse_retry_after_secs(response.headers());
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::status(status, body, retry_after_secs));
    }

    if options.stream {
        let body = response.text().await?;
        parse_streaming_responses_payload(&body)
    } else {
        let body = response.json::<Value>().await?;
        parse_responses_payload(&body)
    }
}

fn build_responses_payload(request: &ChatRequest, store_false: bool, stream: bool) -> Value {
    let mut instructions = Vec::<String>::new();
    let mut input = Vec::<Value>::new();
    for message in &request.messages {
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
        input.extend(message_to_responses_items(message));
    }

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

fn role_to_wire(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn parse_responses_payload(payload: &Value) -> Result<ChatResponse, ApiError> {
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

fn parse_streaming_responses_payload(body: &str) -> Result<ChatResponse, ApiError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(ApiError::InvalidResponse(
            "empty streaming response body".to_string(),
        ));
    }

    // Some providers may still return a non-streaming JSON response even when
    // stream=true is requested.
    if trimmed.starts_with('{') {
        let payload: Value = serde_json::from_str(trimmed)
            .map_err(|err| ApiError::InvalidResponse(format!("invalid JSON response: {err}")))?;
        return parse_responses_payload(&payload);
    }

    let mut completed_response: Option<Value> = None;
    let mut output_text_delta = String::new();
    let mut reasoning_summary_deltas = BTreeMap::<usize, String>::new();
    let mut reasoning_content_deltas = BTreeMap::<usize, String>::new();
    let mut reasoning_items = Vec::<Value>::new();

    for event_payload in parse_sse_event_payloads(trimmed) {
        if event_payload.is_empty() || event_payload == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(&event_payload).map_err(|err| {
            ApiError::InvalidResponse(format!("invalid streaming event payload: {err}"))
        })?;
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    output_text_delta.push_str(delta);
                }
            }
            "response.reasoning_summary_text.delta" => {
                let Some(index) = event
                    .get("summary_index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                else {
                    continue;
                };
                let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                    continue;
                };
                reasoning_summary_deltas
                    .entry(index)
                    .or_default()
                    .push_str(delta);
            }
            "response.reasoning_text.delta" => {
                let Some(index) = event
                    .get("content_index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                else {
                    continue;
                };
                let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                    continue;
                };
                reasoning_content_deltas
                    .entry(index)
                    .or_default()
                    .push_str(delta);
            }
            "response.completed" | "response.done" => {
                if let Some(response) = event.get("response").cloned() {
                    completed_response = Some(response);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item").cloned() {
                    if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                        reasoning_items.push(item);
                    }
                }
            }
            "response.failed" => {
                let message = event
                    .get("response")
                    .and_then(|response| response.get("error"))
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("response.failed event received");
                return Err(ApiError::InvalidResponse(format!(
                    "streaming response failed: {message}"
                )));
            }
            _ => {}
        }
    }

    if let Some(response) = completed_response {
        let mut parsed = parse_responses_payload(&response)?;
        if let Some(choice) = parsed.choices.get_mut(0) {
            if choice
                .message
                .content
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
                && !output_text_delta.trim().is_empty()
            {
                choice.message.content = Some(output_text_delta);
            }

            let mut stream_reasoning_fragments = Vec::<String>::new();
            if !reasoning_summary_deltas.is_empty() {
                let summary = reasoning_summary_deltas
                    .into_values()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !summary.trim().is_empty() {
                    stream_reasoning_fragments.push(format!("summary:\n{summary}"));
                }
            }
            if !reasoning_content_deltas.is_empty() {
                let details = reasoning_content_deltas
                    .into_values()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !details.trim().is_empty() {
                    stream_reasoning_fragments.push(format!("details:\n{details}"));
                }
            }
            if !stream_reasoning_fragments.is_empty() {
                choice.message.extra.insert(
                    "reasoning_stream".to_string(),
                    Value::String(stream_reasoning_fragments.join("\n\n")),
                );
            }

            if !reasoning_items.is_empty() && !choice.message.extra.contains_key("reasoning") {
                choice
                    .message
                    .extra
                    .insert("reasoning".to_string(), Value::Array(reasoning_items));
            }
        }
        return Ok(parsed);
    }

    if !output_text_delta.trim().is_empty() {
        return Ok(ChatResponse {
            id: "responses-stream-unknown".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Some(output_text_delta),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
                finish_reason: None,
            }],
            usage: None,
        });
    }

    Err(ApiError::InvalidResponse(
        "stream closed before response.completed".to_string(),
    ))
}

/// Parse an SSE stream into concatenated `data` payload blocks.
///
/// The SSE spec allows events to contain multiple `data:` lines; payload lines
/// are joined with `\n` and finalized when a blank line is encountered.
fn parse_sse_event_payloads(stream: &str) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut data_lines = Vec::<String>::new();

    let mut flush_event = |lines: &mut Vec<String>| {
        if lines.is_empty() {
            return;
        }
        payloads.push(lines.join("\n"));
        lines.clear();
    };

    for raw_line in stream.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            flush_event(&mut data_lines);
            continue;
        }
        if line.starts_with(':') {
            continue;
        }

        let (field, value) = if let Some((field, value)) = line.split_once(':') {
            (field, value.strip_prefix(' ').unwrap_or(value))
        } else {
            (line, "")
        };
        if field == "data" {
            data_lines.push(value.to_string());
        }
    }
    flush_event(&mut data_lines);
    payloads
}

fn parse_output_message_text(item: &Value, out: &mut Vec<String>) {
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return;
    };
    for part in content {
        let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
        let text = part
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if text.is_none() {
            continue;
        }
        match part_type {
            "output_text" | "input_text" | "text" => out.push(text.unwrap().to_string()),
            _ => {}
        }
    }
}

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
    use crate::testsupport::{sse_done_block, sse_event_block};
    use crate::types::{FunctionDefinition, ToolDefinition};

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

    #[test]
    fn parse_streaming_responses_payload_extracts_completed_response() {
        let sse = format!(
            "{}{}{}",
            sse_event_block(
                "response.output_text.delta",
                r#"{"type":"response.output_text.delta","delta":"hel"}"#
            ),
            sse_event_block(
                "response.completed",
                r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}}"#
            ),
            sse_done_block()
        );
        let parsed = parse_streaming_responses_payload(&sse).expect("parse");
        assert_eq!(parsed.id, "resp_1");
        assert_eq!(parsed.choices[0].message.content.as_deref(), Some("hello"));
        assert_eq!(parsed.usage.as_ref().map(|u| u.total_tokens), Some(3));
    }

    #[test]
    fn parse_streaming_responses_payload_captures_reasoning_deltas() {
        let sse = format!(
            "{}{}{}{}",
            sse_event_block(
                "response.reasoning_summary_text.delta",
                r#"{"type":"response.reasoning_summary_text.delta","summary_index":0,"delta":"plan"}"#
            ),
            sse_event_block(
                "response.reasoning_text.delta",
                r#"{"type":"response.reasoning_text.delta","content_index":0,"delta":"step-1"}"#
            ),
            sse_event_block(
                "response.completed",
                r#"{"type":"response.completed","response":{"id":"resp_2","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}}"#
            ),
            sse_done_block()
        );
        let parsed = parse_streaming_responses_payload(&sse).expect("parse");
        let msg = &parsed.choices[0].message;
        assert_eq!(msg.content.as_deref(), Some("ok"));
        let reasoning = msg
            .extra
            .get("reasoning_stream")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reasoning.contains("plan"));
        assert!(reasoning.contains("step-1"));
    }

    #[test]
    fn parse_streaming_responses_payload_captures_reasoning_items() {
        let sse = format!(
            "{}{}{}",
            sse_event_block(
                "response.output_item.done",
                r#"{"type":"response.output_item.done","item":{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking"}]}}"#
            ),
            sse_event_block(
                "response.completed",
                r#"{"type":"response.completed","response":{"id":"resp_3","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}}"#
            ),
            sse_done_block()
        );
        let parsed = parse_streaming_responses_payload(&sse).expect("parse");
        let msg = &parsed.choices[0].message;
        assert!(msg.extra.contains_key("reasoning"));
    }

    #[test]
    fn parse_streaming_responses_payload_supports_multiline_sse_events() {
        let sse = concat!(
            ": keep-alive\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\n",
            "data: \"delta\":\"hel\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}]}}\n\n",
            "data: [DONE]\n\n"
        );
        let parsed = parse_streaming_responses_payload(sse).expect("parse");
        assert_eq!(parsed.id, "resp_4");
        assert_eq!(parsed.choices[0].message.content.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_sse_event_payloads_joins_data_lines_and_skips_comments() {
        let payloads = parse_sse_event_payloads(
            ": ping\n\
             event: demo\n\
             data: one\n\
             data: two\n\
             id: 1\n\
             \n\
             data: [DONE]\n\
             \n",
        );
        assert_eq!(payloads, vec!["one\ntwo".to_string(), "[DONE]".to_string()]);
    }

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
