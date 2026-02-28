//! SSE parser for streaming `/responses` output.

use super::response_parser::parse_responses_payload;
use crate::error::ApiError;
use crate::types::{ChatResponse, Choice, Message, Role};
use serde_json::Value;
use std::collections::BTreeMap;

/// Parse a streaming SSE payload returned by `POST /responses`.
pub(super) fn parse_streaming_responses_payload(body: &str) -> Result<ChatResponse, ApiError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(ApiError::InvalidResponse(
            "empty streaming response body".to_string(),
        ));
    }

    // Some providers return non-streaming JSON even when stream=true.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::{sse_done_block, sse_event_block};

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

    #[cfg(feature = "fuzz-tests")]
    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn parse_sse_event_payloads_round_trips_data_blocks(
                payloads in proptest::collection::vec(
                    proptest::collection::vec(
                        proptest::string::string_regex("[ -~]{0,24}").expect("regex"),
                        1..4
                    ),
                    0..8
                )
            ) {
                let mut stream = String::new();
                let mut expected = Vec::new();
                for (idx, payload_lines) in payloads.iter().enumerate() {
                    stream.push_str(": keepalive\n");
                    stream.push_str(&format!("event: e{idx}\n"));
                    for line in payload_lines {
                        stream.push_str("data: ");
                        stream.push_str(line);
                        stream.push('\n');
                    }
                    stream.push_str("id: 1\n\n");
                    expected.push(payload_lines.join("\n"));
                }

                prop_assert_eq!(parse_sse_event_payloads(&stream), expected);
            }
        }
    }
}
