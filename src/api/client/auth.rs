//! Login/API-key bearer resolution for API requests.
//!
//! Keeping this separate from the HTTP dispatch flow makes token behavior easy
//! to test and reason about without touching transport logic.

use crate::api::policy;
use crate::auth::{
    load_provider_tokens, login_provider_key_for_base_url, refresh_openai_tokens_with_client,
    save_provider_tokens,
};
use crate::config::AuthMode;
use crate::error::ApiError;

/// Resolve the bearer token used for outbound API requests.
///
/// Resolution order:
/// 1. Explicit `api_key`.
/// 2. Login-based provider token for `auth = "login"` profiles.
/// 3. No auth header when neither applies.
pub(super) async fn resolve_bearer_token(
    http: &reqwest::Client,
    base_url: &str,
    auth: AuthMode,
    api_key: &str,
    profile: &str,
    force_refresh: bool,
) -> Result<Option<String>, ApiError> {
    // Explicit keys always win over login token resolution.
    if !api_key.is_empty() {
        return Ok(Some(api_key.to_string()));
    }
    if !policy::uses_login_auth(auth, api_key) {
        return Ok(None);
    }
    if !policy::supports_login_for_base_url(base_url) {
        return Err(ApiError::LoginRequired(format!(
            "Profile `{profile}` sets `auth = \"login\"`, but base URL `{base_url}` is not an OpenAI login endpoint.",
        )));
    }
    let provider = login_provider_key_for_base_url(base_url).ok_or_else(|| {
        ApiError::LoginRequired(format!(
            "Profile `{profile}` sets `auth = \"login\"`, but provider for `{base_url}` is unsupported.",
        ))
    })?;

    let mut tokens = load_provider_tokens(provider).map_err(|err| {
        ApiError::LoginRequired(format!(
            "failed to read login state for provider `{provider}`: {err}"
        ))
    })?;

    // Missing provider tokens means the user has not completed login yet.
    if tokens.is_none() {
        return Err(ApiError::LoginRequired(format!(
            "Provider `{provider}` requires login auth, but no saved login was found. Run `buddy login`."
        )));
    }

    if let Some(existing) = tokens.as_ref() {
        // Refresh eagerly so requests are not sent with near-expiry credentials.
        if force_refresh || existing.is_expiring_soon() {
            let refreshed = refresh_openai_tokens_with_client(http, existing)
                .await
                .map_err(|err| {
                    ApiError::LoginRequired(format!(
                        "failed to refresh `{provider}` login: {err}. Run `buddy login`."
                    ))
                })?;
            save_provider_tokens(provider, refreshed.clone()).map_err(|err| {
                ApiError::LoginRequired(format!(
                    "failed to persist refreshed `{provider}` login: {err}"
                ))
            })?;
            tokens = Some(refreshed);
        }
    }

    Ok(tokens.map(|t| t.access_token))
}
