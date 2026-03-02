//! HTTP client for OpenAI-compatible APIs.
//!
//! The API layer is split into cohesive protocol modules:
//! - `protocols/completions`: `/chat/completions`
//! - `protocols/responses`: `/responses`
//! - `protocols/messages`: `/messages`
//! - `policy`: provider-specific transport/runtime rules
//! - `client`: shared auth and dispatch orchestration

use crate::config::{AuthMode, ModelProvider};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;
use std::time::SystemTime;

mod client;
mod policy;
mod protocols;
mod provider_compat;

pub use client::ApiClient;

/// Return default provider-native built-in tool names for one request profile.
///
/// This is used by app orchestration code to keep prompt/tool registration
/// aligned with the same policy logic used by HTTP request construction.
pub fn default_builtin_tool_names(
    provider: ModelProvider,
    base_url: &str,
    auth: AuthMode,
    api_key: &str,
    model: &str,
) -> Vec<&'static str> {
    let options = policy::responses_request_options(provider, base_url, auth, api_key, model, None);
    let mut names = Vec::new();
    for builtin in options.builtin_tools {
        let kind = builtin
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = match kind {
            "web_search" | "web_search_preview" => Some("web_search"),
            "code_interpreter" => Some("code_interpreter"),
            _ => None,
        };
        if let Some(name) = name {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Minimal model API interface used by the agent loop.
///
/// This trait lets tests provide deterministic mock responses without network
/// calls while the production path uses [`ApiClient`].
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// Execute one chat request and return a normalized chat response.
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

    // Validates that integer `Retry-After` values are parsed directly.
    #[test]
    fn parse_retry_after_supports_delta_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("12"));
        assert_eq!(parse_retry_after_secs(&headers), Some(12));
    }

    // Validates that HTTP-date `Retry-After` values are converted to a delay.
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

    // Ensures malformed `Retry-After` headers are ignored safely.
    #[test]
    fn parse_retry_after_ignores_invalid_values() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("not-a-date"));
        assert_eq!(parse_retry_after_secs(&headers), None);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SemanticShape {
        /// Assistant text content extracted from the normalized response.
        content: Option<String>,
        /// Flattened function-call tuple: (id, function_name, arguments_json).
        tool_calls: Vec<(String, String, String)>,
        /// Usage tuple: (prompt_tokens, completion_tokens, total_tokens).
        usage: Option<(u64, u64, u64)>,
    }

    /// Reduce a full response to semantic fields shared across protocols.
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

    /// Parse a canonical `/chat/completions` fixture.
    fn parse_completions_fixture(raw: Value) -> ChatResponse {
        serde_json::from_value(raw).expect("valid completions fixture")
    }

    // Confirms a plain text response normalizes equivalently across protocols.
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
        let responses = protocols::responses::parse_responses_payload(&json!({
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

    // Confirms function-call responses normalize equivalently across protocols.
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
        let responses = protocols::responses::parse_responses_payload(&json!({
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

    // Ensures public builtin-tool helper follows OpenAI reasoning defaults.
    #[test]
    fn default_builtin_tool_names_exposes_openai_reasoning_defaults() {
        let names = default_builtin_tool_names(
            ModelProvider::Openai,
            "https://api.openai.com/v1",
            AuthMode::ApiKey,
            "sk-test",
            "gpt-5.3-codex",
        );
        assert_eq!(names, vec!["web_search", "code_interpreter"]);
    }

    // Ensures login-mode responses suppress builtins when policy requires it.
    #[test]
    fn default_builtin_tool_names_suppresses_login_mode_builtins() {
        let names = default_builtin_tool_names(
            ModelProvider::Openai,
            "https://api.openai.com/v1",
            AuthMode::Login,
            "",
            "gpt-5.3-codex",
        );
        assert!(names.is_empty());
    }
}
