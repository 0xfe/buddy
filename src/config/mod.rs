//! Configuration loading from TOML files and environment variables.
//!
//! Config is loaded in this order of precedence (highest wins):
//! 1. Environment variables (`BUDDY_API_KEY`, `BUDDY_BASE_URL`, `BUDDY_MODEL`)
//!    with legacy `AGENT_*` fallback.
//! 2. TOML file specified via --config CLI flag
//! 3. ./buddy.toml in the current directory (legacy ./agent.toml fallback)
//! 4. $XDG_CONFIG_HOME/buddy/buddy.toml (or ~/.config/buddy/buddy.toml;
//!    legacy ~/.config/agent/agent.toml fallback)
//! 5. Built-in defaults

use crate::error::ConfigError;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod defaults;
mod types;

use defaults::{
    default_models_map, DEFAULT_AGENT_NAME, DEFAULT_API_BASE_URL, DEFAULT_BUDDY_CONFIG_TEMPLATE,
    DEFAULT_MODEL_PROFILE_NAME,
};
#[cfg(test)]
use defaults::{DEFAULT_API_TIMEOUT_SECS, DEFAULT_FETCH_TIMEOUT_SECS};
pub use types::{
    AgentConfig, ApiConfig, ApiProtocol, AuthMode, Config, ConfigDiagnostics, DisplayConfig,
    GlobalConfigInitResult, LoadedConfig, ModelConfig, NetworkConfig, ToolsConfig,
};
use types::FileConfig;

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum ConfigSource {
    Explicit(PathBuf),
    LocalBuddy,
    LocalLegacyAgent,
    GlobalBuddy,
    GlobalLegacyAgent(PathBuf),
    BuiltInDefaults,
}

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
    load_config_with_diagnostics_from_sources(
        path_override,
        |path| std::fs::read_to_string(path),
        |name| std::env::var(name).ok(),
        config_root_dir,
    )
}

fn load_config_with_diagnostics_from_sources<FRead, FEnv, FRoot>(
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
    let (config_text, source) =
        read_config_text_with_sources(path_override, &read_file, &config_root)?;
    let mut diagnostics = ConfigDiagnostics::default();
    collect_legacy_source_warnings(&source, &mut diagnostics);
    let parsed: FileConfig = toml::from_str(&config_text)?;
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
    apply_runtime_env_overrides(&mut config, &env_lookup)?;
    collect_legacy_env_warnings(&mut diagnostics, &env_lookup);
    dedupe_diagnostics(&mut diagnostics);

    Ok(LoadedConfig {
        config,
        diagnostics,
    })
}

fn read_config_text_with_sources<FRead, FRoot>(
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

fn collect_legacy_source_warnings(source: &ConfigSource, diagnostics: &mut ConfigDiagnostics) {
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
        ConfigSource::LocalBuddy
        | ConfigSource::GlobalBuddy
        | ConfigSource::BuiltInDefaults => {}
    }
}

fn resolve_config_from_file_config<FEnv, FRead>(
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

fn apply_runtime_env_overrides<FEnv>(
    config: &mut Config,
    env_lookup: &FEnv,
) -> Result<(), ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
{
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
        let parsed = timeout.parse::<u64>().map_err(|_| {
            ConfigError::Invalid(format!(
                "invalid BUDDY_FETCH_TIMEOUT_SECS value `{timeout}`: expected positive integer seconds"
            ))
        })?;
        config.network.fetch_timeout_secs = parsed.max(1);
    }
    Ok(())
}

fn env_with_legacy<FEnv>(env_lookup: &FEnv, canonical: &str, legacy: &str) -> Option<String>
where
    FEnv: Fn(&str) -> Option<String>,
{
    env_lookup(canonical).or_else(|| env_lookup(legacy))
}

fn api_key_override_with<FEnv>(env_lookup: &FEnv) -> Option<String>
where
    FEnv: Fn(&str) -> Option<String>,
{
    env_with_legacy(env_lookup, "BUDDY_API_KEY", "AGENT_API_KEY")
}

fn collect_legacy_env_warnings<FEnv>(diagnostics: &mut ConfigDiagnostics, env_lookup: &FEnv)
where
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

fn dedupe_diagnostics(diagnostics: &mut ConfigDiagnostics) {
    diagnostics.deprecations.sort();
    diagnostics.deprecations.dedup();
}

/// Return the default per-user config path (`~/.config/buddy/buddy.toml`).
pub fn default_global_config_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("buddy.toml"))
}

/// Return the default REPL history path (`~/.config/buddy/history`).
pub fn default_history_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("history"))
}

