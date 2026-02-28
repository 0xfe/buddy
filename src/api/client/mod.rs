//! API client orchestration for OpenAI-compatible chat transports.
//!
//! The client facade here intentionally remains small:
//! - auth token resolution is delegated to `auth`.
//! - dispatch wiring is delegated to `transport`.
//! - retry policy logic is delegated to `retry`.

mod auth;
mod retry;
mod transport;

use super::policy;
use super::ModelClient;
use crate::config::{ApiConfig, ApiProtocol};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use async_trait::async_trait;
use retry::RetryPolicy;
use std::time::Duration;
use tokio::time::sleep;

/// Client for OpenAI-compatible model APIs.
pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    protocol: ApiProtocol,
    auth: crate::config::AuthMode,
    profile: String,
    retry_policy: RetryPolicy,
}

impl ApiClient {
    /// Build a client from resolved API configuration.
    pub fn new(config: &ApiConfig, timeout: Duration) -> Self {
        Self::new_with_retry_policy(config, timeout, RetryPolicy::default())
    }

    fn new_with_retry_policy(
        config: &ApiConfig,
        timeout: Duration,
        retry_policy: RetryPolicy,
    ) -> Self {
        let http = transport::build_http_client(timeout);
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.trim().to_string(),
            protocol: config.protocol,
            auth: config.auth,
            profile: config.profile.clone(),
            retry_policy,
        }
    }

    /// Send a model request and return a normalized chat-style response.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError> {
        let base_url = policy::runtime_base_url(&self.base_url, self.auth, &self.api_key);
        let mut bearer = auth::resolve_bearer_token(
            &self.http,
            &self.base_url,
            self.auth,
            &self.api_key,
            &self.profile,
            false,
        )
        .await?;
        let mut response = self
            .dispatch_request_with_retries(&base_url, request, bearer.as_deref())
            .await;

        // Login tokens may be revoked before local expiry; refresh once on 401.
        if response
            .as_ref()
            .err()
            .and_then(ApiError::status_code)
            .is_some_and(|status| status == 401)
            && policy::uses_login_auth(self.auth, &self.api_key)
        {
            bearer = auth::resolve_bearer_token(
                &self.http,
                &self.base_url,
                self.auth,
                &self.api_key,
                &self.profile,
                true,
            )
            .await?;
            response = self
                .dispatch_request_with_retries(&base_url, request, bearer.as_deref())
                .await;
            if response
                .as_ref()
                .err()
                .and_then(ApiError::status_code)
                .is_some_and(|status| status == 401)
            {
                return Err(ApiError::LoginRequired(
                    "OpenAI login is no longer valid. Run `buddy login` and retry.".to_string(),
                ));
            }
        }

        response
    }

    async fn dispatch_request(
        &self,
        base_url: &str,
        request: &ChatRequest,
        bearer: Option<&str>,
    ) -> Result<ChatResponse, ApiError> {
        transport::dispatch_request(
            &self.http,
            self.protocol,
            self.auth,
            &self.api_key,
            base_url,
            request,
            bearer,
        )
        .await
    }

    async fn dispatch_request_with_retries(
        &self,
        base_url: &str,
        request: &ChatRequest,
        bearer: Option<&str>,
    ) -> Result<ChatResponse, ApiError> {
        let mut attempt: u32 = 0;
        loop {
            let result = self.dispatch_request(base_url, request, bearer).await;
            match result {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if !self.retry_policy.should_retry(&err, attempt) {
                        return Err(transport::with_diagnostic_hints(self.protocol, err));
                    }
                    let delay = self.retry_policy.retry_delay_for(attempt, &err);
                    attempt = attempt.saturating_add(1);
                    sleep(delay).await;
                }
            }
        }
    }
}

#[async_trait]
impl ModelClient for ApiClient {
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError> {
        ApiClient::chat(self, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiProtocol;
    use crate::types::Message;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn api_client_respects_timeout_policy() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept one connection and intentionally keep it open so the client
        // must hit its configured timeout.
        let _accept = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let api = ApiConfig {
            base_url: format!("http://{addr}"),
            api_key: "test-key".to_string(),
            model: "dummy-model".to_string(),
            protocol: ApiProtocol::Completions,
            ..ApiConfig::default()
        };

        let client = ApiClient::new(&api, Duration::from_millis(50));
        let request = ChatRequest {
            model: api.model.clone(),
            messages: vec![Message::user("hello")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let err = client.chat(&request).await.expect_err("timeout expected");
        match err {
            ApiError::Http(inner) => {
                assert!(inner.is_timeout(), "unexpected error: {inner}");
            }
            other => panic!("expected timeout Http error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn api_client_retries_transient_429_with_retry_after() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let _server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request_buf = [0u8; 4096];
                let _ = stream.read(&mut request_buf).await;
                if attempt == 0 {
                    let response = concat!(
                        "HTTP/1.1 429 Too Many Requests\r\n",
                        "Content-Type: application/json\r\n",
                        "Retry-After: 1\r\n",
                        "Content-Length: 18\r\n",
                        "Connection: close\r\n",
                        "\r\n",
                        "{\"error\":\"rate\"}"
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                } else {
                    let body = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            }
        });

        let api = ApiConfig {
            base_url: format!("http://{addr}"),
            api_key: "test-key".to_string(),
            model: "dummy-model".to_string(),
            protocol: ApiProtocol::Completions,
            ..ApiConfig::default()
        };

        let retry_policy = RetryPolicy {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
        };
        let client = ApiClient::new_with_retry_policy(&api, Duration::from_secs(3), retry_policy);
        let request = ChatRequest {
            model: api.model.clone(),
            messages: vec![Message::user("hello")],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let response = client.chat(&request).await.expect("retry should recover");
        assert_eq!(
            response.choices[0].message.content.as_deref(),
            Some("done"),
            "unexpected response body"
        );
    }

    #[test]
    fn api_client_adds_protocol_mismatch_hint_to_404() {
        let api = ApiConfig {
            protocol: ApiProtocol::Responses,
            base_url: "https://example.com/v1".to_string(),
            ..ApiConfig::default()
        };
        let client =
            ApiClient::new_with_retry_policy(&api, Duration::from_secs(1), RetryPolicy::default());
        let err = transport::with_diagnostic_hints(
            client.protocol,
            ApiError::status(404, "not found".to_string(), None),
        );
        let text = err.to_string();
        assert!(text.contains("/responses"), "missing hint: {text}");
        assert!(
            text.contains("api = \"completions\""),
            "missing hint: {text}"
        );
    }
}
