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
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_BUDDY_CONFIG_TEMPLATE: &str = include_str!("templates/buddy.toml");

// ---------------------------------------------------------------------------
// Config structs
// ---------------------------------------------------------------------------

/// Top-level configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub api: ApiConfig,
    pub agent: AgentConfig,
    pub tools: ToolsConfig,
    pub display: DisplayConfig,
}

/// API connection settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub base_url: String,
    pub api_key: String,
    pub api_key_env: Option<String>,
    pub api_key_file: Option<String>,
    pub model: String,
    /// Override for context window size. Auto-detected from model name if omitted.
    pub context_limit: Option<usize>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: "gpt-5.2-codex".into(),
            context_limit: None,
        }
    }
}

/// Agent behavior settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub system_prompt: String,
    /// Safety cap on agentic loop iterations.
    pub max_iterations: usize,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // Empty means "no additional operator instructions"; the built-in
            // system prompt template is rendered at runtime in `main.rs`.
            system_prompt: String::new(),
            max_iterations: 20,
            temperature: None,
            top_p: None,
        }
    }
}

/// Tool availability settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub shell_enabled: bool,
    pub fetch_enabled: bool,
    pub files_enabled: bool,
    pub search_enabled: bool,
    /// Whether to prompt the user before running shell commands.
    pub shell_confirm: bool,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            shell_enabled: true,
            fetch_enabled: true,
            files_enabled: true,
            search_enabled: true,
            shell_confirm: true,
        }
    }
}

/// Display / rendering preferences.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub color: bool,
    pub show_tokens: bool,
    pub show_tool_calls: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            color: true,
            show_tokens: false,
            show_tool_calls: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load configuration from disk and environment.
///
/// `path_override` is an explicit config file path (from --config flag).
pub fn load_config(path_override: Option<&str>) -> Result<Config, ConfigError> {
    let config_text = if let Some(p) = path_override {
        // Explicit path — fail if it doesn't exist.
        std::fs::read_to_string(p)?
    } else if let Ok(text) = std::fs::read_to_string("buddy.toml") {
        text
    } else if let Ok(text) = std::fs::read_to_string("agent.toml") {
        text
    } else if let Some(dir) = config_root_dir() {
        let buddy_global = dir.join("buddy").join("buddy.toml");
        if let Ok(text) = std::fs::read_to_string(&buddy_global) {
            text
        } else {
            let legacy_global = dir.join("agent").join("agent.toml");
            std::fs::read_to_string(legacy_global).unwrap_or_default()
        }
    } else {
        String::new()
    };

    let mut config: Config = toml::from_str(&config_text)?;

    // API key resolution order:
    // 1) BUDDY_API_KEY / AGENT_API_KEY env override
    // 2) api.api_key_env (named env var)
    // 3) api.api_key_file (file contents)
    // 4) api.api_key literal
    let api_key_override = std::env::var("BUDDY_API_KEY")
        .or_else(|_| std::env::var("AGENT_API_KEY"))
        .ok();
    resolve_api_key(
        &mut config.api,
        api_key_override,
        |name| std::env::var(name).ok(),
        |path| {
            std::fs::read_to_string(path).map_err(|e| {
                ConfigError::Invalid(format!("failed to read api.api_key_file `{path}`: {e}"))
            })
        },
    )?;

    // Environment variable overrides.
    if let Ok(url) = std::env::var("BUDDY_BASE_URL").or_else(|_| std::env::var("AGENT_BASE_URL")) {
        config.api.base_url = url;
    }
    if let Ok(model) = std::env::var("BUDDY_MODEL").or_else(|_| std::env::var("AGENT_MODEL")) {
        config.api.model = model;
    }

    Ok(config)
}

/// Return the default per-user config path (`~/.config/buddy/buddy.toml`).
pub fn default_global_config_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("buddy.toml"))
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

