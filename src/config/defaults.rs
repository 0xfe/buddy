//! Default configuration constants and profile templates.
//!
//! Keeping defaults in one module makes behavior-preserving refactors safer:
//! callers can share the same constants without duplicating literals.

use std::collections::BTreeMap;

use super::{ApiProtocol, AuthMode, ModelConfig};

/// Embedded default `buddy.toml` template written by `buddy init`.
pub(super) const DEFAULT_BUDDY_CONFIG_TEMPLATE: &str = include_str!("../templates/buddy.toml");
/// Default profile key selected when no profile is specified.
pub(super) const DEFAULT_MODEL_PROFILE_NAME: &str = "gpt-codex";
/// Default provider model ID used by the default profile.
pub(super) const DEFAULT_MODEL_ID: &str = "gpt-5.3-codex";
/// Default OpenAI-compatible API base URL.
pub(super) const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";
/// Default timeout for model API requests.
pub(super) const DEFAULT_API_TIMEOUT_SECS: u64 = 120;
/// Default timeout for `fetch_url` tool requests.
pub(super) const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 20;
/// Default operator/agent display name.
pub(super) const DEFAULT_AGENT_NAME: &str = "agent-mo";

/// Default set of model profiles bundled with Buddy.
pub(super) fn default_models_map() -> BTreeMap<String, ModelConfig> {
    let mut models = BTreeMap::new();
    // First-party OpenAI profile using login-based auth by default.
    models.insert(
        DEFAULT_MODEL_PROFILE_NAME.to_string(),
        ModelConfig {
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            api: ApiProtocol::Responses,
            auth: AuthMode::Login,
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: Some(DEFAULT_MODEL_ID.to_string()),
            context_limit: None,
        },
    );
    // Alternate OpenAI profile targeting the `spark` variant.
    models.insert(
        "gpt-spark".to_string(),
        ModelConfig {
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            api: ApiProtocol::Responses,
            auth: AuthMode::Login,
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: Some("gpt-5.3-codex-spark".to_string()),
            context_limit: None,
        },
    );
    // OpenRouter profile pre-wired for DeepSeek with env-based key lookup.
    models.insert(
        "openrouter-deepseek".to_string(),
        ModelConfig {
            api_base_url: "https://openrouter.ai/api/v1".to_string(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            api_key_file: None,
            model: Some("deepseek/deepseek-v3.2".to_string()),
            context_limit: None,
        },
    );
    // OpenRouter profile pre-wired for GLM family models.
    models.insert(
        "openrouter-glm".to_string(),
        ModelConfig {
            api_base_url: "https://openrouter.ai/api/v1".to_string(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            api_key_file: None,
            model: Some("z-ai/glm-5".to_string()),
            context_limit: None,
        },
    );
    // Moonshot Kimi profile with explicit provider endpoint and env key.
    models.insert(
        "kimi".to_string(),
        ModelConfig {
            api_base_url: "https://api.moonshot.ai/v1".to_string(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: Some("MOONSHOT_API_KEY".to_string()),
            api_key_file: None,
            model: Some("kimi-k2.5".to_string()),
            context_limit: None,
        },
    );
    models
}
