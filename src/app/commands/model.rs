//! Model profile command helpers.
//!
//! This module owns `/model` selection and selector parsing so REPL command
//! logic stays isolated from top-level boot/runtime wiring.

use buddy::config::{select_model_profile, Config, ModelConfig};
use buddy::render::RenderSink;
use buddy::runtime::{BuddyRuntimeHandle, RuntimeCommand};
use buddy::tui as repl;

/// Handle `/model` command behavior.
pub(crate) async fn handle_model_command(
    renderer: &dyn RenderSink,
    config: &mut Config,
    runtime: &BuddyRuntimeHandle,
    selector: Option<&str>,
) {
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
        match repl::pick_from_list(
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

    if profile_name == config.agent.model {
        renderer.section(&format!("model profile already active: {profile_name}"));
        eprintln!();
        return;
    }

    if let Err(e) = runtime
        .send(RuntimeCommand::SwitchModel {
            profile: profile_name.clone(),
        })
        .await
    {
        renderer.warn(&format!(
            "failed to submit model switch command for `{profile_name}`: {e}"
        ));
        return;
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
        let value = format!(
            "{}.{} {} | {} | {:?} | {:?}",
            idx + 1,
            marker,
            api_model,
            profile.api_base_url.trim(),
            profile.api,
            profile.auth
        );
        options.push(value);
    }
    options
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

    #[test]
    fn resolve_model_profile_selector_accepts_index_and_name() {
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
        let cfg = Config::default();
        let names = configured_model_profile_names(&cfg);
        let err = resolve_model_profile_selector(&cfg, &names, "missing").unwrap_err();
        assert!(err.contains("Unknown model profile"));
    }
}