fn resolve_api_key<FEnv, FRead>(
    api: &mut ApiConfig,
    key_override: Option<String>,
    env_lookup: FEnv,
    read_file: FRead,
) -> Result<(), ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
    FRead: Fn(&str) -> Result<String, ConfigError>,
{
    validate_api_key_sources(api)?;

    if let Some(key) = key_override {
        api.api_key = key;
        return Ok(());
    }

    if let Some(env_name) = normalized_option(&api.api_key_env) {
        api.api_key = env_lookup(&env_name).unwrap_or_default();
        return Ok(());
    }

    if let Some(path) = normalized_option(&api.api_key_file) {
        api.api_key = read_file(&path)?.trim_end().to_string();
        return Ok(());
    }

    api.api_key = api.api_key.trim().to_string();
    Ok(())
}

fn validate_api_key_sources(api: &ApiConfig) -> Result<(), ConfigError> {
    let mut configured = Vec::new();
    if normalized_string(&api.api_key).is_some() {
        configured.push("api_key");
    }
    if normalized_option(&api.api_key_env).is_some() {
        configured.push("api_key_env");
    }
    if normalized_option(&api.api_key_file).is_some() {
        configured.push("api_key_file");
    }
    if configured.len() > 1 {
        return Err(ConfigError::Invalid(format!(
            "only one of api.api_key, api.api_key_env, and api.api_key_file may be set (found: {})",
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

fn config_root_dir() -> Option<PathBuf> {
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

    #[test]
    fn defaults_are_sensible() {
        let c = Config::default();
        assert_eq!(c.api.base_url, "https://api.openai.com/v1");
        assert_eq!(c.api.model, "gpt-5.2-codex");
        assert_eq!(c.agent.max_iterations, 20);
        assert!(c.tools.shell_enabled);
        assert!(c.display.color);
    }

    #[test]
    fn parse_partial_toml() {
        let toml = r#"
            [api]
            model = "llama3"
            [tools]
            shell_confirm = false
        "#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.api.model, "llama3");
        assert!(!c.tools.shell_confirm);
        // Unset fields keep defaults.
        assert_eq!(c.api.base_url, "https://api.openai.com/v1");
        assert!(c.display.color);
    }

    #[test]
    fn parse_empty_string() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.api.model, "gpt-5.2-codex");
    }

    #[test]
    fn api_key_sources_are_mutually_exclusive() {
        let mut api = ApiConfig {
            api_key: "literal".into(),
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..ApiConfig::default()
        };
        let err = resolve_api_key(&mut api, None, |_| None, |_| Ok(String::new())).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("only one of api.api_key"));
    }

    #[test]
    fn api_key_env_source_is_resolved() {
        let mut api = ApiConfig {
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..ApiConfig::default()
        };

        resolve_api_key(
            &mut api,
            None,
            |name| (name == "OPENAI_API_KEY").then(|| "env-secret".into()),
            |_| Ok(String::new()),
        )
        .unwrap();

        assert_eq!(api.api_key, "env-secret");
    }

    #[test]
    fn missing_api_key_env_source_defaults_to_empty() {
        let mut api = ApiConfig {
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..ApiConfig::default()
        };

        resolve_api_key(&mut api, None, |_| None, |_| Ok(String::new())).unwrap();
        assert!(api.api_key.is_empty());
    }

    #[test]
    fn api_key_file_source_is_trimmed() {
        let mut api = ApiConfig {
            api_key_file: Some("/tmp/key.txt".into()),
            ..ApiConfig::default()
        };

        resolve_api_key(
            &mut api,
            None,
            |_| None,
            |path| {
                assert_eq!(path, "/tmp/key.txt");
                Ok("file-secret\n".into())
            },
        )
        .unwrap();

        assert_eq!(api.api_key, "file-secret");
    }

    #[test]
    fn explicit_api_key_env_override_wins() {
        let mut api = ApiConfig {
            api_key_file: Some("/tmp/key.txt".into()),
            ..ApiConfig::default()
        };

        resolve_api_key(
            &mut api,
            Some("override".into()),
            |_| None,
            |_| Ok("ignored".into()),
        )
        .unwrap();

        assert_eq!(api.api_key, "override");
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
}
