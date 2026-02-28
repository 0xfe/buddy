//! Top-level config loading pipeline.
//!
//! This module wires together source discovery, TOML parsing, model-profile
//! resolution, env overrides, and compatibility diagnostics.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

use super::env::{
    api_key_override_with, apply_runtime_env_overrides, collect_legacy_env_warnings,
    dedupe_diagnostics,
};
use super::init::config_root_dir;
use super::resolve::resolve_config_from_file_config;
use super::sources::{collect_legacy_source_warnings, read_config_text_with_sources};
use super::{Config, FileConfig, LoadedConfig};

/// Load configuration from disk and environment.
///
/// `path_override` is an explicit config file path (from --config flag).
pub fn load_config(path_override: Option<&str>) -> Result<Config, ConfigError> {
    Ok(load_config_with_diagnostics(path_override)?.config)
}

/// Load configuration and return compatibility diagnostics.
pub fn load_config_with_diagnostics(
    path_override: Option<&str>,
) -> Result<LoadedConfig, ConfigError> {
    // Production wiring: real filesystem, real environment, real config root.
    load_config_with_diagnostics_from_sources(
        path_override,
        |path| std::fs::read_to_string(path),
        |name| std::env::var(name).ok(),
        config_root_dir,
    )
}

pub(super) fn load_config_with_diagnostics_from_sources<FRead, FEnv, FRoot>(
    path_override: Option<&str>,
    read_file: FRead,
    env_lookup: FEnv,
    config_root: FRoot,
) -> Result<LoadedConfig, ConfigError>
where
    FRead: Fn(&Path) -> Result<String, std::io::Error>,
    FEnv: Fn(&str) -> Option<String>,
    FRoot: Fn() -> Option<PathBuf>,
{
    // 1) Read config text from the highest-precedence source.
    let (config_text, source) =
        read_config_text_with_sources(path_override, &read_file, &config_root)?;
    let mut diagnostics = super::ConfigDiagnostics::default();
    // 2) Capture source-level compatibility warnings (legacy file names/paths).
    collect_legacy_source_warnings(&source, &mut diagnostics);
    // 3) Parse TOML into intermediate file-configuration representation.
    let parsed: FileConfig = toml::from_str(&config_text)?;
    // 4) Resolve profile defaults and API key sources into runtime config.
    let mut config = resolve_config_from_file_config(
        parsed,
        api_key_override_with(&env_lookup),
        &env_lookup,
        |path| {
            read_file(Path::new(path)).map_err(|e| {
                ConfigError::Invalid(format!(
                    "failed to read model profile api_key_file `{path}`: {e}"
                ))
            })
        },
        &mut diagnostics,
    )?;
    // 5) Apply direct runtime env overrides (base URL, model, timeouts, etc.).
    apply_runtime_env_overrides(&mut config, &env_lookup)?;
    // 6) Attach env compatibility diagnostics and normalize message ordering.
    collect_legacy_env_warnings(&mut diagnostics, &env_lookup);
    dedupe_diagnostics(&mut diagnostics);

    Ok(LoadedConfig {
        config,
        diagnostics,
    })
}
