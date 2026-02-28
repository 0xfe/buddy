//! HTTP transport helpers for protocol-specific API requests.

use crate::api::completions;
use crate::api::policy;
use crate::api::responses::{self, ResponsesRequestOptions};
use crate::config::{ApiProtocol, AuthMode};
use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use std::time::Duration;

/// Build an HTTP client with timeout applied.
pub(super) fn build_http_client(timeout: Duration) -> reqwest::Client {
    // Fall back to reqwest defaults if builder creation fails for any reason.
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Dispatch one API request for the configured wire protocol.
pub(super) async fn dispatch_request(
    http: &reqwest::Client,
    protocol: ApiProtocol,
    auth: AuthMode,
    api_key: &str,
    base_url: &str,
    request: &ChatRequest,
    bearer: Option<&str>,
) -> Result<ChatResponse, ApiError> {
    // Dispatch by wire protocol while keeping a single normalized return type.
    match protocol {
        ApiProtocol::Completions => completions::request(http, base_url, request, bearer).await,
        ApiProtocol::Responses => {
            let options: ResponsesRequestOptions =
                policy::responses_request_options(base_url, auth, api_key);
            responses::request(http, base_url, request, bearer, options).await
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
    ApiError::status(code, body, retry_after_secs)
}
