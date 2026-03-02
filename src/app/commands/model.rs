//! Model profile command helpers.
//!
//! This module owns `/model` selection and selector parsing so REPL command
//! logic stays isolated from top-level boot/runtime wiring.

use crate::app::entry::run_login_flow;
use buddy::auth::{
    api_key_provider_key, load_provider_api_key, save_provider_api_key, supports_login_for_provider,
};
use buddy::config::{
    persist_model_profile_api_key_env, persist_model_profile_auth, supported_reasoning_efforts,
    AuthMode, Config, ModelConfig, ModelProvider, ReasoningEffort,
};
use buddy::runtime::{BuddyRuntimeHandle, RuntimeCommand};
use buddy::ui::render::RenderSink;
use buddy::ui::terminal as term_ui;
use rpassword::prompt_password;
use std::io::{self, Write};

/// Model-switch command submission details returned to the REPL loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelSwitchSubmission {
    /// Selected model profile key.
    pub(crate) profile_name: String,
    /// Selected request model id shown in switch confirmation.
    pub(crate) model_name: String,
    /// Selected optional reasoning effort override.
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    /// Optional profile-auth patch applied by this switch.
    pub(crate) auth_patch: ModelAuthPatch,
}

/// Runtime patch for profile auth/key-source fields applied during switch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ModelAuthPatch {
    /// Override auth mode for the selected profile.
    pub(crate) auth_override: Option<AuthMode>,
    /// Optional override for `api_key_env`.
    pub(crate) api_key_env_override: Option<String>,
    /// Clear inline/file/env key sources before applying overrides.
    pub(crate) clear_key_sources: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelAuthChoice {
    /// Keep existing profile auth fields unchanged.
    KeepCurrent,
    /// Use provider login flow when available.
    Login,
    /// Persist API key to encrypted provider store.
    ApiKeyStored,
    /// Read API key from an environment variable name.
    ApiKeyEnvVar,
    /// Cancel `/model` command.
    Cancel,
}

