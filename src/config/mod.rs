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

mod defaults;
mod env;
mod init;
mod loader;
mod resolve;
mod selector;
mod sources;
mod types;

use crate::error::ConfigError;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(test)]
use defaults::{
    DEFAULT_API_TIMEOUT_SECS, DEFAULT_BUDDY_CONFIG_TEMPLATE, DEFAULT_FETCH_TIMEOUT_SECS,
};
use types::FileConfig;
pub use types::{
    AgentConfig, ApiConfig, ApiProtocol, AuthMode, Config, ConfigDiagnostics, DisplayConfig,
    GlobalConfigInitResult, LoadedConfig, ModelConfig, NetworkConfig, TmuxConfig, ToolsConfig,
};

/// Load configuration from disk and environment.
///
/// `path_override` is an explicit config file path (from --config flag).
pub fn load_config(path_override: Option<&str>) -> Result<Config, ConfigError> {
    loader::load_config(path_override)
}

/// Load configuration and return compatibility diagnostics.
pub fn load_config_with_diagnostics(
    path_override: Option<&str>,
) -> Result<LoadedConfig, ConfigError> {
    loader::load_config_with_diagnostics(path_override)
}

#[cfg(test)]
/// Test seam for dependency-injected config loading.
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
    loader::load_config_with_diagnostics_from_sources(
        path_override,
        read_file,
        env_lookup,
        config_root,
    )
}

#[cfg(test)]
/// Test seam for dependency-injected config resolution.
fn resolve_config_from_file_config<FEnv, FRead>(
    parsed: FileConfig,
    key_override: Option<String>,
    env_lookup: FEnv,
    read_file: FRead,
    diagnostics: &mut ConfigDiagnostics,
) -> Result<Config, ConfigError>
where
    FEnv: Fn(&str) -> Option<String>,
    FRead: Fn(&str) -> Result<String, ConfigError>,
{
    resolve::resolve_config_from_file_config(
        parsed,
        key_override,
        env_lookup,
        read_file,
        diagnostics,
    )
}

/// Return the default per-user config path (`~/.config/buddy/buddy.toml`).
pub fn default_global_config_path() -> Option<PathBuf> {
    init::default_global_config_path()
}

/// Return the default REPL history path (`~/.config/buddy/history`).
pub fn default_history_path() -> Option<PathBuf> {
    init::default_history_path()
}

/// Initialize `~/.config/buddy/buddy.toml`.
///
/// - Without `force`, returns `AlreadyInitialized` if the file exists.
/// - With `force`, backs up the existing file in the same directory using a
///   timestamped name, then rewrites it from the compiled template.
pub fn initialize_default_global_config(
    force: bool,
) -> Result<GlobalConfigInitResult, ConfigError> {
    init::initialize_default_global_config(force)
}

/// Ensure the default global config file exists.
///
/// Returns the global config path when available on this platform.
pub fn ensure_default_global_config() -> Result<Option<PathBuf>, ConfigError> {
    init::ensure_default_global_config()
}

/// Switch the active profile to a configured `[models.<name>]` entry.
pub fn select_model_profile(config: &mut Config, profile_name: &str) -> Result<(), ConfigError> {
    selector::select_model_profile(config, profile_name)
}

#[cfg(test)]
/// Test seam for path-targeted default-config creation.
fn ensure_default_global_config_at_path(path: &Path) -> Result<(), ConfigError> {
    init::ensure_default_global_config_at_path(path)
}

#[cfg(test)]
/// Test seam for path-targeted default-config initialization.
fn initialize_default_global_config_at_path(
    path: &Path,
    force: bool,
) -> Result<GlobalConfigInitResult, ConfigError> {
    init::initialize_default_global_config_at_path(path, force)
}

#[cfg(test)]
/// Test seam exposing API-key precedence/source resolution behavior.
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
    resolve::resolve_api_key(model, key_override, env_lookup, read_file, path_prefix)
}

