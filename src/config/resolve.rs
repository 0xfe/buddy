//! Config-file to runtime-config resolution.

use std::collections::BTreeMap;

use crate::error::ConfigError;

use super::defaults::{
    default_models_map, DEFAULT_AGENT_NAME, DEFAULT_API_BASE_URL, DEFAULT_MODEL_PROFILE_NAME,
};
use super::{ApiConfig, Config, ConfigDiagnostics, FileConfig, ModelConfig};

pub(super) fn resolve_config_from_file_config<FEnv, FRead>(
    mut parsed: FileConfig,
    key_override: Option<String>,
    env_lookup: FEnv,
    read_file: FRead,
    diagnostics: &mut ConfigDiagnostics,
) -> Result<Config, ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
    FRead: Fn(&str) -> Result<String, ConfigError>,
{
    if parsed.models.is_empty() {
        if let Some(legacy_api) = parsed.api.take() {
            diagnostics.deprecations.push(
                "Config uses deprecated `[api]`; migrate to `[models.<name>]` + `agent.model` (legacy support will be removed after v0.4)."
                    .to_string(),
            );
            parsed.models.insert(
                DEFAULT_MODEL_PROFILE_NAME.to_string(),
                legacy_api.into_model_config(),
            );
            if normalized_string(&parsed.agent.model).is_none() {
                parsed.agent.model = DEFAULT_MODEL_PROFILE_NAME.to_string();
            }
        } else {
            parsed.models = default_models_map();
        }
    }

    if normalized_string(&parsed.agent.model).is_none() {
        parsed.agent.model = parsed
            .models
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| DEFAULT_MODEL_PROFILE_NAME.to_string());
    }
    if normalized_string(&parsed.agent.name).is_none() {
        parsed.agent.name = DEFAULT_AGENT_NAME.to_string();
    } else if let Some(name) = normalized_string(&parsed.agent.name) {
        parsed.agent.name = name;
    }

    let mut config = Config {
        api: ApiConfig::default(),
        models: parsed.models,
        agent: parsed.agent,
        tools: parsed.tools,
        network: parsed.network,
        display: parsed.display,
    };

    config.api = resolve_active_api_with(
        &config.models,
        &config.agent.model,
        key_override,
        env_lookup,
        read_file,
    )?;

    Ok(config)
}

pub(super) fn resolve_active_api_with<FEnv, FRead>(
    models: &BTreeMap<String, ModelConfig>,
    selected_profile: &str,
    key_override: Option<String>,
    env_lookup: FEnv,
    read_file: FRead,
) -> Result<ApiConfig, ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
    FRead: Fn(&str) -> Result<String, ConfigError>,
{
    let profile_name = selected_profile.trim();
    let Some(profile) = models.get(profile_name) else {
        return Err(ConfigError::Invalid(format!(
            "agent.model `{profile_name}` not found in `[models.<name>]`"
        )));
    };

    let path_prefix = format!("models.{profile_name}");
    let api_key = resolve_api_key(profile, key_override, env_lookup, read_file, &path_prefix)?;
    let base_url = normalized_string(&profile.api_base_url)
        .unwrap_or_else(|| DEFAULT_API_BASE_URL.to_string());

    Ok(ApiConfig {
        base_url,
        api_key,
        model: profile.resolved_model_name(profile_name),
        protocol: profile.api,
        auth: profile.auth,
        profile: profile_name.to_string(),
        context_limit: profile.context_limit,
    })
}

pub(super) fn resolve_api_key<FEnv, FRead>(
    model: &ModelConfig,
    key_override: Option<String>,
    env_lookup: FEnv,
    read_file: FRead,
    path_prefix: &str,
) -> Result<String, ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
    FRead: Fn(&str) -> Result<String, ConfigError>,
{
    validate_api_key_sources(model, path_prefix)?;

    if let Some(key) = key_override {
        return Ok(key.trim().to_string());
    }

    if let Some(env_name) = normalized_option(&model.api_key_env) {
        return Ok(env_lookup(&env_name).unwrap_or_default().trim().to_string());
    }

    if let Some(path) = normalized_option(&model.api_key_file) {
        return Ok(read_file(&path)?.trim_end().to_string());
    }

    Ok(model.api_key.trim().to_string())
}

fn validate_api_key_sources(model: &ModelConfig, path_prefix: &str) -> Result<(), ConfigError> {
    let mut configured = Vec::new();
    if normalized_string(&model.api_key).is_some() {
        configured.push("api_key");
    }
    if normalized_option(&model.api_key_env).is_some() {
        configured.push("api_key_env");
    }
    if normalized_option(&model.api_key_file).is_some() {
        configured.push("api_key_file");
    }
    if configured.len() > 1 {
        return Err(ConfigError::Invalid(format!(
            "only one of {path_prefix}.api_key, {path_prefix}.api_key_env, and {path_prefix}.api_key_file may be set (found: {})",
            configured.join(", ")
        )));
    }
    Ok(())
}

pub(super) fn normalized_option(value: &Option<String>) -> Option<String> {
    value.as_deref().and_then(normalized_string)
}

pub(super) fn normalized_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
