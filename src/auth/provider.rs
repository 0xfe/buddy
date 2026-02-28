//! Provider detection and compatibility helpers.

pub(crate) const OPENAI_PROVIDER_KEY: &str = "openai";
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
