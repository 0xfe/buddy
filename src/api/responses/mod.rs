//! `/responses` protocol support.
//!
//! The module is split into:
//! - request builder (`request_builder`)
//! - non-streaming response parser (`response_parser`)
//! - SSE streaming parser (`sse_parser`)

mod request_builder;
mod response_parser;
mod sse_parser;

use crate::api::parse_retry_after_secs;
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use request_builder::build_responses_payload;
pub(crate) use response_parser::parse_responses_payload;
use serde_json::Value;
use sse_parser::parse_streaming_responses_payload;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ResponsesRequestOptions {
    pub(crate) store_false: bool,
    pub(crate) stream: bool,
}

/// Send one `/responses` request and normalize provider output.
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
