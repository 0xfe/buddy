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
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_BUDDY_CONFIG_TEMPLATE: &str = include_str!("templates/buddy.toml");
const DEFAULT_MODEL_PROFILE_NAME: &str = "gpt-codex";
const DEFAULT_MODEL_ID: &str = "gpt-5.3-codex";
const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_API_TIMEOUT_SECS: u64 = 120;
const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 20;

/// Provider wire protocol for model requests.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApiProtocol {
    #[default]
    Completions,
    Responses,
}

/// Authentication mode for a model profile.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    #[default]
    ApiKey,
    Login,
}

// ---------------------------------------------------------------------------
// Config structs
// ---------------------------------------------------------------------------

/// Top-level runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Resolved active API settings from `agent.model` + `models.<name>`.
    pub api: ApiConfig,
    /// Configured model profiles keyed by profile name.
    pub models: BTreeMap<String, ModelConfig>,
    pub agent: AgentConfig,
    pub tools: ToolsConfig,
    pub network: NetworkConfig,
    pub display: DisplayConfig,
}

impl Default for Config {
    fn default() -> Self {
        let models = default_models_map();
        let mut agent = AgentConfig::default();
        agent.model = DEFAULT_MODEL_PROFILE_NAME.into();
        let api =
            resolve_active_api_with(&models, &agent.model, None, |_| None, |_| Ok(String::new()))
                .unwrap_or_default();
        Self {
            api,
            models,
            agent,
            tools: ToolsConfig::default(),
            network: NetworkConfig::default(),
            display: DisplayConfig::default(),
        }
    }
}

/// Resolved API connection settings used by the runtime HTTP client.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub protocol: ApiProtocol,
    pub auth: AuthMode,
    /// Selected profile key (for login-token lookup and UX messaging).
    pub profile: String,
    /// Override for context window size. Auto-detected from model name if omitted.
    pub context_limit: Option<usize>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_API_BASE_URL.into(),
            api_key: String::new(),
            model: DEFAULT_MODEL_ID.into(),
            protocol: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            profile: DEFAULT_MODEL_PROFILE_NAME.to_string(),
            context_limit: None,
        }
    }
}

impl ApiConfig {
    /// True when the active profile requires stored login credentials.
    pub fn uses_login(&self) -> bool {
        self.auth == AuthMode::Login && self.api_key.trim().is_empty()
    }
}

/// API model-profile settings stored under `[models.<name>]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    #[serde(alias = "base_url")]
    pub api_base_url: String,
    #[serde(default)]
    pub api: ApiProtocol,
    #[serde(default)]
    pub auth: AuthMode,
    pub api_key: String,
    pub api_key_env: Option<String>,
    pub api_key_file: Option<String>,
    /// Optional concrete model id; defaults to the profile key when omitted.
    pub model: Option<String>,
    /// Optional override for context window size.
    pub context_limit: Option<usize>,
}

impl ModelConfig {
    fn resolved_model_name(&self, profile_name: &str) -> String {
        normalized_option(&self.model).unwrap_or_else(|| profile_name.to_string())
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            api_base_url: DEFAULT_API_BASE_URL.into(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: None,
            context_limit: None,
        }
    }
}

/// Agent behavior settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Active model-profile key (must exist under `[models.<name>]`).
    pub model: String,
    pub system_prompt: String,
    /// Safety cap on agentic loop iterations.
    pub max_iterations: usize,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
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
    /// Optional confirmation prompt for `fetch_url` tool executions.
    pub fetch_confirm: bool,
    /// Domain allowlist for `fetch_url`. When non-empty, only matching domains are allowed.
    pub fetch_allowed_domains: Vec<String>,
    /// Domain denylist for `fetch_url`. Matches exact domain and subdomains.
    pub fetch_blocked_domains: Vec<String>,
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
            fetch_confirm: false,
            fetch_allowed_domains: Vec::new(),
            fetch_blocked_domains: Vec::new(),
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

/// Network/HTTP timeout policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// Default timeout for model API requests.
    pub api_timeout_secs: u64,
    /// Timeout for `fetch_url` tool requests.
    pub fetch_timeout_secs: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            api_timeout_secs: DEFAULT_API_TIMEOUT_SECS,
            fetch_timeout_secs: DEFAULT_FETCH_TIMEOUT_SECS,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    #[serde(alias = "model")]
    models: BTreeMap<String, ModelConfig>,
    /// Legacy compatibility for older configs that still use `[api]`.
    api: Option<LegacyApiConfig>,
    agent: AgentConfig,
    tools: ToolsConfig,
    network: NetworkConfig,
    display: DisplayConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct LegacyApiConfig {
    base_url: String,
    api_key: String,
    api_key_env: Option<String>,
    api_key_file: Option<String>,
    model: String,
    context_limit: Option<usize>,
}

impl Default for LegacyApiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_API_BASE_URL.into(),
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: DEFAULT_MODEL_ID.into(),
            context_limit: None,
        }
    }
}