/// Initialize `~/.config/buddy/buddy.toml`.
///
/// - Without `force`, returns `AlreadyInitialized` if the file exists.
/// - With `force`, backs up the existing file in the same directory using a
///   timestamped name, then rewrites it from the compiled template.
pub fn initialize_default_global_config(
    force: bool,
) -> Result<GlobalConfigInitResult, ConfigError> {
    let path = default_global_config_path().ok_or_else(|| {
        ConfigError::Invalid(
            "unable to resolve default config path for ~/.config/buddy/buddy.toml".to_string(),
        )
    })?;
    initialize_default_global_config_at_path(&path, force)
}

/// Ensure the default global config file exists.
///
/// Returns the global config path when available on this platform.
pub fn ensure_default_global_config() -> Result<Option<PathBuf>, ConfigError> {
    let Some(path) = default_global_config_path() else {
        return Ok(None);
    };
    ensure_default_global_config_at_path(&path)?;
    Ok(Some(path))
}

/// Switch the active profile to a configured `[models.<name>]` entry.
pub fn select_model_profile(config: &mut Config, profile_name: &str) -> Result<(), ConfigError> {
    let selected = profile_name.trim();
    if selected.is_empty() {
        return Err(ConfigError::Invalid(
            "model profile name must not be empty".to_string(),
        ));
    }

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

fn ensure_default_global_config_at_path(path: &Path) -> Result<(), ConfigError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // create_new avoids clobbering an existing file if another process won the race.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(DEFAULT_BUDDY_CONFIG_TEMPLATE.as_bytes())?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

fn initialize_default_global_config_at_path(
    path: &Path,
    force: bool,
) -> Result<GlobalConfigInitResult, ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if path.exists() {
        if !force {
            return Ok(GlobalConfigInitResult::AlreadyInitialized {
                path: path.to_path_buf(),
            });
        }
        let backup_path = timestamped_backup_path(path);
        std::fs::copy(path, &backup_path)?;
        std::fs::write(path, DEFAULT_BUDDY_CONFIG_TEMPLATE)?;
        return Ok(GlobalConfigInitResult::Overwritten {
            path: path.to_path_buf(),
            backup_path,
        });
    }

    // create_new avoids clobbering if another process wins a race to create.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(DEFAULT_BUDDY_CONFIG_TEMPLATE.as_bytes())?;
            Ok(GlobalConfigInitResult::Created {
                path: path.to_path_buf(),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(GlobalConfigInitResult::AlreadyInitialized {
                path: path.to_path_buf(),
            })
        }
        Err(e) => Err(ConfigError::Io(e)),
    }
}

fn timestamped_backup_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "buddy.toml".to_string());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for suffix in 0..1000usize {
        let candidate_name = if suffix == 0 {
            format!("{file_name}.{timestamp}.bak")
        } else {
            format!("{file_name}.{timestamp}.{suffix}.bak")
        };
        let candidate = path.with_file_name(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }

    path.with_file_name(format!(
        "{file_name}.{timestamp}.{}.bak",
        std::process::id()
    ))
}

fn api_key_override_env() -> Option<String> {
    api_key_override_with(&|name| std::env::var(name).ok())
}

