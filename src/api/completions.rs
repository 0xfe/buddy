//! `/chat/completions` protocol request/parse helpers.

use crate::api::{parse_retry_after_secs, provider_compat};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use serde_json::Value;

/// Send one `/chat/completions` request and parse the chat response payload.
pub(crate) async fn request(
    http: &reqwest::Client,
    base_url: &str,
    request: &ChatRequest,
    bearer: Option<&str>,
) -> Result<ChatResponse, ApiError> {
    let url = format!("{base_url}/chat/completions");
    let payload = build_completions_payload(base_url, request)?;
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

    let payload = response.json::<Value>().await?;
    parse_completions_payload(&payload)
}

/// Build a `/chat/completions` payload with provider-specific compatibility tweaks.
fn build_completions_payload(base_url: &str, request: &ChatRequest) -> Result<Value, ApiError> {
    let mut payload = serde_json::to_value(request)
        .map_err(|err| ApiError::InvalidResponse(format!("invalid request payload: {err}")))?;
    provider_compat::apply_completions_overrides(base_url, &request.model, &mut payload);
    Ok(payload)
}

/// Parse and normalize one `/chat/completions` payload into the shared response shape.
fn parse_completions_payload(payload: &Value) -> Result<ChatResponse, ApiError> {
    let mut normalized = payload.clone();
    normalize_completions_content_shapes(&mut normalized);
    serde_json::from_value(normalized)
        .map_err(|err| ApiError::InvalidResponse(format!("invalid completions response: {err}")))
}

/// Coerce provider content arrays/objects into string content expected by `Message`.
fn normalize_completions_content_shapes(payload: &mut Value) {
    let Some(choices) = payload.get_mut("choices").and_then(Value::as_array_mut) else {
        return;
    };
    for choice in choices {
        let Some(message) = choice.get_mut("message").and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        match content {
            Value::Array(parts) => {
                let merged = merge_content_parts(parts);
                *content = merged.map(Value::String).unwrap_or(Value::Null);
            }
            Value::Object(part) => {
                let merged = part
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                *content = merged.map(Value::String).unwrap_or(Value::Null);
            }
            Value::String(_) | Value::Null => {}
            _ => {}
        }
    }
}

/// Merge multimodal content-parts arrays into one display text block.
fn merge_content_parts(parts: &[Value]) -> Option<String> {
    let mut text_parts = Vec::<String>::new();
    for part in parts {
        match part {
            Value::String(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed.to_string());
                }
            }
            Value::Object(obj) => {
                let from_text = obj
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                if let Some(text) = from_text {
                    text_parts.push(text);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {}
        }
    }
    if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use serde_json::json;

    // Verifies content arrays from OpenRouter-style assistant messages are merged.
    #[test]
    fn parse_completions_payload_merges_array_content_parts() {
        let raw = json!({
            "id":"chatcmpl_1",
            "choices":[{
                "index":0,
                "message":{
                    "role":"assistant",
                    "content":[
                        {"type":"text","text":"hello"},
                        {"type":"output_text","text":"world"}
                    ]
                },
                "finish_reason":"stop"
            }]
        });
        let parsed = parse_completions_payload(&raw).expect("parse");
        assert_eq!(
            parsed.choices[0].message.content.as_deref(),
            Some("hello\nworld")
        );
    }

    // Verifies provider-specific payload overrides add reasoning flags for OpenRouter profiles.
    #[test]
    fn build_completions_payload_applies_openrouter_reasoning_overrides() {
        let req = ChatRequest {
            model: "deepseek/deepseek-v3.2".to_string(),
            messages: vec![Message::user("hi")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let payload = build_completions_payload("https://openrouter.ai/api/v1", &req).expect("ok");
        assert_eq!(payload["include_reasoning"], true);
        assert_eq!(payload["reasoning"]["enabled"], true);
    }
}
