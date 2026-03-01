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
    /// OpenAI-style `/chat/completions` payload shape.
    #[default]
    Completions,
    /// OpenAI-style `/responses` payload shape.
    Responses,
}

/// Authentication mode for a model profile.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    /// Static API key auth (inline/env/file sourced).
    #[default]
    ApiKey,
    /// Login-token auth (provider credentials loaded from local token store).
    Login,
}

/// Top-level runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Resolved active API settings from `agent.model` + `models.<name>`.
    pub api: ApiConfig,
    /// Configured model profiles keyed by profile name.
    pub models: BTreeMap<String, ModelConfig>,
    /// Agent behavior/runtime parameters.
    pub agent: AgentConfig,
    /// Tool enablement and policy controls.
    pub tools: ToolsConfig,
    /// Network timeout defaults.
    pub network: NetworkConfig,
    /// Output/UI display preferences.
    pub display: DisplayConfig,
    /// Optional named theme override tables (`[themes.<name>]`).
    pub themes: BTreeMap<String, ThemeOverrideConfig>,
    /// Managed tmux session/pane limits and policy knobs.
    pub tmux: TmuxConfig,
}

impl Default for Config {
    fn default() -> Self {
        let models = default_models_map();
        // Default agent points to the default bundled profile key.
        let agent = AgentConfig {
            model: DEFAULT_MODEL_PROFILE_NAME.into(),
            ..AgentConfig::default()
        };
        // Resolve active API exactly as runtime would (env/file hooks disabled here).
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
            themes: BTreeMap::new(),
            tmux: TmuxConfig::default(),
        }
    }
}

/// Resolved API connection settings used by the runtime HTTP client.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Provider base URL (e.g., `https://api.openai.com/v1`).
    pub base_url: String,
    /// Resolved API key value (possibly empty for login/local endpoints).
    pub api_key: String,
    /// Concrete provider model ID to request.
    pub model: String,
    /// API wire protocol variant used for request/response formatting.
    pub protocol: ApiProtocol,
    /// Auth mechanism associated with this profile.
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
    /// Profile-specific API base URL.
    #[serde(alias = "base_url")]
    pub api_base_url: String,
    /// Provider protocol for this profile.
    #[serde(default)]
    pub api: ApiProtocol,
    /// Auth mechanism for this profile.
    #[serde(default)]
    pub auth: AuthMode,
    /// Inline literal API key (mutually exclusive with env/file sources).
    pub api_key: String,
    /// Environment variable name to read API key from.
    pub api_key_env: Option<String>,
    /// File path to read API key text from.
    pub api_key_file: Option<String>,
    /// Optional concrete model id; defaults to the profile key when omitted.
    pub model: Option<String>,
    /// Optional override for context window size.
    pub context_limit: Option<usize>,
}

impl ModelConfig {
    /// Resolve concrete model ID, defaulting to the profile key when absent.
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
    /// Additional operator instructions appended to system prompt rendering.
    pub system_prompt: String,
    /// Safety cap on agentic loop iterations.
    pub max_iterations: usize,
    /// Optional model temperature override.
    pub temperature: Option<f64>,
    /// Optional nucleus-sampling override.
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
    /// Enable `run_shell` tool registration.
    pub shell_enabled: bool,
    /// Enable `fetch_url` tool registration.
    pub fetch_enabled: bool,
    /// Optional confirmation prompt for `fetch_url` tool executions.
    pub fetch_confirm: bool,
    /// Domain allowlist for `fetch_url`. When non-empty, only matching domains are allowed.
    pub fetch_allowed_domains: Vec<String>,
    /// Domain denylist for `fetch_url`. Matches exact domain and subdomains.
    pub fetch_blocked_domains: Vec<String>,
    /// Enable filesystem read/write tools.
    pub files_enabled: bool,
    /// Optional allowlist roots for `write_file`. When non-empty, writes are
    /// only permitted under one of these paths.
    pub files_allowed_paths: Vec<String>,
    /// Enable web search tool registration.
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
            // Conservative baseline denylist for dangerous shell operations.
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
    /// Enable ANSI colorized terminal output.
    pub color: bool,
    /// Show token usage stats in UI/status lines.
    pub show_tokens: bool,
    /// Show tool-call metadata in output stream.
    pub show_tool_calls: bool,
    /// Persist REPL input history under `~/.config/buddy/history`.
    pub persist_history: bool,
    /// Active terminal theme name (`dark`, `light`, or custom from `[themes.*]`).
    pub theme: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            color: true,
            show_tokens: false,
            show_tool_calls: true,
            persist_history: true,
            theme: "dark".to_string(),
        }
    }
}

/// Raw theme-override table for one named theme.
///
/// Example:
/// `[themes.my-theme]`
/// `warning = "#ffaa00"`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct ThemeOverrideConfig {
    /// Semantic token -> color value string mapping.
    pub values: BTreeMap<String, String>,
}

/// Managed tmux lifecycle constraints.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TmuxConfig {
    /// Maximum number of managed tmux sessions for this agent identity.
    pub max_sessions: usize,
    /// Maximum number of managed tmux panes per managed session.
    pub max_panes: usize,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            max_sessions: 1,
            max_panes: 5,
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
    /// Parsed modern model-profile table (`[models.<name>]`).
    #[serde(alias = "model")]
    pub(super) models: BTreeMap<String, ModelConfig>,
    /// Legacy compatibility for older configs that still use `[api]`.
    /// Legacy flat API section kept for compatibility migration.
    pub(super) api: Option<LegacyApiConfig>,
    /// Agent section from config file.
    pub(super) agent: AgentConfig,
    /// Tool section from config file.
    pub(super) tools: ToolsConfig,
    /// Network section from config file.
    pub(super) network: NetworkConfig,
    /// Display section from config file.
    pub(super) display: DisplayConfig,
    /// Optional custom theme override tables from config file.
    pub(super) themes: BTreeMap<String, ThemeOverrideConfig>,
    /// Tmux section from config file.
    pub(super) tmux: TmuxConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(super) struct LegacyApiConfig {
    /// Legacy base URL field from `[api]`.
    pub(super) base_url: String,
    /// Legacy inline API key field from `[api]`.
    pub(super) api_key: String,
    /// Legacy env API key source from `[api]`.
    pub(super) api_key_env: Option<String>,
    /// Legacy file API key source from `[api]`.
    pub(super) api_key_file: Option<String>,
    /// Legacy model ID field from `[api]`.
    pub(super) model: String,
    /// Legacy context limit override field from `[api]`.
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
    /// Convert legacy `[api]` shape into a modern single-profile model config.
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
    /// Fully resolved runtime config.
    pub config: Config,
    /// Warnings/deprecations collected during load.
    pub diagnostics: ConfigDiagnostics,
}

/// Result of explicit global config initialization (`buddy init`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalConfigInitResult {
    /// Config file was newly created.
    Created {
        /// Path written.
        path: std::path::PathBuf,
    },
    /// Config file already existed and was left unchanged.
    AlreadyInitialized {
        /// Existing config path.
        path: std::path::PathBuf,
    },
    /// Existing config was overwritten after backup.
    Overwritten {
        /// Path rewritten with template content.
        path: std::path::PathBuf,
        /// Backup file path containing previous config.
        backup_path: std::path::PathBuf,
    },
}
