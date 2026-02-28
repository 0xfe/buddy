//! Environment override and legacy env-alias handling.
//!
//! Canonical `BUDDY_*` variables take precedence. Legacy `AGENT_*` aliases are
//! accepted for compatibility and optionally surfaced via diagnostics.

use crate::error::ConfigError;

use super::Config;
use super::ConfigDiagnostics;

pub(super) fn apply_runtime_env_overrides<FEnv>(
    config: &mut Config,
    env_lookup: &FEnv,
) -> Result<(), ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
{
    // Canonical env vars override resolved profile values for immediate CLI use.
    if let Some(url) = env_with_legacy(env_lookup, "BUDDY_BASE_URL", "AGENT_BASE_URL") {
        config.api.base_url = url;
    }
    if let Some(model) = env_with_legacy(env_lookup, "BUDDY_MODEL", "AGENT_MODEL") {
        config.api.model = model;
    }
    if let Some(timeout) = env_with_legacy(
        env_lookup,
        "BUDDY_API_TIMEOUT_SECS",
        "AGENT_API_TIMEOUT_SECS",
    ) {
        // Clamp to at least 1 second to avoid "no-timeout" accidental behavior.
        let parsed = timeout.parse::<u64>().map_err(|_| {
            ConfigError::Invalid(format!(
                "invalid BUDDY_API_TIMEOUT_SECS value `{timeout}`: expected positive integer seconds"
            ))
        })?;
        config.network.api_timeout_secs = parsed.max(1);
    }
    if let Some(timeout) = env_with_legacy(
        env_lookup,
        "BUDDY_FETCH_TIMEOUT_SECS",
        "AGENT_FETCH_TIMEOUT_SECS",
    ) {
        // Clamp to at least 1 second to avoid "no-timeout" accidental behavior.
        let parsed = timeout.parse::<u64>().map_err(|_| {
            ConfigError::Invalid(format!(
                "invalid BUDDY_FETCH_TIMEOUT_SECS value `{timeout}`: expected positive integer seconds"
            ))
        })?;
        config.network.fetch_timeout_secs = parsed.max(1);
    }
    Ok(())
}

/// Resolve a value from canonical env var or, if absent, its legacy alias.
pub(super) fn env_with_legacy<FEnv>(
    env_lookup: &FEnv,
    canonical: &str,
    legacy: &str,
) -> Option<String>
where
    FEnv: Fn(&str) -> Option<String>,
{
    env_lookup(canonical).or_else(|| env_lookup(legacy))
}

/// Return runtime API key override from env vars, including legacy alias.
pub(super) fn api_key_override_with<FEnv>(env_lookup: &FEnv) -> Option<String>
where
    FEnv: Fn(&str) -> Option<String>,
{
    env_with_legacy(env_lookup, "BUDDY_API_KEY", "AGENT_API_KEY")
}

/// Record diagnostics for legacy env alias usage when canonical vars are absent.
pub(super) fn collect_legacy_env_warnings<FEnv>(
    diagnostics: &mut ConfigDiagnostics,
    env_lookup: &FEnv,
) where
    FEnv: Fn(&str) -> Option<String>,
{
    add_legacy_env_warning(
        diagnostics,
        env_lookup,
        "BUDDY_API_KEY",
        "AGENT_API_KEY",
        "Use BUDDY_API_KEY instead (legacy support removed after v0.4).",
    );
    add_legacy_env_warning(
        diagnostics,
        env_lookup,
        "BUDDY_BASE_URL",
        "AGENT_BASE_URL",
        "Use BUDDY_BASE_URL instead (legacy support removed after v0.4).",
    );
    add_legacy_env_warning(
        diagnostics,
        env_lookup,
        "BUDDY_MODEL",
        "AGENT_MODEL",
        "Use BUDDY_MODEL instead (legacy support removed after v0.4).",
    );
    add_legacy_env_warning(
        diagnostics,
        env_lookup,
        "BUDDY_API_TIMEOUT_SECS",
        "AGENT_API_TIMEOUT_SECS",
        "Use BUDDY_API_TIMEOUT_SECS instead (legacy support removed after v0.4).",
    );
    add_legacy_env_warning(
        diagnostics,
        env_lookup,
        "BUDDY_FETCH_TIMEOUT_SECS",
        "AGENT_FETCH_TIMEOUT_SECS",
        "Use BUDDY_FETCH_TIMEOUT_SECS instead (legacy support removed after v0.4).",
    );
}

/// Append one legacy-alias warning if only the legacy key is present.
fn add_legacy_env_warning<FEnv>(
    diagnostics: &mut ConfigDiagnostics,
    env_lookup: &FEnv,
    canonical: &str,
    legacy: &str,
    guidance: &str,
) where
    FEnv: Fn(&str) -> Option<String>,
{
    if env_lookup(canonical).is_none() && env_lookup(legacy).is_some() {
        diagnostics.deprecations.push(format!(
            "Detected deprecated env var `{legacy}`. {guidance}"
        ));
    }
}

/// Sort and deduplicate diagnostic strings for stable output.
pub(super) fn dedupe_diagnostics(diagnostics: &mut ConfigDiagnostics) {
    diagnostics.deprecations.sort();
    diagnostics.deprecations.dedup();
}
