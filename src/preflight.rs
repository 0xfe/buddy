//! Runtime profile preflight validation.
//!
//! These checks run before startup and model switches to surface common
//! configuration/auth mistakes as actionable errors instead of raw API failures.

use crate::auth::{load_provider_tokens, login_provider_key_for_base_url, supports_openai_login};
use crate::config::{AuthMode, Config, ModelConfig};
use std::net::IpAddr;

/// Validate that the currently active profile can be used for requests.
pub fn validate_active_profile_ready(config: &Config) -> Result<(), String> {
    let base_url = validate_base_url(config)?;
    validate_model_name(config)?;

    let profile = config.models.get(&config.api.profile);
    match config.api.auth {
        AuthMode::ApiKey => validate_api_key_mode(config, profile, &base_url),
        AuthMode::Login => validate_login_mode(config),
    }
}

fn validate_base_url(config: &Config) -> Result<String, String> {
    let trimmed = config.api.base_url.trim();
    if trimmed.is_empty() {
        return Err(
            "No API base URL configured. Set models.<name>.api_base_url in buddy.toml or BUDDY_BASE_URL."
                .to_string(),
        );
    }

    let parsed = reqwest::Url::parse(trimmed)
        .map_err(|err| format!("invalid api_base_url `{trimmed}` for profile `{}`: {err}", config.api.profile))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "invalid api_base_url `{trimmed}` for profile `{}`: unsupported scheme `{other}` (expected http or https)",
                config.api.profile
            ));
        }
    }
    if parsed.host_str().is_none() {
        return Err(format!(
            "invalid api_base_url `{trimmed}` for profile `{}`: missing host",
            config.api.profile
        ));
    }

    Ok(trimmed.to_string())
}

fn validate_model_name(config: &Config) -> Result<(), String> {
    if config.api.model.trim().is_empty() {
        return Err(format!(
            "profile `{}` resolved an empty model name. Set `models.{}.model` (or rename the profile key).",
            config.api.profile, config.api.profile
        ));
    }
    Ok(())
}

fn validate_api_key_mode(
    config: &Config,
    profile: Option<&ModelConfig>,
    base_url: &str,
) -> Result<(), String> {
    if !config.api.api_key.trim().is_empty() {
        return Ok(());
    }

    let Some(profile) = profile else {
        // Should not happen after config resolution, but avoid hard-failing
        // unknown callers that build Config manually.
        return Ok(());
    };

    // When the profile explicitly points to a key source, fail with a targeted
    // hint if that source resolved to empty.
    if let Some(env_name) = profile.api_key_env.as_deref() {
        return Err(format!(
            "profile `{}` expects an API key from env var `{env_name}`, but it is unset or empty",
            config.api.profile
        ));
    }
    if let Some(path) = profile.api_key_file.as_deref() {
        return Err(format!(
            "profile `{}` expects an API key from file `{path}`, but the file resolved to empty contents",
            config.api.profile
        ));
    }
    if !profile.api_key.trim().is_empty() {
        return Ok(());
    }

    // No configured key source. Allow localhost-style endpoints where auth is
    // often intentionally disabled, but otherwise fail early.
    if is_localhost_endpoint(base_url) {
        return Ok(());
    }

    Err(format!(
        "profile `{}` uses auth=api-key but no API key is configured. Set `models.{}.api_key`, `api_key_env`, or `api_key_file`.",
        config.api.profile, config.api.profile
    ))
}

fn validate_login_mode(config: &Config) -> Result<(), String> {
    if !supports_openai_login(&config.api.base_url) {
        return Err(format!(
            "profile `{}` uses `auth = \"login\"`, but base URL `{}` is not an OpenAI login endpoint",
            config.api.profile, config.api.base_url
        ));
    }

    let Some(provider) = login_provider_key_for_base_url(&config.api.base_url) else {
        return Err(format!(
            "profile `{}` uses `auth = \"login\"`, but provider for base URL `{}` is unsupported",
            config.api.profile, config.api.base_url
        ));
    };

    match load_provider_tokens(provider) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!(
            "provider `{}` requires login auth, but no saved login was found. Run `buddy login` (or `/login` inside REPL).",
            provider
        )),
        Err(err) => Err(format!(
            "failed to load login credentials for provider `{}`: {err}",
            provider
        )),
    }
}

fn is_localhost_endpoint(base_url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .ok()
        .is_some_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProtocol, Config};

    #[test]
    fn preflight_rejects_empty_model_name() {
        let mut cfg = Config::default();
        cfg.api.model.clear();
        let err = validate_active_profile_ready(&cfg).expect_err("should fail");
        assert!(err.contains("empty model name"), "err: {err}");
    }

    #[test]
    fn preflight_rejects_non_http_base_url() {
        let mut cfg = Config::default();
        cfg.api.base_url = "file:///tmp".to_string();
        let err = validate_active_profile_ready(&cfg).expect_err("should fail");
        assert!(err.contains("unsupported scheme"), "err: {err}");
    }

    #[test]
    fn preflight_rejects_empty_api_key_env_source() {
        let mut cfg = Config::default();
        cfg.api.profile = "test".to_string();
        cfg.api.base_url = "https://api.example.com/v1".to_string();
        cfg.api.model = "x".to_string();
        cfg.api.protocol = ApiProtocol::Completions;
        cfg.api.auth = AuthMode::ApiKey;
        cfg.api.api_key.clear();
        cfg.models.insert(
            "test".to_string(),
            ModelConfig {
                api_base_url: "https://api.example.com/v1".to_string(),
                api: ApiProtocol::Completions,
                auth: AuthMode::ApiKey,
                api_key: String::new(),
                api_key_env: Some("TEST_KEY".to_string()),
                api_key_file: None,
                model: Some("x".to_string()),
                context_limit: None,
            },
        );
        let err = validate_active_profile_ready(&cfg).expect_err("should fail");
        assert!(err.contains("TEST_KEY"), "err: {err}");
    }

    #[test]
    fn preflight_allows_localhost_without_key_source() {
        let mut cfg = Config::default();
        cfg.api.base_url = "http://localhost:11434/v1".to_string();
        cfg.api.auth = AuthMode::ApiKey;
        cfg.api.api_key.clear();
        let profile = cfg
            .models
            .get_mut(&cfg.api.profile)
            .expect("default profile present");
        profile.api_base_url = cfg.api.base_url.clone();
        profile.api_key.clear();
        profile.api_key_env = None;
        profile.api_key_file = None;
        assert!(validate_active_profile_ready(&cfg).is_ok());
    }
}
