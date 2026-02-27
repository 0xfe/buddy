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
}
