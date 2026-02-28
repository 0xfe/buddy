//! Active model-profile selection helpers.
//!
//! This module updates both `agent.model` and resolved runtime API settings so
//! profile switches are immediately reflected in subsequent requests.

use crate::error::ConfigError;

use super::env::api_key_override_with;
use super::resolve::resolve_active_api_with;
use super::Config;

/// Switch the active profile to a configured `[models.<name>]` entry.
pub fn select_model_profile(config: &mut Config, profile_name: &str) -> Result<(), ConfigError> {
    let selected = profile_name.trim();
    if selected.is_empty() {
        return Err(ConfigError::Invalid(
            "model profile name must not be empty".to_string(),
        ));
    }

    // Resolve profile with the same env/key-file semantics used at startup.
    let resolved_api = resolve_active_api_with(
        &config.models,
        selected,
        api_key_override_env(),
        |name| std::env::var(name).ok(),
        |path| {
            std::fs::read_to_string(path).map_err(|e| {
                ConfigError::Invalid(format!(
                    "failed to read model profile api_key_file `{path}`: {e}"
                ))
            })
        },
    )?;

    config.agent.model = selected.to_string();
    config.api = resolved_api;
    Ok(())
}

/// Read runtime API key override from canonical/legacy env vars.
fn api_key_override_env() -> Option<String> {
    api_key_override_with(&|name| std::env::var(name).ok())
}
