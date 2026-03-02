//! Model profile command helpers.
//!
//! This module owns `/model` selection and selector parsing so REPL command
//! logic stays isolated from top-level boot/runtime wiring.

use buddy::config::{
    select_model_profile, supported_reasoning_efforts, Config, ModelConfig, ModelProvider,
    ReasoningEffort,
};
use buddy::runtime::{BuddyRuntimeHandle, RuntimeCommand};
use buddy::ui::render::RenderSink;
use buddy::ui::terminal as term_ui;

/// Handle `/model` command behavior.
pub(crate) async fn handle_model_command(
    renderer: &dyn RenderSink,
    config: &mut Config,
    runtime: &BuddyRuntimeHandle,
    selector: Option<&str>,
) {
    // `/model` flow:
    // 1) choose target profile (selector or picker),
    // 2) skip if already active,
    // 3) submit runtime switch and sync local config.
    if config.models.is_empty() {
        renderer.warn("No configured model profiles. Add `[models.<name>]` entries to buddy.toml.");
        return;
    }

    let names = configured_model_profile_names(config);
    if names.len() == 1 {
        renderer.warn("Only one model profile is configured.");
        return;
    }

    let profile_name = if let Some(selector) = selector {
        let selected_input = selector.trim();
        if selected_input.is_empty() {
            return;
        }
        match resolve_model_profile_selector(config, &names, selected_input) {
            Ok(name) => name,
            Err(msg) => {
                renderer.warn(&msg);
                return;
            }
        }
    } else {
        let options = model_picker_options(config, &names);
        let initial = names
            .iter()
            .position(|name| name == &config.agent.model)
            .unwrap_or(0);
        match term_ui::pick_from_list(
            config.display.color,
            "model profiles",
            "Use ↑/↓ to pick, Enter to confirm, Esc to cancel.",
            &options,
            initial,
        ) {
            Ok(Some(index)) => names[index].clone(),
            Ok(None) => return,
            Err(e) => {
                renderer.warn(&format!("failed to read model selection: {e}"));
                return;
            }
        }
    };

    let target_reasoning_effort = match choose_reasoning_effort(renderer, config, &profile_name) {
        Some(value) => value,
        None => return,
    };

    let current_reasoning_effort = config
        .models
        .get(&config.agent.model)
        .and_then(|profile| profile.reasoning_effort);
    if profile_name == config.agent.model && target_reasoning_effort == current_reasoning_effort {
        renderer.section(&format!("model profile already active: {profile_name}"));
        if let Some(effort) = current_reasoning_effort {
            renderer.field("reasoning_effort", effort.as_str());
        }
        eprintln!();
        return;
    }

    if let Err(e) = runtime
        .send(RuntimeCommand::SwitchModel {
            profile: profile_name.clone(),
            reasoning_effort: target_reasoning_effort,
        })
        .await
    {
        renderer.warn(&format!(
            "failed to submit model switch command for `{profile_name}`: {e}"
        ));
        return;
    }

    if let Some(profile) = config.models.get_mut(&profile_name) {
        profile.reasoning_effort = target_reasoning_effort;
    }
    if let Err(e) = select_model_profile(config, &profile_name) {
        renderer.warn(&format!(
            "runtime accepted model switch for `{profile_name}`, but local config sync failed: {e}"
        ));
    }
}

/// Return model profile keys in stable config-map iteration order.
pub(crate) fn configured_model_profile_names(config: &Config) -> Vec<String> {
    config.models.keys().cloned().collect()
}

/// Build visible picker option labels.
pub(crate) fn model_picker_options(config: &Config, names: &[String]) -> Vec<String> {
    let mut options = Vec::with_capacity(names.len());
    for (idx, name) in names.iter().enumerate() {
        let Some(profile) = config.models.get(name) else {
            continue;
        };
        let marker = if name == &config.agent.model {
            "*"
        } else {
            " "
        };
        let api_model = resolved_profile_api_model(profile, name);
        let provider = profile.provider.resolved(profile.api_base_url.trim());
        let value = format!(
            "{}.{} {} | {}",
            idx + 1,
            marker,
            api_model,
            provider_brand_name(provider)
        );
        options.push(value);
    }
    options
}

