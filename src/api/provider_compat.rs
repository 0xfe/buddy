//! Provider/model compatibility helpers for request-shape and reasoning behavior.
//!
//! Provider behavior is driven by explicit profile configuration when present.
//! `provider = "auto"` falls back to base-URL inference.

use crate::config::{supported_reasoning_efforts, ApiProtocol, ModelProvider, ReasoningEffort};
use serde_json::{json, Value};

/// Apply provider/model-specific request body overrides for `/chat/completions`.
pub(crate) fn apply_completions_overrides(
    provider: ModelProvider,
    model: &str,
    payload: &mut Value,
) {
    if !payload.is_object() {
        return;
    }
    if provider != ModelProvider::Openrouter {
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
pub(crate) fn responses_reasoning_config(
    provider: ModelProvider,
    model: &str,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<Value> {
    let supported = supported_reasoning_efforts(provider, ApiProtocol::Responses, model);
    if supported.is_empty() {
        return None;
    }

    // Request reasoning summaries so the REPL can render useful thinking text.
    let mut reasoning = serde_json::Map::new();
    reasoning.insert("summary".to_string(), Value::String("auto".to_string()));
    if let Some(effort) = reasoning_effort.filter(|candidate| supported.contains(candidate)) {
        reasoning.insert(
            "effort".to_string(),
            Value::String(effort.as_str().to_string()),
        );
    }
    Some(Value::Object(reasoning))
}

/// Return default OpenAI built-in tool declarations for `/responses` requests.
pub(crate) fn responses_builtin_tools(
    provider: ModelProvider,
    model: &str,
    login_mode: bool,
) -> Vec<Value> {
    if provider != ModelProvider::Openai {
        return Vec::new();
    }
    // ChatGPT/Codex login runtime currently rejects these server built-ins.
    // Keep request shape conservative in login mode.
    if login_mode {
        return Vec::new();
    }
    if supported_reasoning_efforts(provider, ApiProtocol::Responses, model).is_empty() {
        return Vec::new();
    }

    // For GPT-5/Codex reasoning profiles, expose OpenAI-native built-ins so the
    // model can choose server-side search/python flows when appropriate.
    vec![
        json!({"type":"web_search"}),
        json!({"type":"code_interpreter","container":{"type":"auto"}}),
    ]
}

/// Return true when an OpenRouter model should request reasoning output.
fn is_openrouter_reasoning_profile(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized.contains("deepseek")
        || normalized.contains("glm")
        || normalized.contains("kimi")
        || normalized.contains("reason")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Verifies explicit provider wins over base-URL heuristics and auto infers by URL.
    #[test]
    fn resolved_provider_prefers_explicit_or_auto_infers() {
        assert_eq!(
            ModelProvider::Auto.resolved("https://api.openai.com/v1"),
            ModelProvider::Openai
        );
        assert_eq!(
            ModelProvider::Auto.resolved("https://openrouter.ai/api/v1"),
            ModelProvider::Openrouter
        );
        assert_eq!(
            ModelProvider::Auto.resolved("https://api.moonshot.ai/v1"),
            ModelProvider::Moonshot
        );
        assert_eq!(
            ModelProvider::Auto.resolved("https://api.anthropic.com/v1"),
            ModelProvider::Anthropic
        );
        assert_eq!(
            ModelProvider::Auto.resolved("https://example.invalid/v1"),
            ModelProvider::Other
        );
        assert_eq!(
            ModelProvider::Openrouter.resolved("https://api.openai.com/v1"),
            ModelProvider::Openrouter
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
            ModelProvider::Openrouter,
            "deepseek/deepseek-v3.2",
            &mut payload,
        );
        assert_eq!(payload["include_reasoning"], true);
        assert_eq!(payload["reasoning"]["enabled"], true);
    }

    // Verifies OpenAI reasoning-summary config is enabled for codex/reasoning models only.
    #[test]
    fn responses_reasoning_config_only_for_openai_reasoning_models() {
        let openai = responses_reasoning_config(
            ModelProvider::Openai,
            "gpt-5.3-codex",
            Some(ReasoningEffort::High),
        );
        assert_eq!(openai, Some(json!({"summary":"auto","effort":"high"})));

        let non_reasoning = responses_reasoning_config(ModelProvider::Openai, "gpt-4o-mini", None);
        assert!(non_reasoning.is_none());

        let openrouter = responses_reasoning_config(
            ModelProvider::Openrouter,
            "deepseek/deepseek-v3.2",
            Some(ReasoningEffort::High),
        );
        assert!(openrouter.is_none());
    }

    // Verifies OpenAI built-ins are enabled only for OpenAI reasoning profiles.
    #[test]
    fn responses_builtin_tools_only_for_openai_reasoning_models() {
        let openai = responses_builtin_tools(ModelProvider::Openai, "gpt-5.3-codex", false);
        assert_eq!(openai.len(), 2);
        assert_eq!(openai[0]["type"], "web_search");
        assert_eq!(openai[1]["type"], "code_interpreter");

        let non_reasoning = responses_builtin_tools(ModelProvider::Openai, "gpt-4o-mini", false);
        assert!(non_reasoning.is_empty());

        let openrouter = responses_builtin_tools(ModelProvider::Openrouter, "gpt-5.3-codex", false);
        assert!(openrouter.is_empty());
    }

    // Verifies non-OpenAI providers never inherit OpenAI Responses-only defaults.
    #[test]
    fn anthropic_provider_never_enables_openai_responses_defaults() {
        let reasoning = responses_reasoning_config(
            ModelProvider::Anthropic,
            "claude-sonnet-4-5",
            Some(ReasoningEffort::High),
        );
        assert!(reasoning.is_none());

        let builtins =
            responses_builtin_tools(ModelProvider::Anthropic, "claude-sonnet-4-5", false);
        assert!(builtins.is_empty());
    }

    // Verifies login-mode requests suppress OpenAI Responses built-ins.
    #[test]
    fn responses_builtin_tools_disabled_in_login_mode() {
        let builtins = responses_builtin_tools(ModelProvider::Openai, "gpt-5.3-codex", true);
        assert!(builtins.is_empty());
    }

    // Verifies unsupported effort values are ignored instead of producing invalid payloads.
    #[test]
    fn responses_reasoning_config_filters_unsupported_effort_values() {
        let openai = responses_reasoning_config(
            ModelProvider::Openai,
            "gpt-5.1",
            Some(ReasoningEffort::Xhigh),
        )
        .expect("reasoning config");
        assert_eq!(openai, json!({"summary":"auto"}));
    }
}
