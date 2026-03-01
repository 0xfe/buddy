//! Provider/model compatibility helpers for request-shape and reasoning behavior.
//!
//! OpenAI-compatible providers differ in how they expose reasoning:
//! - OpenAI `/responses` can emit reasoning summary events when requested.
//! - OpenRouter `/chat/completions` often needs explicit reasoning flags.
//! - Moonshot exposes `reasoning_content` on chat-completions responses.
//!
//! This module centralizes small provider/model conditionals so protocol modules
//! stay focused on transport and parsing.

use crate::auth::supports_openai_login;
use serde_json::{json, Value};

/// Provider family inferred from the configured API base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderFamily {
    /// OpenAI-hosted endpoints (`api.openai.com`, ChatGPT codex runtime).
    OpenAI,
    /// OpenRouter proxy endpoints.
    OpenRouter,
    /// Moonshot native endpoints.
    Moonshot,
    /// Any other OpenAI-compatible provider.
    Other,
}

/// Infer provider family from an API base URL.
pub(crate) fn provider_family(base_url: &str) -> ProviderFamily {
    let normalized = base_url.trim().to_ascii_lowercase();
    if normalized.contains("openrouter.ai") {
        return ProviderFamily::OpenRouter;
    }
    if normalized.contains("moonshot.ai") {
        return ProviderFamily::Moonshot;
    }
    if supports_openai_login(base_url) {
        return ProviderFamily::OpenAI;
    }
    ProviderFamily::Other
}

/// Apply provider/model-specific request body overrides for `/chat/completions`.
pub(crate) fn apply_completions_overrides(base_url: &str, model: &str, payload: &mut Value) {
    if !payload.is_object() {
        return;
    }
    if provider_family(base_url) != ProviderFamily::OpenRouter {
        return;
    }
    if !is_openrouter_reasoning_profile(model) {
        return;
    }

    let Some(map) = payload.as_object_mut() else {
        return;
    };
    // OpenRouter compatibility: request surfaced reasoning when supported.
    map.entry("include_reasoning".to_string())
        .or_insert_with(|| Value::Bool(true));
    map.entry("reasoning".to_string())
        .or_insert_with(|| json!({}));

    // DeepSeek V3.2 documents `reasoning.enabled` controls explicitly.
    if model.to_ascii_lowercase().contains("deepseek-v3.2") {
        if let Some(reasoning) = map.get_mut("reasoning").and_then(Value::as_object_mut) {
            reasoning
                .entry("enabled".to_string())
                .or_insert_with(|| Value::Bool(true));
        }
    }
}

/// Return default `/responses` reasoning config for this provider/model pair.
pub(crate) fn responses_reasoning_config(base_url: &str, model: &str) -> Option<Value> {
    if provider_family(base_url) != ProviderFamily::OpenAI {
        return None;
    }
    if !is_openai_reasoning_model(model) {
        return None;
    }
    // Request reasoning summaries so the REPL can render useful thinking text.
    Some(json!({ "summary": "auto" }))
}

/// Return true when an OpenRouter model should request reasoning output.
fn is_openrouter_reasoning_profile(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized.contains("deepseek")
        || normalized.contains("glm")
        || normalized.contains("kimi")
        || normalized.contains("reason")
}

/// Return true for model IDs that commonly support OpenAI reasoning summaries.
fn is_openai_reasoning_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized.contains("gpt-5")
        || normalized.contains("codex")
        || normalized.starts_with("o1")
        || normalized.starts_with("o3")
        || normalized.starts_with("o4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Verifies provider inference for OpenAI, OpenRouter, Moonshot, and unknown hosts.
    #[test]
    fn provider_family_detection() {
        assert_eq!(
            provider_family("https://api.openai.com/v1"),
            ProviderFamily::OpenAI
        );
        assert_eq!(
            provider_family("https://chatgpt.com/backend-api/codex"),
            ProviderFamily::OpenAI
        );
        assert_eq!(
            provider_family("https://openrouter.ai/api/v1"),
            ProviderFamily::OpenRouter
        );
        assert_eq!(
            provider_family("https://api.moonshot.ai/v1"),
            ProviderFamily::Moonshot
        );
        assert_eq!(
            provider_family("https://example.invalid/v1"),
            ProviderFamily::Other
        );
    }

    // Verifies OpenRouter reasoning defaults are injected for DeepSeek/GLM-style profiles.
    #[test]
    fn apply_completions_overrides_sets_openrouter_reasoning_fields() {
        let mut payload = json!({
            "model": "deepseek/deepseek-v3.2",
            "messages": [{"role":"user","content":"hi"}]
        });
        apply_completions_overrides(
            "https://openrouter.ai/api/v1",
            "deepseek/deepseek-v3.2",
            &mut payload,
        );
        assert_eq!(payload["include_reasoning"], true);
        assert_eq!(payload["reasoning"]["enabled"], true);
    }

    // Verifies OpenAI reasoning-summary config is enabled for codex/reasoning models only.
    #[test]
    fn responses_reasoning_config_only_for_openai_reasoning_models() {
        let openai = responses_reasoning_config("https://api.openai.com/v1", "gpt-5.3-codex");
        assert_eq!(openai, Some(json!({"summary":"auto"})));

        let non_reasoning = responses_reasoning_config("https://api.openai.com/v1", "gpt-4o-mini");
        assert!(non_reasoning.is_none());

        let openrouter =
            responses_reasoning_config("https://openrouter.ai/api/v1", "deepseek/deepseek-v3.2");
        assert!(openrouter.is_none());
    }
}