/// Handle `/model` command behavior.
pub(crate) async fn handle_model_command(
    renderer: &dyn RenderSink,
    config: &mut Config,
    runtime: &BuddyRuntimeHandle,
    selector: Option<&str>,
    config_path_override: Option<&str>,
) -> Option<ModelSwitchSubmission> {
    // `/model` flow:
    // 1) choose target profile (selector or picker),
    // 2) choose auth setup for that profile,
    // 3) choose reasoning effort (when supported),
    // 4) submit runtime switch.
    if config.models.is_empty() {
        renderer.warn("No configured model profiles. Add `[models.<name>]` entries to buddy.toml.");
        return None;
    }

    let names = configured_model_profile_names(config);
    if names.len() == 1 {
        renderer.warn("Only one model profile is configured.");
        return None;
    }

    let selected_via_picker = selector.is_none();
    let profile_name = if let Some(selector) = selector {
        let selected_input = selector.trim();
        if selected_input.is_empty() {
            return None;
        }
        match resolve_model_profile_selector(config, &names, selected_input) {
            Ok(name) => name,
            Err(msg) => {
                renderer.warn(&msg);
                return None;
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
            Ok(None) => return None,
            Err(e) => {
                renderer.warn(&format!("failed to read model selection: {e}"));
                return None;
            }
        }
    };

    let auth_patch = if selected_via_picker {
        match configure_model_auth(renderer, config, &profile_name, config_path_override).await {
            Ok(AuthUpdateOutcome::Updated(patch)) => patch,
            Ok(AuthUpdateOutcome::Unchanged) => ModelAuthPatch::default(),
            Ok(AuthUpdateOutcome::Cancelled) => return None,
            Err(msg) => {
                renderer.warn(&msg);
                return None;
            }
        }
    } else {
        ModelAuthPatch::default()
    };

    let target_reasoning_effort = match choose_reasoning_effort(renderer, config, &profile_name) {
        Some(value) => value,
        None => return None,
    };

    let current_reasoning_effort = config
        .models
        .get(&config.agent.model)
        .and_then(|profile| profile.reasoning_effort);
    if profile_name == config.agent.model
        && target_reasoning_effort == current_reasoning_effort
        && auth_patch == ModelAuthPatch::default()
    {
        renderer.section(&format!("model profile already active: {profile_name}"));
        if let Some(effort) = current_reasoning_effort {
            renderer.field("reasoning_effort", effort.as_str());
        }
        eprintln!();
        return None;
    }

    if let Some(profile) = config.models.get_mut(&profile_name) {
        profile.reasoning_effort = target_reasoning_effort;
    }

    if let Err(e) = runtime
        .send(RuntimeCommand::SwitchModel {
            profile: profile_name.clone(),
            reasoning_effort: target_reasoning_effort,
            auth_override: auth_patch.auth_override,
            api_key_env_override: auth_patch.api_key_env_override.clone(),
            clear_key_sources: auth_patch.clear_key_sources,
        })
        .await
    {
        renderer.warn(&format!(
            "failed to submit model switch command for `{profile_name}`: {e}"
        ));
        return None;
    }

    let model_name = config
        .models
        .get(&profile_name)
        .map(|profile| resolved_profile_api_model(profile, &profile_name))
        .unwrap_or_else(|| profile_name.clone());
    Some(ModelSwitchSubmission {
        profile_name,
        model_name,
        reasoning_effort: target_reasoning_effort,
        auth_patch,
    })
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthUpdateOutcome {
    /// Auth settings were changed and persisted.
    Updated(ModelAuthPatch),
    /// Auth settings were left unchanged.
    Unchanged,
    /// User cancelled auth setup.
    Cancelled,
}

/// Prompt for model-profile auth strategy and persist any selected updates.
async fn configure_model_auth(
    renderer: &dyn RenderSink,
    config: &mut Config,
    profile_name: &str,
    config_path_override: Option<&str>,
) -> Result<AuthUpdateOutcome, String> {
    let Some(profile) = config.models.get(profile_name).cloned() else {
        return Ok(AuthUpdateOutcome::Unchanged);
    };
    let provider = profile.provider.resolved(profile.api_base_url.trim());
    let supports_login = supports_login_for_provider(provider, &profile.api_base_url);

    let choices = build_auth_choices(supports_login);
    let options = choices
        .iter()
        .map(|choice| match choice {
            ModelAuthChoice::KeepCurrent => {
                format!("keep current ({})", auth_mode_label(profile.auth))
            }
            ModelAuthChoice::Login => "login".to_string(),
            ModelAuthChoice::ApiKeyStored => "api key (stored securely)".to_string(),
            ModelAuthChoice::ApiKeyEnvVar => "api key via env var".to_string(),
            ModelAuthChoice::Cancel => "cancel".to_string(),
        })
        .collect::<Vec<_>>();

    let initial = choices
        .iter()
        .position(|choice| *choice == ModelAuthChoice::KeepCurrent)
        .unwrap_or(0);
    let selection = term_ui::pick_from_list(
        config.display.color,
        "authentication",
        "Pick auth method for this model profile.",
        &options,
        initial,
    )
    .map_err(|err| format!("failed to read auth selection: {err}"))?;
    let Some(selection_idx) = selection else {
        return Ok(AuthUpdateOutcome::Cancelled);
    };
    let choice = choices[selection_idx];

    match choice {
        ModelAuthChoice::KeepCurrent => Ok(AuthUpdateOutcome::Unchanged),
        ModelAuthChoice::Cancel => Ok(AuthUpdateOutcome::Cancelled),
        ModelAuthChoice::Login => {
            persist_model_profile_auth(config_path_override, profile_name, "login", true)
                .map_err(|err| format!("failed to persist login auth mode: {err}"))?;
            apply_auth_mode_to_profile(config, profile_name, AuthMode::Login, None);
            let provider_selector = api_key_provider_key(provider, &profile.api_base_url);
            run_login_flow(
                renderer,
                config,
                Some(provider_selector.as_str()),
                false,
                false,
            )
            .await?;
            Ok(AuthUpdateOutcome::Updated(ModelAuthPatch {
                auth_override: Some(AuthMode::Login),
                api_key_env_override: None,
                clear_key_sources: true,
            }))
        }
        ModelAuthChoice::ApiKeyStored => {
            persist_model_profile_auth(config_path_override, profile_name, "api-key", true)
                .map_err(|err| format!("failed to persist api-key auth mode: {err}"))?;
            apply_auth_mode_to_profile(config, profile_name, AuthMode::ApiKey, None);

            let provider_key = api_key_provider_key(provider, &profile.api_base_url);
            let existing = {
                let mut progress = renderer.progress("checking stored provider API key");
                let result = load_provider_api_key(&provider_key).map_err(|err| {
                    format!("failed to read stored API key for `{provider_key}`: {err}")
                });
                progress.finish();
                result?
            };
            if existing.is_none() {
                let key = prompt_password("Enter API key (input hidden): ")
                    .map_err(|err| format!("failed to read API key from terminal: {err}"))?;
                let trimmed = key.trim();
                if trimmed.is_empty() {
                    return Err("empty API key entered; cancelled model switch".to_string());
                }
                save_provider_api_key(&provider_key, trimmed).map_err(|err| {
                    format!("failed to save API key for provider `{provider_key}`: {err}")
                })?;
            } else {
                renderer.detail(&format!(
                    "using stored API key for provider `{provider_key}`."
                ));
            }
            Ok(AuthUpdateOutcome::Updated(ModelAuthPatch {
                auth_override: Some(AuthMode::ApiKey),
                api_key_env_override: None,
                clear_key_sources: true,
            }))
        }
        ModelAuthChoice::ApiKeyEnvVar => {
            let default_env = provider_default_api_key_env(provider);
            let env_name = prompt_env_name(default_env)?;
            persist_model_profile_api_key_env(config_path_override, profile_name, &env_name)
                .map_err(|err| format!("failed to persist api_key_env for profile: {err}"))?;
            apply_auth_mode_to_profile(config, profile_name, AuthMode::ApiKey, Some(&env_name));
            let env_set = std::env::var(&env_name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty());
            if !env_set {
                renderer.warn(&format!(
                    "env var `{env_name}` is unset or empty. Set it before sending prompts, or run /model again and choose API key storage."
                ));
            }
            Ok(AuthUpdateOutcome::Updated(ModelAuthPatch {
                auth_override: Some(AuthMode::ApiKey),
                api_key_env_override: Some(env_name),
                clear_key_sources: true,
            }))
        }
    }
}

/// Build auth picker choices for one provider.
fn build_auth_choices(supports_login: bool) -> Vec<ModelAuthChoice> {
    let mut choices = Vec::new();
    choices.push(ModelAuthChoice::KeepCurrent);
    if supports_login {
        choices.push(ModelAuthChoice::Login);
    }
    choices.push(ModelAuthChoice::ApiKeyStored);
    choices.push(ModelAuthChoice::ApiKeyEnvVar);
    choices.push(ModelAuthChoice::Cancel);
    choices
}

/// Convert auth enum to a stable user-facing label.
fn auth_mode_label(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::ApiKey => "api-key",
        AuthMode::Login => "login",
    }
}

/// Return provider-default API-key environment variable names.
fn provider_default_api_key_env(provider: ModelProvider) -> &'static str {
    match provider {
        ModelProvider::Openai => "OPENAI_API_KEY",
        ModelProvider::Openrouter => "OPENROUTER_API_KEY",
        ModelProvider::Moonshot => "MOONSHOT_API_KEY",
        ModelProvider::Anthropic => "ANTHROPIC_API_KEY",
        ModelProvider::Auto | ModelProvider::Other => "BUDDY_API_KEY",
    }
}