impl LegacyApiConfig {
    fn into_model_config(self) -> ModelConfig {
        ModelConfig {
            api_base_url: self.base_url,
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: self.api_key,
            api_key_env: self.api_key_env,
            api_key_file: self.api_key_file,
            model: Some(self.model),
            context_limit: self.context_limit,
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

    let mut parsed: FileConfig = toml::from_str(&config_text)?;

    if parsed.models.is_empty() {
        if let Some(legacy_api) = parsed.api.take() {
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

    // Environment variable overrides for active runtime settings.
    if let Ok(url) = std::env::var("BUDDY_BASE_URL").or_else(|_| std::env::var("AGENT_BASE_URL")) {
        config.api.base_url = url;
    }
    if let Ok(model) = std::env::var("BUDDY_MODEL").or_else(|_| std::env::var("AGENT_MODEL")) {
        config.api.model = model;
    }
    if let Ok(timeout) =
        std::env::var("BUDDY_API_TIMEOUT_SECS").or_else(|_| std::env::var("AGENT_API_TIMEOUT_SECS"))
    {
        let parsed = timeout.parse::<u64>().map_err(|_| {
            ConfigError::Invalid(format!(
                "invalid BUDDY_API_TIMEOUT_SECS value `{timeout}`: expected positive integer seconds"
            ))
        })?;
        config.network.api_timeout_secs = parsed.max(1);
    }
    if let Ok(timeout) = std::env::var("BUDDY_FETCH_TIMEOUT_SECS")
        .or_else(|_| std::env::var("AGENT_FETCH_TIMEOUT_SECS"))
    {
        let parsed = timeout.parse::<u64>().map_err(|_| {
            ConfigError::Invalid(format!(
                "invalid BUDDY_FETCH_TIMEOUT_SECS value `{timeout}`: expected positive integer seconds"
            ))
        })?;
        config.network.fetch_timeout_secs = parsed.max(1);
    }

    Ok(config)
}

/// Return the default per-user config path (`~/.config/buddy/buddy.toml`).
pub fn default_global_config_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("buddy.toml"))
}

/// Result of explicit global config initialization (`buddy init`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalConfigInitResult {
    Created { path: PathBuf },
    AlreadyInitialized { path: PathBuf },
    Overwritten { path: PathBuf, backup_path: PathBuf },
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
    std::env::var("BUDDY_API_KEY")
        .or_else(|_| std::env::var("AGENT_API_KEY"))
        .ok()
}

fn default_models_map() -> BTreeMap<String, ModelConfig> {
    let mut models = BTreeMap::new();
    models.insert(
        DEFAULT_MODEL_PROFILE_NAME.to_string(),
        ModelConfig {
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            api: ApiProtocol::Responses,
            auth: AuthMode::Login,
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: Some(DEFAULT_MODEL_ID.to_string()),
            context_limit: None,
        },
    );
    models.insert(
        "gpt-spark".to_string(),
        ModelConfig {
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            api: ApiProtocol::Responses,
            auth: AuthMode::Login,
            api_key: String::new(),
            api_key_env: None,
            api_key_file: None,
            model: Some("gpt-5.3-codex-spark".to_string()),
            context_limit: None,
        },
    );
    models.insert(
        "openrouter-deepseek".to_string(),
        ModelConfig {
            api_base_url: "https://openrouter.ai/api/v1".to_string(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            api_key_file: None,
            model: Some("deepseek/deepseek-v3.2".to_string()),
            context_limit: None,
        },
    );
    models.insert(
        "openrouter-glm".to_string(),
        ModelConfig {
            api_base_url: "https://openrouter.ai/api/v1".to_string(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            api_key_file: None,
            model: Some("z-ai/glm-5".to_string()),
            context_limit: None,
        },
    );
    models.insert(
        "kimi".to_string(),
        ModelConfig {
            api_base_url: "https://api.moonshot.ai/v1".to_string(),
            api: ApiProtocol::Completions,
            auth: AuthMode::ApiKey,
            api_key: String::new(),
            api_key_env: Some("MOONSHOT_API_KEY".to_string()),
            api_key_file: None,
            model: Some("kimi-k2.5".to_string()),
            context_limit: None,
        },
    );
    models
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

    #[test]
    fn defaults_are_sensible() {
        let c = Config::default();
        assert_eq!(c.agent.model, "gpt-codex");
        assert_eq!(c.api.base_url, "https://api.openai.com/v1");
        assert_eq!(c.api.model, "gpt-5.3-codex");
        assert_eq!(c.api.protocol, ApiProtocol::Responses);
        assert_eq!(c.api.auth, AuthMode::Login);
        assert_eq!(c.agent.max_iterations, 20);
        assert!(c.tools.shell_enabled);
        assert!(c.display.color);
        assert!(c.models.contains_key("gpt-codex"));
        assert!(c.models.contains_key("gpt-spark"));
        assert!(c.models.contains_key("openrouter-deepseek"));
        assert!(c.models.contains_key("openrouter-glm"));
        assert!(c.models.contains_key("kimi"));
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
        assert_eq!(c.agent.model, "kimi");
        assert_eq!(c.api.model, "kimi");
        assert_eq!(c.api.base_url, "https://api.moonshot.ai/v1");
        assert!(!c.tools.shell_confirm);
        assert!(c.display.color);
    }

    #[test]
    fn parse_fetch_security_policy() {
        let toml = r#"
            [tools]
            fetch_confirm = true
            fetch_allowed_domains = ["example.com", "api.example.com"]
            fetch_blocked_domains = ["internal.example.com", "localhost"]
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

    fn parse_file_config_for_test(toml_text: &str) -> Result<Config, ConfigError> {
        let mut parsed: FileConfig = toml::from_str(toml_text)?;
        if parsed.models.is_empty() {
            if let Some(legacy_api) = parsed.api.take() {
                parsed.models.insert(
                    DEFAULT_MODEL_PROFILE_NAME.to_string(),
                    legacy_api.into_model_config(),
                );
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

        let api = resolve_active_api_with(
            &parsed.models,
            &parsed.agent.model,
            None,
            |_| None,
            |_| Ok(String::new()),
        )?;

        Ok(Config {
            api,
            models: parsed.models,
            agent: parsed.agent,
            tools: parsed.tools,
            network: parsed.network,
            display: parsed.display,
        })
    }
}
