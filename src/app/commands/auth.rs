//! Auth command selector helpers.
//!
//! This module centralizes provider selection logic for `login`/`logout`
//! commands so CLI and REPL share one resolution path.

use super::model::{configured_model_profile_names, resolve_model_profile_selector};
use buddy::auth::api_key_provider_key;
use buddy::config::Config;

/// Result of resolving auth command selector input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthProviderSelection {
    /// Stable provider key used by auth storage.
    pub(crate) provider_key: String,
    /// Canonical user-facing provider label.
    pub(crate) provider_label: String,
    /// Optional legacy profile input, used to emit deprecation guidance.
    pub(crate) legacy_profile: Option<String>,
}

/// Resolve an auth provider from optional selector text.
///
/// Selector precedence:
/// 1) explicit provider alias (`openai`, `openrouter`, `moonshot`/`kimi`, `anthropic`/`claude`)
/// 2) configured profile selector (`<name>` or `<index>`) for compatibility
/// 3) active profile provider when selector is omitted
pub(crate) fn resolve_auth_provider_selector(
    config: &Config,
    selector: Option<&str>,
    command_name: &str,
) -> Result<AuthProviderSelection, String> {
    if config.models.is_empty() {
        return Err(
            "No configured model profiles. Add `[models.<name>]` entries to buddy.toml."
                .to_string(),
        );
    }

    let normalized = normalize_auth_selector(selector.unwrap_or(""), command_name);
    if normalized.is_empty() {
        return selection_from_profile(config, &config.agent.model, None);
    }

    if let Some(provider) = provider_alias(normalized) {
        return Ok(AuthProviderSelection {
            provider_key: provider.to_string(),
            provider_label: provider.to_string(),
            legacy_profile: None,
        });
    }

    let names = configured_model_profile_names(config);
    let profile_name = resolve_model_profile_selector(config, &names, normalized)?;
    selection_from_profile(config, &profile_name, Some(profile_name.clone()))
}

/// Normalize auth selector input, tolerating pasted `/login ...` and
/// `/logout ...` command forms.
fn normalize_auth_selector<'a>(selector: &'a str, command_name: &str) -> &'a str {
    let trimmed = selector.trim();
    if !trimmed.starts_with('/') {
        return trimmed;
    }
    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return trimmed;
    };
    let matches = command.eq_ignore_ascii_case(&format!("/{command_name}"));
    if matches {
        return parts.next().unwrap_or("");
    }
    trimmed
}

/// Convert known provider aliases into stable provider keys.
fn provider_alias(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "openai" => Some("openai"),
        "openrouter" => Some("openrouter"),
        "moonshot" | "kimi" => Some("moonshot"),
        "anthropic" | "claude" => Some("anthropic"),
        _ => None,
    }
}

/// Build provider selection from one configured model profile.
fn selection_from_profile(
    config: &Config,
    profile_name: &str,
    legacy_profile: Option<String>,
) -> Result<AuthProviderSelection, String> {
    let Some(profile) = config.models.get(profile_name) else {
        return Err(format!("unknown profile `{profile_name}`"));
    };
    let provider_key = api_key_provider_key(profile.provider, &profile.api_base_url);
    let provider_label = provider_key
        .split(':')
        .next()
        .unwrap_or(provider_key.as_str())
        .to_string();
    Ok(AuthProviderSelection {
        provider_key,
        provider_label,
        legacy_profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::config::{Config, ModelConfig, ModelProvider};

    #[test]
    fn resolve_auth_provider_selector_accepts_provider_aliases() {
        let cfg = Config::default();
        let openai =
            resolve_auth_provider_selector(&cfg, Some("openai"), "login").expect("provider");
        assert_eq!(openai.provider_key, "openai");

        let moonshot =
            resolve_auth_provider_selector(&cfg, Some("kimi"), "login").expect("provider");
        assert_eq!(moonshot.provider_key, "moonshot");
    }

    #[test]
    fn resolve_auth_provider_selector_accepts_profile_for_compat() {
        let mut cfg = Config::default();
        cfg.models.insert(
            "moon".to_string(),
            ModelConfig {
                api_base_url: "https://api.moonshot.ai/v1".to_string(),
                provider: ModelProvider::Moonshot,
                model: Some("kimi-k2.5".to_string()),
                ..ModelConfig::default()
            },
        );
        let resolved =
            resolve_auth_provider_selector(&cfg, Some("moon"), "login").expect("provider");
        assert_eq!(resolved.provider_key, "moonshot");
        assert_eq!(resolved.legacy_profile.as_deref(), Some("moon"));
    }

    #[test]
    fn resolve_auth_provider_selector_defaults_to_active_profile_provider() {
        let mut cfg = Config::default();
        cfg.agent.model = "openrouter-deepseek".to_string();
        let resolved = resolve_auth_provider_selector(&cfg, None, "login").expect("provider");
        assert_eq!(resolved.provider_key, "openrouter");
    }

    #[test]
    fn normalize_auth_selector_handles_prefixed_forms() {
        assert_eq!(normalize_auth_selector("/login openai", "login"), "openai");
        assert_eq!(normalize_auth_selector("/logout kimi", "logout"), "kimi");
    }
}