/// Pick optional reasoning effort for models supporting reasoning controls.
///
/// Returns:
/// - `Some(Some(level))` when reasoning is supported and user selected a level.
/// - `Some(None)` when reasoning is unsupported for selected profile.
/// - `None` when user cancelled the picker.
fn choose_reasoning_effort(
    renderer: &dyn RenderSink,
    config: &Config,
    profile_name: &str,
) -> Option<Option<ReasoningEffort>> {
    let Some(profile) = config.models.get(profile_name) else {
        return Some(None);
    };
    let model = resolved_profile_api_model(profile, profile_name);
    let supported = supported_reasoning_efforts(profile.provider, profile.api, &model);
    if supported.is_empty() {
        return Some(None);
    }

    let options = supported
        .iter()
        .map(|effort| effort.as_str().to_string())
        .collect::<Vec<_>>();
    let selected = profile.reasoning_effort.or_else(|| {
        supported
            .iter()
            .find(|value| **value == ReasoningEffort::Medium)
            .copied()
            .or_else(|| supported.first().copied())
    });
    let initial = selected
        .and_then(|value| supported.iter().position(|candidate| *candidate == value))
        .unwrap_or(0);
    match term_ui::pick_from_list(
        config.display.color,
        "reasoning effort",
        "Use ↑/↓ to pick, Enter to confirm, Esc to cancel.",
        &options,
        initial,
    ) {
        Ok(Some(index)) => Some(Some(supported[index])),
        Ok(None) => None,
        Err(e) => {
            renderer.warn(&format!("failed to read reasoning effort selection: {e}"));
            None
        }
    }
}

/// Resolve the request model-id shown in selection UI.
pub(crate) fn resolved_profile_api_model(profile: &ModelConfig, profile_name: &str) -> String {
    profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(profile_name)
        .to_string()
}

/// Branded provider label for picker and status surfaces.
fn provider_brand_name(provider: ModelProvider) -> &'static str {
    match provider {
        ModelProvider::Openai => "OpenAI",
        ModelProvider::Moonshot => "Moonshot AI",
        ModelProvider::Openrouter => "OpenRouter",
        ModelProvider::Anthropic => "Anthropic",
        ModelProvider::Auto => "Auto",
        ModelProvider::Other => "Other",
    }
}

/// Resolve model selector input into a configured profile key.
pub(crate) fn resolve_model_profile_selector(
    config: &Config,
    names: &[String],
    selector: &str,
) -> Result<String, String> {
    let trimmed = normalize_model_selector(selector);
    if trimmed.is_empty() {
        return Err("Usage: /model <name|index>".to_string());
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 || index > names.len() {
            return Err(format!(
                "Model index out of range: {index}. Choose 1-{}.",
                names.len()
            ));
        }
        return Ok(names[index - 1].clone());
    }

    if config.models.contains_key(trimmed) {
        return Ok(trimmed.to_string());
    }

    let normalized = trimmed.to_ascii_lowercase();
    let mut matches = config
        .models
        .keys()
        .filter(|name| name.to_ascii_lowercase() == normalized)
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Ok(matches.remove(0));
    }

    Err(format!(
        "Unknown model profile `{trimmed}`. Use /model to pick from configured profiles."
    ))
}

