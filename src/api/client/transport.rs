//! HTTP transport helpers for protocol-specific API requests.

use crate::api::anthropic;
use crate::api::completions;
use crate::api::policy;
use crate::api::responses::{self, ResponsesRequestOptions};
use crate::config::{ApiProtocol, AuthMode, ModelProvider};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use std::time::Duration;

/// Borrowed request parameters required for one protocol dispatch.
pub(super) struct DispatchRequest<'a> {
    /// Shared reqwest client used for HTTP calls.
    pub(super) http: &'a reqwest::Client,
    /// Selected wire protocol for this profile.
    pub(super) protocol: ApiProtocol,
    /// Resolved provider family for compatibility behavior.
    pub(super) provider: ModelProvider,
    /// Selected auth mode for this profile.
    pub(super) auth: AuthMode,
    /// Configured API key value (can be empty under login auth).
    pub(super) api_key: &'a str,
    /// Runtime base URL (already normalized).
    pub(super) base_url: &'a str,
    /// Normalized chat request payload.
    pub(super) request: &'a ChatRequest,
    /// Optional resolved bearer token.
    pub(super) bearer: Option<&'a str>,
}

/// Build an HTTP client with timeout applied.
pub(super) fn build_http_client(timeout: Duration) -> reqwest::Client {
    // Fall back to reqwest defaults if builder creation fails for any reason.
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Dispatch one API request for the configured wire protocol.
pub(super) async fn dispatch_request(args: DispatchRequest<'_>) -> Result<ChatResponse, ApiError> {
    let DispatchRequest {
        http,
        protocol,
        provider,
        auth,
        api_key,
        base_url,
        request,
        bearer,
    } = args;
    // Dispatch by wire protocol while keeping a single normalized return type.
    match protocol {
        ApiProtocol::Completions => {
            completions::request(http, base_url, provider, request, bearer).await
        }
        ApiProtocol::Responses => {
            let options: ResponsesRequestOptions = policy::responses_request_options(
                provider,
                base_url,
                auth,
                api_key,
                &request.model,
            );
            responses::request(http, base_url, request, bearer, options).await
        }
        ApiProtocol::Anthropic => {
            let api_key = bearer
                .filter(|value| !value.trim().is_empty())
                .or_else(|| (!api_key.trim().is_empty()).then_some(api_key));
            anthropic::request(http, base_url, request, api_key).await
        }
    }
}

/// Add protocol mismatch hints to 404 responses.
pub(super) fn with_diagnostic_hints(protocol: ApiProtocol, err: ApiError) -> ApiError {
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

    // 404 often means the endpoint supports a different API protocol path.
    if code == 404 && protocol == ApiProtocol::Responses {
        body.push_str(
            "\nHint: this endpoint may not support `/responses`; set `api = \"completions\"` for this model profile.",
        );
    }
    if code == 404 && protocol == ApiProtocol::Completions {
        body.push_str(
            "\nHint: this endpoint may not support `/chat/completions`; set `api = \"responses\"` for this model profile.",
        );
    }
    if code == 404 && protocol == ApiProtocol::Anthropic {
        body.push_str(
            "\nHint: this endpoint may not support `/messages`; set `api = \"anthropic\"` for Anthropic model profiles.",
        );
    }
    ApiError::status(code, body, retry_after_secs)
}