/// Prompt for an environment-variable name, defaulting to provider convention.
fn prompt_env_name(default_name: &str) -> Result<String, String> {
    eprint!("  env var for API key [{default_name}]: ");
    io::stderr()
        .flush()
        .map_err(|err| format!("failed to render env-var prompt: {err}"))?;
    let mut line = String::new();
    if io::stdin()
        .read_line(&mut line)
        .map_err(|err| format!("failed to read env-var input: {err}"))?
        == 0
    {
        return Err("no env var entered; cancelled model switch".to_string());
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default_name.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Update in-memory profile auth fields to match persisted auth choices.
fn apply_auth_mode_to_profile(
    config: &mut Config,
    profile_name: &str,
    auth_mode: AuthMode,
    api_key_env: Option<&str>,
) {
    let Some(profile) = config.models.get_mut(profile_name) else {
        return;
    };
    profile.auth = auth_mode;
    profile.api_key.clear();
    profile.api_key_file = None;
    profile.api_key_env = api_key_env.map(|value| value.to_string());
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
    fn build_auth_choices_includes_login_only_when_supported() {
        let with_login = build_auth_choices(true);
        assert!(with_login.contains(&ModelAuthChoice::Login));
        let without_login = build_auth_choices(false);
        assert!(!without_login.contains(&ModelAuthChoice::Login));
    }

    #[test]
    fn provider_default_api_key_envs_match_provider_conventions() {
        assert_eq!(
            provider_default_api_key_env(ModelProvider::Openai),
            "OPENAI_API_KEY"
        );
        assert_eq!(
            provider_default_api_key_env(ModelProvider::Openrouter),
            "OPENROUTER_API_KEY"
        );
        assert_eq!(
            provider_default_api_key_env(ModelProvider::Moonshot),
            "MOONSHOT_API_KEY"
        );
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
