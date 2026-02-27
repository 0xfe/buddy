use super::completions;
use super::policy;
use super::responses::{self, ResponsesRequestOptions};
use super::ModelClient;
use crate::auth::{
    load_provider_tokens, login_provider_key_for_base_url, refresh_openai_tokens_with_client,
    save_provider_tokens,
};
use crate::config::{ApiConfig, ApiProtocol, AuthMode};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use async_trait::async_trait;
use std::time::Duration;
use tokio::time::sleep;

/// Client for OpenAI-compatible model APIs.
pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    protocol: ApiProtocol,
    auth: AuthMode,
    profile: String,
    retry_policy: RetryPolicy,
}

#[derive(Clone, Copy, Debug)]
struct RetryPolicy {
    max_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(8),
        }
    }
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
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
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
        let mut bearer = self.resolve_bearer_token(false).await?;
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
            bearer = self.resolve_bearer_token(true).await?;
            response = self
                .dispatch_request_with_retries(&base_url, request, bearer.as_deref())
                .await;
            if response
                .as_ref()
                .err()
                .and_then(ApiError::status_code)
                .is_some_and(|status| status == 401)
            {
                return Err(ApiError::LoginRequired(format!(
                    "OpenAI login is no longer valid. Run `buddy login` and retry.",
                )));
            }
        }

        response
    }

    async fn resolve_bearer_token(&self, force_refresh: bool) -> Result<Option<String>, ApiError> {
        if !self.api_key.is_empty() {
            return Ok(Some(self.api_key.clone()));
        }
        if !policy::uses_login_auth(self.auth, &self.api_key) {
            return Ok(None);
        }
        if !policy::supports_login_for_base_url(&self.base_url) {
            return Err(ApiError::LoginRequired(format!(
                "Profile `{}` sets `auth = \"login\"`, but base URL `{}` is not an OpenAI login endpoint.",
                self.profile, self.base_url
            )));
        }
        let provider = login_provider_key_for_base_url(&self.base_url).ok_or_else(|| {
            ApiError::LoginRequired(format!(
                "Profile `{}` sets `auth = \"login\"`, but provider for `{}` is unsupported.",
                self.profile, self.base_url
            ))
        })?;

        let mut tokens = load_provider_tokens(provider).map_err(|err| {
            ApiError::LoginRequired(format!(
                "failed to read login state for provider `{}`: {}",
                provider, err
            ))
        })?;

        if tokens.is_none() {
            return Err(ApiError::LoginRequired(format!(
                "Provider `{}` requires login auth, but no saved login was found. Run `buddy login`.",
                provider
            )));
        }

        if let Some(existing) = tokens.as_ref() {
            if force_refresh || existing.is_expiring_soon() {
                let refreshed = refresh_openai_tokens_with_client(&self.http, existing)
                    .await
                    .map_err(|err| {
                        ApiError::LoginRequired(format!(
                            "failed to refresh `{}` login: {}. Run `buddy login`.",
                            provider, err
                        ))
                    })?;
                save_provider_tokens(provider, refreshed.clone()).map_err(|err| {
                    ApiError::LoginRequired(format!(
                        "failed to persist refreshed `{}` login: {}",
                        provider, err
                    ))
                })?;
                tokens = Some(refreshed);
            }
        }

        Ok(tokens.map(|t| t.access_token))
    }

    async fn dispatch_request(
        &self,
        base_url: &str,
        request: &ChatRequest,
        bearer: Option<&str>,
    ) -> Result<ChatResponse, ApiError> {
        match self.protocol {
            ApiProtocol::Completions => {
                completions::request(&self.http, base_url, request, bearer).await
            }
            ApiProtocol::Responses => {
                let options: ResponsesRequestOptions =
                    policy::responses_request_options(base_url, self.auth, &self.api_key);
                responses::request(&self.http, base_url, request, bearer, options).await
            }
        }
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
                    if !self.should_retry(&err, attempt) {
                        return Err(self.with_diagnostic_hints(err));
                    }
                    let delay = self.retry_delay_for(attempt, &err);
                    attempt = attempt.saturating_add(1);
                    sleep(delay).await;
                }
            }
        }
    }

    fn should_retry(&self, err: &ApiError, attempt: u32) -> bool {
        if attempt.saturating_add(1) >= self.retry_policy.max_attempts {
            return false;
        }
        match err {
            ApiError::Http(inner) => inner.is_timeout() || inner.is_connect(),
            ApiError::Status { code, .. } => *code == 429 || (*code >= 500 && *code <= 599),
            ApiError::LoginRequired(_) | ApiError::InvalidResponse(_) => false,
        }
    }

    fn retry_delay_for(&self, attempt: u32, err: &ApiError) -> Duration {
        if let Some(seconds) = err.retry_after_secs() {
            return Duration::from_secs(seconds.clamp(1, 300));
        }
        let pow = 2u32.saturating_pow(attempt);
        let millis = self
            .retry_policy
            .initial_backoff
            .as_millis()
            .saturating_mul(pow as u128)
            .min(self.retry_policy.max_backoff.as_millis());
        Duration::from_millis(millis as u64)
    }

    fn with_diagnostic_hints(&self, err: ApiError) -> ApiError {
        let Some(code) = err.status_code() else {
            return err;
        };
        let ApiError::Status {
            code: _,
            mut body,
            retry_after_secs,
        } = err
        else {
            return err;
        };

        if code == 404 && self.protocol == ApiProtocol::Responses {
            body.push_str(
                "\nHint: this endpoint may not support `/responses`; set `api = \"completions\"` for this model profile.",
            );
        }
        if code == 404 && self.protocol == ApiProtocol::Completions {
            body.push_str(
                "\nHint: this endpoint may not support `/chat/completions`; set `api = \"responses\"` for this model profile.",
            );
        }
        ApiError::status(code, body, retry_after_secs)
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

        let mut api = ApiConfig::default();
        api.base_url = format!("http://{addr}");
        api.api_key = "test-key".to_string();
        api.model = "dummy-model".to_string();
        api.protocol = ApiProtocol::Completions;

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

        let mut api = ApiConfig::default();
        api.base_url = format!("http://{addr}");
        api.api_key = "test-key".to_string();
        api.model = "dummy-model".to_string();
        api.protocol = ApiProtocol::Completions;

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
        let mut api = ApiConfig::default();
        api.protocol = ApiProtocol::Responses;
        api.base_url = "https://example.com/v1".to_string();
        let client =
            ApiClient::new_with_retry_policy(&api, Duration::from_secs(1), RetryPolicy::default());
        let err =
            client.with_diagnostic_hints(ApiError::status(404, "not found".to_string(), None));
        let text = err.to_string();
        assert!(text.contains("/responses"), "missing hint: {text}");
        assert!(
            text.contains("api = \"completions\""),
            "missing hint: {text}"
        );
    }
}
