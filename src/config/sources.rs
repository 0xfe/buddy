//! Config-file source discovery and legacy-source diagnostics.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

use super::ConfigDiagnostics;

#[derive(Debug, Clone)]
pub(super) enum ConfigSource {
    Explicit(PathBuf),
    LocalBuddy,
    LocalLegacyAgent,
    GlobalBuddy,
    GlobalLegacyAgent(PathBuf),
    BuiltInDefaults,
}

pub(super) fn read_config_text_with_sources<FRead, FRoot>(
    path_override: Option<&str>,
    read_file: &FRead,
    config_root: &FRoot,
) -> Result<(String, ConfigSource), ConfigError>
where
    FRead: Fn(&Path) -> Result<String, std::io::Error>,
    FRoot: Fn() -> Option<PathBuf>,
{
    if let Some(p) = path_override {
        let path = PathBuf::from(p);
        let text = read_file(&path)?;
        return Ok((text, ConfigSource::Explicit(path)));
    }

    if let Ok(text) = read_file(Path::new("buddy.toml")) {
        return Ok((text, ConfigSource::LocalBuddy));
    }
    if let Ok(text) = read_file(Path::new("agent.toml")) {
        return Ok((text, ConfigSource::LocalLegacyAgent));
    }
    if let Some(dir) = config_root() {
        let buddy_global = dir.join("buddy").join("buddy.toml");
        if let Ok(text) = read_file(&buddy_global) {
            return Ok((text, ConfigSource::GlobalBuddy));
        }
        let legacy_global = dir.join("agent").join("agent.toml");
        if let Ok(text) = read_file(&legacy_global) {
            return Ok((text, ConfigSource::GlobalLegacyAgent(legacy_global)));
        }
    }

    Ok((String::new(), ConfigSource::BuiltInDefaults))
}

pub(super) fn collect_legacy_source_warnings(
    source: &ConfigSource,
    diagnostics: &mut ConfigDiagnostics,
) {
    match source {
        ConfigSource::Explicit(path) => {
            if path.file_name().is_some_and(|name| name == "agent.toml") {
                diagnostics.deprecations.push(format!(
                    "Config file `{}` uses deprecated `agent.toml` naming; rename to `buddy.toml` (legacy support will be removed after v0.4).",
                    path.display()
                ));
            }
        }
        ConfigSource::LocalLegacyAgent => diagnostics.deprecations.push(
            "Using local `./agent.toml`; rename to `./buddy.toml` (legacy support will be removed after v0.4)."
                .to_string(),
        ),
        ConfigSource::GlobalLegacyAgent(path) => diagnostics.deprecations.push(format!(
            "Using deprecated global config `{}`; move it to `~/.config/buddy/buddy.toml` (legacy support will be removed after v0.4).",
            path.display()
        )),
        ConfigSource::LocalBuddy | ConfigSource::GlobalBuddy | ConfigSource::BuiltInDefaults => {}
    }
}
