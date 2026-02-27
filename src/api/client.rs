use super::completions;
use super::policy;
use super::responses::{self, ResponsesRequestOptions};
use crate::auth::{
    load_provider_tokens, login_provider_key_for_base_url, refresh_openai_tokens,
    save_provider_tokens,
};
use crate::config::{ApiConfig, ApiProtocol, AuthMode};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};

/// Client for OpenAI-compatible model APIs.
pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    protocol: ApiProtocol,
    auth: AuthMode,
    profile: String,
}

impl ApiClient {
    /// Build a client from resolved API configuration.
    pub fn new(config: &ApiConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.trim().to_string(),
            protocol: config.protocol,
            auth: config.auth,
            profile: config.profile.clone(),
        }
    }

    /// Send a model request and return a normalized chat-style response.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError> {
        let base_url = policy::runtime_base_url(&self.base_url, self.auth, &self.api_key);
        let mut bearer = self.resolve_bearer_token(false).await?;
        let mut response = self
            .dispatch_request(&base_url, request, bearer.as_deref())
            .await;

        // Login tokens may be revoked before local expiry; refresh once on 401.
        if matches!(response, Err(ApiError::Status(401, _)))
            && policy::uses_login_auth(self.auth, &self.api_key)
        {
            bearer = self.resolve_bearer_token(true).await?;
            response = self
                .dispatch_request(&base_url, request, bearer.as_deref())
                .await;
            if matches!(response, Err(ApiError::Status(401, _))) {
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
                let refreshed = refresh_openai_tokens(existing).await.map_err(|err| {
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
}
