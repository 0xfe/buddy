//! HTTP client for OpenAI-compatible APIs.
//!
//! The API layer is split into cohesive protocol modules:
//! - `completions`: `/chat/completions`
//! - `responses`: `/responses`
//! - `policy`: provider-specific transport/runtime rules
//! - `client`: shared auth and dispatch orchestration

use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use std::time::SystemTime;

mod client;
mod completions;
mod policy;
mod responses;

pub use client::ApiClient;

/// Minimal model API interface used by the agent loop.
///
/// This trait lets tests provide deterministic mock responses without network
/// calls while the production path uses [`ApiClient`].
#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError>;
}

/// Parse `Retry-After` response headers into a delay in seconds.
///
/// The header can be either delta-seconds (`120`) or an HTTP-date.
pub(crate) fn parse_retry_after_secs(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let at = httpdate::parse_http_date(value).ok()?;
    let now = SystemTime::now();
    let delay = at.duration_since(now).ok()?;
    Some(delay.as_secs().max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use serde_json::{json, Value};

    #[test]
    fn parse_retry_after_supports_delta_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("12"));
        assert_eq!(parse_retry_after_secs(&headers), Some(12));
    }

    #[test]
    fn parse_retry_after_supports_http_date() {
        use std::time::UNIX_EPOCH;
        let mut headers = HeaderMap::new();
        let future = UNIX_EPOCH + std::time::Duration::from_secs(4_102_444_800); // 2100-01-01
        let date = httpdate::fmt_http_date(future);
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_str(&date).expect("valid header value"),
        );
        assert!(parse_retry_after_secs(&headers).is_some());
    }

    #[test]
    fn parse_retry_after_ignores_invalid_values() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("not-a-date"));
        assert_eq!(parse_retry_after_secs(&headers), None);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SemanticShape {
        content: Option<String>,
        tool_calls: Vec<(String, String, String)>,
        usage: Option<(u64, u64, u64)>,
    }

    fn semantic_shape(response: &ChatResponse) -> SemanticShape {
        let message = &response.choices[0].message;
        let tool_calls = message
            .tool_calls
            .as_ref()
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| {
                        (
                            call.id.clone(),
                            call.function.name.clone(),
                            call.function.arguments.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let usage = response
            .usage
            .as_ref()
            .map(|u| (u.prompt_tokens, u.completion_tokens, u.total_tokens));
        SemanticShape {
            content: message.content.clone(),
            tool_calls,
            usage,
        }
    }

    fn parse_completions_fixture(raw: Value) -> ChatResponse {
        serde_json::from_value(raw).expect("valid completions fixture")
    }

    #[test]
    fn protocol_fixture_text_response_normalizes_like_completions() {
        let completions = parse_completions_fixture(json!({
            "id": "chatcmpl_1",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "done" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12 }
        }));
        let responses = responses::parse_responses_payload(&json!({
            "id": "resp_1",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "done" }]
            }],
            "usage": { "input_tokens": 9, "output_tokens": 3, "total_tokens": 12 }
        }))
        .expect("valid responses fixture");

        assert_eq!(semantic_shape(&responses), semantic_shape(&completions));
    }

    #[test]
    fn protocol_fixture_tool_call_normalizes_like_completions() {
        let completions = parse_completions_fixture(json!({
            "id": "chatcmpl_2",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "run_shell", "arguments": "{\"command\":\"ls\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 20, "completion_tokens": 5, "total_tokens": 25 }
        }));
        let responses = responses::parse_responses_payload(&json!({
            "id": "resp_2",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "call_id": "call_1",
                "name": "run_shell",
                "arguments": "{\"command\":\"ls\"}"
            }],
            "usage": { "input_tokens": 20, "output_tokens": 5, "total_tokens": 25 }
        }))
        .expect("valid responses fixture");

        assert_eq!(semantic_shape(&responses), semantic_shape(&completions));
    }
}