/// Normalize selector input, supporting both raw value and `/model ...` forms.
pub(crate) fn normalize_model_selector(selector: &str) -> &str {
    let trimmed = selector.trim();
    if !trimmed.starts_with('/') {
        return trimmed;
    }

    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return trimmed;
    };
    if command.eq_ignore_ascii_case("/model") {
        return parts.next().unwrap_or("");
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::config::{ApiProtocol, ModelProvider};

    #[test]
    fn resolve_model_profile_selector_accepts_index_and_name() {
        // Selector parser should accept both explicit names and 1-based indices.
        let mut cfg = Config::default();
        cfg.models.insert(
            "kimi".to_string(),
            ModelConfig {
                api_base_url: "https://api.moonshot.ai/v1".to_string(),
                model: Some("moonshot-v1".to_string()),
                ..ModelConfig::default()
            },
        );
        let names = configured_model_profile_names(&cfg);

        let by_name = resolve_model_profile_selector(&cfg, &names, "kimi").unwrap();
        assert_eq!(by_name, "kimi");

        let by_index = resolve_model_profile_selector(&cfg, &names, "2").unwrap();
        assert_eq!(by_index, names[1]);
    }

    #[test]
    fn resolve_model_profile_selector_accepts_slash_prefixed_input() {
        // Parser should tolerate full `/model ...` forms from command history/paste.
        let mut cfg = Config::default();
        cfg.models.insert(
            "kimi".to_string(),
            ModelConfig {
                api_base_url: "https://api.moonshot.ai/v1".to_string(),
                model: Some("moonshot-v1".to_string()),
                ..ModelConfig::default()
            },
        );
        let names = configured_model_profile_names(&cfg);

        let by_prefixed_name = resolve_model_profile_selector(&cfg, &names, "/model kimi").unwrap();
        assert_eq!(by_prefixed_name, "kimi");

        let by_prefixed_index = resolve_model_profile_selector(&cfg, &names, "/model 2").unwrap();
        assert_eq!(by_prefixed_index, names[1]);
    }

    #[test]
    fn resolve_model_profile_selector_rejects_unknown() {
        // Unknown selectors should return a user-facing guidance message.
        let cfg = Config::default();
        let names = configured_model_profile_names(&cfg);
        let err = resolve_model_profile_selector(&cfg, &names, "missing").unwrap_err();
        assert!(err.contains("Unknown model profile"));
    }

    #[test]
    fn model_picker_options_show_model_and_branded_provider_only() {
        let mut cfg = Config::default();
        cfg.models.insert(
            "kimi".to_string(),
            ModelConfig {
                api_base_url: "https://api.moonshot.ai/v1".to_string(),
                provider: ModelProvider::Moonshot,
                api: ApiProtocol::Completions,
                model: Some("kimi-k2.5".to_string()),
                ..ModelConfig::default()
            },
        );
        let names = configured_model_profile_names(&cfg);
        let options = model_picker_options(&cfg, &names);
        assert!(options
            .iter()
            .any(|line| line.contains("gpt-5.3-codex-spark | OpenAI")));
        assert!(options
            .iter()
            .any(|line| line.contains("kimi-k2.5 | Moonshot AI")));
    }

    #[test]
    fn provider_brand_names_are_stable() {
        assert_eq!(provider_brand_name(ModelProvider::Openai), "OpenAI");
        assert_eq!(provider_brand_name(ModelProvider::Openrouter), "OpenRouter");
        assert_eq!(provider_brand_name(ModelProvider::Moonshot), "Moonshot AI");
        assert_eq!(provider_brand_name(ModelProvider::Anthropic), "Anthropic");
    }

    #[test]
    fn reasoning_picker_support_matrix_matches_openai_responses_profiles() {
        let profile = ModelConfig {
            provider: ModelProvider::Openai,
            api: ApiProtocol::Responses,
            model: Some("gpt-5.3-codex".to_string()),
            ..ModelConfig::default()
        };
        let model = resolved_profile_api_model(&profile, "gpt-codex");
        let supported = supported_reasoning_efforts(profile.provider, profile.api, &model);
        assert!(supported.contains(&ReasoningEffort::Low));
        assert!(supported.contains(&ReasoningEffort::Xhigh));
    }

    #[test]
    fn reasoning_picker_skips_unsupported_profiles() {
        let profile = ModelConfig {
            provider: ModelProvider::Openrouter,
            api: ApiProtocol::Completions,
            model: Some("deepseek/deepseek-v3.2".to_string()),
            ..ModelConfig::default()
        };
        let model = resolved_profile_api_model(&profile, "openrouter-deepseek");
        assert!(supported_reasoning_efforts(profile.provider, profile.api, &model).is_empty());
    }
}
