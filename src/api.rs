//! HTTP client for OpenAI-compatible chat completion APIs.
//!
//! Sends `ChatRequest` payloads and parses `ChatResponse` results.
//! Conditionally omits the Authorization header for local models (Ollama, etc.)
//! that don't require authentication.

use crate::config::ApiConfig;
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};

/// Client for OpenAI-compatible chat completion APIs.
pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl ApiClient {
    /// Build a client from API configuration.
    pub fn new(config: &ApiConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
        }
    }

    /// Send a chat completion request and return the parsed response.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut req = self.http.post(&url).json(request);

        // Only add auth for non-empty keys (local models like Ollama skip this).
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Status(status, body));
        }

        let chat_response: ChatResponse = response.json().await?;
        Ok(chat_response)
    }
}