/// Resolve the effective config root directory (`$XDG_CONFIG_HOME` or `~/.config`).
pub fn config_root_dir() -> Option<PathBuf> {
    init::config_root_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    // Verifies built-in default config values remain stable and complete.
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
        assert_eq!(c.tmux.max_sessions, 1);
        assert_eq!(c.tmux.max_panes, 5);
    }

    // Verifies partial TOML merges with defaults and activates selected profile.
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

    // Verifies tool security policy fields deserialize correctly from TOML.
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

    // Ensures blank/whitespace agent names normalize back to default identity.
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

    // Verifies network timeout overrides deserialize from TOML.
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

    // Verifies managed tmux limit settings deserialize from TOML.
    #[test]
    fn parse_tmux_limits() {
        let toml = r#"
            [tmux]
            max_sessions = 2
            max_panes = 8
        "#;
        let c = parse_file_config_for_test(toml).unwrap();
        assert_eq!(c.tmux.max_sessions, 2);
        assert_eq!(c.tmux.max_panes, 8);
    }

    // Verifies legacy `[model.*]` alias table is still accepted.
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

    // Verifies per-profile protocol/auth enums resolve into runtime API config.
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

    // Ensures missing `agent.model` defaults to the first available profile.
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

    // Ensures empty config input still yields sane defaults.
    #[test]
    fn parse_empty_string() {
        let c = parse_file_config_for_test("").unwrap();
        assert_eq!(c.agent.model, "gpt-codex");
        assert_eq!(c.api.model, "gpt-5.3-codex");
    }

    // Ensures mutually exclusive API-key source validation is enforced.
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

    // Ensures env-sourced API keys are resolved from configured env var names.
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

    // Ensures missing env key sources degrade to empty strings (not hard errors).
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

    // Ensures file-sourced API keys trim trailing newlines from key files.
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

    // Ensures explicit runtime/API-key override takes precedence over file source.
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

    // Ensures profile switching updates both `agent.model` and active API fields.
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

    // Ensures bootstrap helper writes the embedded default config template.
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

    // Ensures non-force init preserves existing config files unchanged.
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

    // Ensures force init overwrites current config and emits a backup file.
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

    // Ensures explicit legacy file naming (`agent.toml`) emits deprecation diagnostics.
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

    // Ensures legacy top-level `[api]` section emits migration diagnostics.
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

    // Verifies source precedence prefers local `buddy.toml` over global config.
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

    // Verifies env overrides apply after file parsing in injected-source path.
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

    // Verifies legacy env aliases are still honored and flagged.
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

    // Verifies canonical env vars win over corresponding legacy aliases.
    #[test]
    fn injected_sources_prefer_canonical_env_over_legacy_alias() {
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
        env.insert("BUDDY_MODEL".to_string(), "canonical-model".to_string());
        env.insert("AGENT_MODEL".to_string(), "legacy-model".to_string());

        let loaded =
            load_config_with_sources_for_test(None, files, env, Some(PathBuf::from("/cfg")))
                .unwrap();

        assert_eq!(loaded.config.api.model, "canonical-model");
        assert!(!loaded
            .diagnostics
            .deprecations
            .iter()
            .any(|msg| msg.contains("AGENT_MODEL")));
    }

    // Verifies explicit path override outranks both local and global config files.
    #[test]
    fn injected_sources_explicit_path_override_beats_local_and_global() {
        let mut files = BTreeMap::<String, String>::new();
        files.insert(
            "/override.toml".to_string(),
            r#"
            [models.explicit]
            model = "explicit-model"
            api_base_url = "https://explicit.example/v1"

            [agent]
            model = "explicit"
            "#
            .to_string(),
        );
        files.insert(
            "buddy.toml".to_string(),
            r#"
            [models.local]
            model = "local-model"

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
            Some("/override.toml"),
            files,
            BTreeMap::new(),
            Some(PathBuf::from("/cfg")),
        )
        .unwrap();

        assert_eq!(loaded.config.agent.model, "explicit");
        assert_eq!(loaded.config.api.model, "explicit-model");
        assert_eq!(loaded.config.api.base_url, "https://explicit.example/v1");
    }

    /// Test helper that wires an in-memory file/env view into the loader.
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

    /// Test helper for parsing TOML into runtime `Config` with default injections.
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
