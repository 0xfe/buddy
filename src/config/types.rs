//! Configuration data model.
//!
//! This module intentionally holds struct/enum definitions plus default values.
//! Loader and source-resolution logic remains in `config::mod` so parsing and
//! precedence behavior stays centralized.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::defaults::{
    default_models_map, DEFAULT_AGENT_NAME, DEFAULT_API_BASE_URL, DEFAULT_API_TIMEOUT_SECS,
    DEFAULT_FETCH_TIMEOUT_SECS, DEFAULT_MODEL_ID, DEFAULT_MODEL_PROFILE_NAME,
};

/// Provider wire protocol for model requests.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApiProtocol {
    #[default]
    Completions,
    Responses,
}

/// Authentication mode for a model profile.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    #[default]
    ApiKey,
    Login,
}

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
        let api = super::resolve::resolve_active_api_with(
            &models,
            &agent.model,
            None,
            |_| None,
            |_| Ok(String::new()),
        )
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
    pub(super) fn resolved_model_name(&self, profile_name: &str) -> String {
        super::resolve::normalized_option(&self.model).unwrap_or_else(|| profile_name.to_string())
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
    /// Operator-provided agent identity used for tmux session naming.
    pub name: String,
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
            name: DEFAULT_AGENT_NAME.to_string(),
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
    /// Optional allowlist roots for `write_file`. When non-empty, writes are
    /// only permitted under one of these paths.
    pub files_allowed_paths: Vec<String>,
    pub search_enabled: bool,
    /// Whether to prompt the user before running shell commands.
    pub shell_confirm: bool,
    /// Command denylist patterns for `run_shell`.
    pub shell_denylist: Vec<String>,
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
            files_allowed_paths: Vec::new(),
            search_enabled: true,
            shell_confirm: true,
            shell_denylist: vec![
                "rm -rf /".to_string(),
                "mkfs".to_string(),
                "shutdown".to_string(),
                "reboot".to_string(),
                "dd if=".to_string(),
                ":(){ :|:& };:".to_string(),
            ],
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
    /// Persist REPL input history under `~/.config/buddy/history`.
    pub persist_history: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            color: true,
            show_tokens: false,
            show_tool_calls: true,
            persist_history: true,
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
pub(super) struct FileConfig {
    #[serde(alias = "model")]
    pub(super) models: BTreeMap<String, ModelConfig>,
    /// Legacy compatibility for older configs that still use `[api]`.
    pub(super) api: Option<LegacyApiConfig>,
    pub(super) agent: AgentConfig,
    pub(super) tools: ToolsConfig,
    pub(super) network: NetworkConfig,
    pub(super) display: DisplayConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(super) struct LegacyApiConfig {
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) api_key_env: Option<String>,
    pub(super) api_key_file: Option<String>,
    pub(super) model: String,
    pub(super) context_limit: Option<usize>,
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
    pub(super) fn into_model_config(self) -> ModelConfig {
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

/// Diagnostics captured while resolving runtime configuration.
#[derive(Debug, Clone, Default)]
pub struct ConfigDiagnostics {
    /// Legacy compatibility paths currently in use.
    pub deprecations: Vec<String>,
}

/// Configuration payload plus load-time diagnostics.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub diagnostics: ConfigDiagnostics,
}

/// Result of explicit global config initialization (`buddy init`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalConfigInitResult {
    Created {
        path: std::path::PathBuf,
    },
    AlreadyInitialized {
        path: std::path::PathBuf,
    },
    Overwritten {
        path: std::path::PathBuf,
        backup_path: std::path::PathBuf,
    },
}