fn resolve_active_api_with<FEnv, FRead>(
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

fn resolve_api_key<FEnv, FRead>(
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

fn normalized_option(value: &Option<String>) -> Option<String> {
    value.as_deref().and_then(normalized_string)
}

fn normalized_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn config_root_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir()
        .map(|home| home.join(".config"))
        .or_else(dirs::config_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn defaults_are_sensible() {
        let c = Config::default();
        assert_eq!(c.agent.name, "agent-mo");
        assert_eq!(c.agent.model, "gpt-codex");
        assert_eq!(c.api.base_url, "https://api.openai.com/v1");
        assert_eq!(c.api.model, "gpt-5.3-codex");
        assert_eq!(c.api.protocol, ApiProtocol::Responses);
        assert_eq!(c.api.auth, AuthMode::Login);
        assert_eq!(c.agent.max_iterations, 20);
        assert!(c.tools.shell_enabled);
        assert!(c.display.color);
        assert!(c.display.persist_history);
        assert!(c.models.contains_key("gpt-codex"));
        assert!(c.models.contains_key("gpt-spark"));
        assert!(c.models.contains_key("openrouter-deepseek"));
        assert!(c.models.contains_key("openrouter-glm"));
        assert!(c.models.contains_key("kimi"));
        assert!(!c.tools.shell_denylist.is_empty());
        assert_eq!(c.network.api_timeout_secs, DEFAULT_API_TIMEOUT_SECS);
        assert_eq!(c.network.fetch_timeout_secs, DEFAULT_FETCH_TIMEOUT_SECS);
    }

    #[test]
    fn parse_partial_toml() {
        let toml = r#"
            [models.kimi]
            api_base_url = "https://api.moonshot.ai/v1"
            api_key_env = "MOONSHOT_API_KEY"

            [agent]
            model = "kimi"

            [tools]
            shell_confirm = false
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.agent.name, "agent-mo");
        assert_eq!(c.agent.model, "kimi");
        assert_eq!(c.api.model, "kimi");
        assert_eq!(c.api.base_url, "https://api.moonshot.ai/v1");
        assert!(!c.tools.shell_confirm);
        assert!(c.display.color);
        assert!(c.display.persist_history);
    }

    #[test]
    fn parse_fetch_security_policy() {
        let toml = r#"
            [tools]
            fetch_confirm = true
            fetch_allowed_domains = ["example.com", "api.example.com"]
            fetch_blocked_domains = ["internal.example.com", "localhost"]
            files_allowed_paths = ["/workspace", "/tmp/project"]
            shell_denylist = ["rm -rf /", "mkfs"]
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert!(c.tools.fetch_confirm);
        assert_eq!(
            c.tools.fetch_allowed_domains,
            vec!["example.com", "api.example.com"]
        );
        assert_eq!(
            c.tools.fetch_blocked_domains,
            vec!["internal.example.com", "localhost"]
        );
        assert_eq!(
            c.tools.files_allowed_paths,
            vec!["/workspace", "/tmp/project"]
        );
        assert_eq!(c.tools.shell_denylist, vec!["rm -rf /", "mkfs"]);
    }

    #[test]
    fn blank_agent_name_falls_back_to_default() {
        let toml = r#"
            [models.local]
            api_base_url = "https://api.example.com/v1"
            api_key = "k"

            [agent]
            name = "   "
            model = "local"
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.agent.name, "agent-mo");
    }

    #[test]
    fn parse_network_timeouts() {
        let toml = r#"
            [network]
            api_timeout_secs = 45
            fetch_timeout_secs = 12
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.network.api_timeout_secs, 45);
        assert_eq!(c.network.fetch_timeout_secs, 12);
    }

    #[test]
    fn parse_model_alias_table() {
        let toml = r#"
            [model.kimi]
            api_base_url = "https://api.moonshot.ai/v1"

            [agent]
            model = "kimi"
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.agent.model, "kimi");
        assert_eq!(c.api.model, "kimi");
        assert_eq!(c.api.base_url, "https://api.moonshot.ai/v1");
    }

    #[test]
    fn parse_model_api_and_auth_modes() {
        let toml = r#"
            [models.gpt-codex]
            api_base_url = "https://api.openai.com/v1"
            api = "responses"
            auth = "login"
            model = "gpt-5.3-codex"

            [agent]
            model = "gpt-codex"
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.api.protocol, ApiProtocol::Responses);
        assert_eq!(c.api.auth, AuthMode::Login);
    }

    #[test]
    fn missing_agent_model_defaults_to_first_profile() {
        let toml = r#"
            [models.kimi]
            api_base_url = "https://api.moonshot.ai/v1"
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.agent.model, "kimi");
        assert_eq!(c.api.model, "kimi");
    }

    #[test]
    fn parse_empty_string() {
        let c = parse_file_config_for_test("").unwrap();
        assert_eq!(c.agent.model, "gpt-codex");
        assert_eq!(c.api.model, "gpt-5.3-codex");
    }

    #[test]
    fn api_key_sources_are_mutually_exclusive() {
        let model = ModelConfig {
            api_key: "literal".into(),
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..ModelConfig::default()
        };
        let err = resolve_api_key(
            &model,
            None,
            |_| None,
            |_| Ok(String::new()),
            "models.gpt-codex",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("only one of models.gpt-codex.api_key"));
    }

    #[test]
    fn api_key_env_source_is_resolved() {
        let model = ModelConfig {
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..ModelConfig::default()
        };

        let resolved = resolve_api_key(
            &model,
            None,
            |name| (name == "OPENAI_API_KEY").then(|| "env-secret".into()),
            |_| Ok(String::new()),
            "models.gpt-codex",
        )
        .unwrap();

        assert_eq!(resolved, "env-secret");
    }

    #[test]
    fn missing_api_key_env_source_defaults_to_empty() {
        let model = ModelConfig {
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..ModelConfig::default()
        };

        let resolved = resolve_api_key(
            &model,
            None,
            |_| None,
            |_| Ok(String::new()),
            "models.gpt-codex",
        )
        .unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn api_key_file_source_is_trimmed() {
        let model = ModelConfig {
            api_key_file: Some("/tmp/key.txt".into()),
            ..ModelConfig::default()
        };

        let resolved = resolve_api_key(
            &model,
            None,
            |_| None,
            |path| {
                assert_eq!(path, "/tmp/key.txt");
                Ok("file-secret\n".into())
            },
            "models.gpt-codex",
        )
        .unwrap();

        assert_eq!(resolved, "file-secret");
    }

    #[test]
    fn explicit_api_key_env_override_wins() {
        let model = ModelConfig {
            api_key_file: Some("/tmp/key.txt".into()),
            ..ModelConfig::default()
        };

        let resolved = resolve_api_key(
            &model,
            Some("override".into()),
            |_| None,
            |_| Ok("ignored".into()),
            "models.gpt-codex",
        )
        .unwrap();

        assert_eq!(resolved, "override");
    }

    #[test]
    fn select_model_profile_updates_active_api() {
        let mut config = parse_file_config_for_test(
            r#"
            [models.gpt-codex]
            model = "gpt-5.3-codex"

            [models.kimi]
            api_base_url = "https://api.moonshot.ai/v1"
            model = "moonshot-v1"

            [agent]
            model = "gpt-codex"
        "#,
        )
        .unwrap();

        select_model_profile(&mut config, "kimi").unwrap();

        assert_eq!(config.agent.model, "kimi");
        assert_eq!(config.api.base_url, "https://api.moonshot.ai/v1");
        assert_eq!(config.api.model, "moonshot-v1");
    }

    #[test]
    fn ensure_default_global_config_writes_template() {
        let tmp_root = std::env::temp_dir().join(format!(
            "buddy-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = tmp_root.join("buddy").join("buddy.toml");

        ensure_default_global_config_at_path(&path).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, DEFAULT_BUDDY_CONFIG_TEMPLATE);

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&tmp_root).unwrap();
    }

    #[test]
    fn initialize_global_config_returns_already_initialized_without_force() {
        let tmp_root = std::env::temp_dir().join(format!(
            "buddy-config-init-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = tmp_root.join("buddy").join("buddy.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "old-config").unwrap();

        let outcome = initialize_default_global_config_at_path(&path, false).unwrap();
        assert!(matches!(
            outcome,
            GlobalConfigInitResult::AlreadyInitialized { path: ref p } if p == &path
        ));
        let current = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current, "old-config");

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&tmp_root).unwrap();
    }

    #[test]
    fn initialize_global_config_force_overwrites_and_creates_backup() {
        let tmp_root = std::env::temp_dir().join(format!(
            "buddy-config-force-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = tmp_root.join("buddy").join("buddy.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "old-config").unwrap();

        let outcome = initialize_default_global_config_at_path(&path, true).unwrap();
        let backup_path = match outcome {
            GlobalConfigInitResult::Overwritten {
                path: returned_path,
                backup_path,
            } => {
                assert_eq!(returned_path, path);
                backup_path
            }
            other => panic!("unexpected outcome: {other:?}"),
        };

        let current = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current, DEFAULT_BUDDY_CONFIG_TEMPLATE);
        let backup = std::fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup, "old-config");
        assert_eq!(backup_path.parent(), path.parent());
        assert!(backup_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.contains(".bak")));

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&backup_path).unwrap();
        std::fs::remove_dir_all(&tmp_root).unwrap();
    }

    #[test]
    fn diagnostics_warn_for_explicit_legacy_agent_toml_path() {
        let tmp_root = std::env::temp_dir().join(format!(
            "buddy-config-diag-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_root).unwrap();
        let path = tmp_root.join("agent.toml");
        std::fs::write(
            &path,
            r#"
            [models.gpt-codex]
            model = "gpt-5.3-codex"

            [agent]
            model = "gpt-codex"
            "#,
        )
        .unwrap();

        let loaded = load_config_with_diagnostics(Some(path.to_string_lossy().as_ref())).unwrap();
        assert!(loaded
            .diagnostics
            .deprecations
            .iter()
            .any(|msg| msg.contains("agent.toml")));

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&tmp_root).unwrap();
    }

    #[test]
    fn diagnostics_warn_when_legacy_api_section_is_used() {
        let tmp_root = std::env::temp_dir().join(format!(
            "buddy-config-diag-api-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_root).unwrap();
        let path = tmp_root.join("buddy.toml");
        std::fs::write(
            &path,
            r#"
            [api]
            base_url = "https://api.openai.com/v1"
            model = "gpt-5.3-codex"

            [agent]
            model = "gpt-codex"
            "#,
        )
        .unwrap();

        let loaded = load_config_with_diagnostics(Some(path.to_string_lossy().as_ref())).unwrap();
        assert!(loaded
            .diagnostics
            .deprecations
            .iter()
            .any(|msg| msg.contains("deprecated `[api]`")));

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&tmp_root).unwrap();
    }

    #[test]
    fn injected_sources_prefer_local_buddy_toml_over_global() {
        let mut files = BTreeMap::<String, String>::new();
        files.insert(
            "buddy.toml".to_string(),
            r#"
            [models.local]
            model = "local-model"
            api_base_url = "https://local.example/v1"

            [agent]
            model = "local"
            "#
            .to_string(),
        );
        files.insert(
            "/cfg/buddy/buddy.toml".to_string(),
            r#"
            [models.global]
            model = "global-model"

            [agent]
            model = "global"
            "#
            .to_string(),
        );

        let loaded = load_config_with_sources_for_test(
            None,
            files,
            BTreeMap::new(),
            Some(PathBuf::from("/cfg")),
        )
        .unwrap();

        assert_eq!(loaded.config.agent.model, "local");
        assert_eq!(loaded.config.api.model, "local-model");
        assert_eq!(loaded.config.api.base_url, "https://local.example/v1");
    }

    #[test]
    fn injected_sources_apply_env_overrides() {
        let mut files = BTreeMap::<String, String>::new();
        files.insert(
            "buddy.toml".to_string(),
            r#"
            [models.gpt-codex]
            api_base_url = "https://api.openai.com/v1"
            model = "gpt-5.3-codex"

            [agent]
            model = "gpt-codex"
            "#
            .to_string(),
        );

        let mut env = BTreeMap::<String, String>::new();
        env.insert(
            "BUDDY_BASE_URL".to_string(),
            "https://override.example/v1".to_string(),
        );
        env.insert("BUDDY_MODEL".to_string(), "override-model".to_string());
        env.insert("BUDDY_API_TIMEOUT_SECS".to_string(), "9".to_string());
        env.insert("BUDDY_FETCH_TIMEOUT_SECS".to_string(), "3".to_string());

        let loaded =
            load_config_with_sources_for_test(None, files, env, Some(PathBuf::from("/cfg")))
                .unwrap();

        assert_eq!(loaded.config.api.base_url, "https://override.example/v1");
        assert_eq!(loaded.config.api.model, "override-model");
        assert_eq!(loaded.config.network.api_timeout_secs, 9);
        assert_eq!(loaded.config.network.fetch_timeout_secs, 3);
    }

    #[test]
    fn injected_sources_warn_for_legacy_env_aliases() {
        let mut files = BTreeMap::<String, String>::new();
        files.insert(
            "buddy.toml".to_string(),
            r#"
            [models.gpt-codex]
            model = "gpt-5.3-codex"

            [agent]
            model = "gpt-codex"
            "#
            .to_string(),
        );
        let mut env = BTreeMap::<String, String>::new();
        env.insert("AGENT_MODEL".to_string(), "legacy-model".to_string());

        let loaded =
            load_config_with_sources_for_test(None, files, env, Some(PathBuf::from("/cfg")))
                .unwrap();

        assert_eq!(loaded.config.api.model, "legacy-model");
        assert!(loaded
            .diagnostics
            .deprecations
            .iter()
            .any(|msg| msg.contains("AGENT_MODEL")));
    }

    fn load_config_with_sources_for_test(
        path_override: Option<&str>,
        files: BTreeMap<String, String>,
        env: BTreeMap<String, String>,
        config_root: Option<PathBuf>,
    ) -> Result<LoadedConfig, ConfigError> {
        load_config_with_diagnostics_from_sources(
            path_override,
            move |path| {
                let key = path.to_string_lossy().into_owned();
                files
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, key))
            },
            move |name| env.get(name).cloned(),
            move || config_root.clone(),
        )
    }

    fn parse_file_config_for_test(toml_text: &str) -> Result<Config, ConfigError> {
        let parsed: FileConfig = toml::from_str(toml_text)?;
        let mut diagnostics = ConfigDiagnostics::default();
        resolve_config_from_file_config(
            parsed,
            None,
            |_| None,
            |_| Ok(String::new()),
            &mut diagnostics,
        )
    }
}
