//! Provider detection and compatibility helpers.

use crate::config::ModelProvider;
use reqwest::Url;

/// Stable provider key used for OpenAI login token records.
pub(crate) const OPENAI_PROVIDER_KEY: &str = "openai";
/// Runtime base URL required for OpenAI login-backed API requests.
pub(crate) const OPENAI_CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Returns true when the model base URL appears to target OpenAI.
pub fn supports_openai_login(base_url: &str) -> bool {
    let normalized = base_url.trim().to_ascii_lowercase();
    normalized.contains("api.openai.com") || normalized.contains("chatgpt.com/backend-api/codex")
}

/// Resolve login provider key for a configured base URL.
pub fn login_provider_key_for_base_url(base_url: &str) -> Option<&'static str> {
    if supports_openai_login(base_url) {
        Some(OPENAI_PROVIDER_KEY)
    } else {
        None
    }
}

/// Returns true when this provider/base-url pair supports login auth.
pub fn supports_login_for_provider(provider: ModelProvider, base_url: &str) -> bool {
    match provider {
        ModelProvider::Auto => supports_openai_login(base_url),
        ModelProvider::Openai => true,
        ModelProvider::Openrouter
        | ModelProvider::Moonshot
        | ModelProvider::Anthropic
        | ModelProvider::Other => false,
    }
}

/// Resolve login provider key from explicit provider + URL fallback.
pub fn login_provider_key(provider: ModelProvider, base_url: &str) -> Option<&'static str> {
    if supports_login_for_provider(provider, base_url) {
        Some(OPENAI_PROVIDER_KEY)
    } else {
        None
    }
}

/// Resolve the runtime request base URL for OpenAI login auth.
///
/// OpenAI login tokens are accepted by ChatGPT Codex backend endpoints.
pub fn openai_login_runtime_base_url(base_url: &str) -> String {
    let normalized = base_url.trim().to_ascii_lowercase();
    if normalized.contains("api.openai.com") {
        OPENAI_CHATGPT_CODEX_BASE_URL.to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    }
}

/// Resolve provider key used for API-key secret storage.
///
/// Keys are provider-scoped so multiple model profiles on the same provider
/// can share one stored API key.
pub fn api_key_provider_key(provider: ModelProvider, base_url: &str) -> String {
    match provider.resolved(base_url) {
        ModelProvider::Openai => OPENAI_PROVIDER_KEY.to_string(),
        ModelProvider::Openrouter => "openrouter".to_string(),
        ModelProvider::Moonshot => "moonshot".to_string(),
        ModelProvider::Anthropic => "anthropic".to_string(),
        ModelProvider::Other | ModelProvider::Auto => {
            let host = Url::parse(base_url)
                .ok()
                .and_then(|url| url.host_str().map(|value| value.to_string()))
                .unwrap_or_else(|| "unknown".to_string());
            format!("other:{host}")
        }
    }
}
