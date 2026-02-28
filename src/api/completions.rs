//! `/chat/completions` protocol request helper.

use crate::api::parse_retry_after_secs;
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};

/// Send one `/chat/completions` request and parse the chat response payload.
pub(crate) async fn request(
    http: &reqwest::Client,
    base_url: &str,
    request: &ChatRequest,
    bearer: Option<&str>,
) -> Result<ChatResponse, ApiError> {
    let url = format!("{base_url}/chat/completions");
    let mut req = http.post(&url).json(request);
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

    response
        .json::<ChatResponse>()
        .await
        .map_err(ApiError::from)
}
