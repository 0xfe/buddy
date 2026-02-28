//! Provider-specific API transport/runtime rules.

use super::responses::ResponsesRequestOptions;
use crate::auth::{openai_login_runtime_base_url, supports_openai_login};
use crate::config::AuthMode;

/// True when this request should use login-derived bearer tokens instead of API keys.
pub(crate) fn uses_login_auth(auth: AuthMode, api_key: &str) -> bool {
    auth == AuthMode::Login && api_key.trim().is_empty()
}

/// True when the configured base URL supports OpenAI login token auth.
pub(crate) fn supports_login_for_base_url(base_url: &str) -> bool {
    supports_openai_login(base_url)
}

/// Compute the actual API base URL used for runtime requests.
///
/// For OpenAI login auth, requests are routed to the ChatGPT Codex backend.
pub(crate) fn runtime_base_url(base_url: &str, auth: AuthMode, api_key: &str) -> String {
    if uses_login_auth(auth, api_key) && supports_login_for_base_url(base_url) {
        openai_login_runtime_base_url(base_url)
    } else {
        base_url.to_string()
    }
}

/// Derive `/responses` request toggles required by the selected auth/runtime mode.
pub(crate) fn responses_request_options(
    base_url: &str,
    auth: AuthMode,
    api_key: &str,
) -> ResponsesRequestOptions {
    let login_openai = uses_login_auth(auth, api_key) && supports_login_for_base_url(base_url);
    ResponsesRequestOptions {
        store_false: login_openai,
        stream: login_openai,
    }
}
