//! Live model regression probes.
//!
//! This suite is intentionally `#[ignore]` and is never run by default.
//! It validates that the default model profiles from `src/templates/buddy.toml`
//! can execute a tiny prompt end-to-end with current auth semantics.
//!
//! Run explicitly:
//! `cargo test --test model_regression -- --ignored --nocapture`

use buddy::api::ApiClient;
use buddy::auth::{
    api_key_provider_key, load_provider_api_key, load_provider_tokens, login_provider_key,
};
use buddy::config::{load_config, select_model_profile, AuthMode, Config};
use buddy::tokens::model_auth_capabilities;
use buddy::types::{
    ChatRequest, FunctionCall, FunctionDefinition, Message, Role, ToolCall, ToolDefinition,
};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

/// Embedded default config template used as the regression test source-of-truth.
const TEMPLATE_BUDDY_TOML: &str = include_str!("../src/templates/buddy.toml");

#[tokio::test]
#[ignore = "network regression suite; run explicitly"]
async fn default_template_profiles_round_trip() {
    // Validate template-derived profiles end-to-end against live providers.
    assert_no_global_runtime_overrides().expect("clean runtime env required");

    let path = write_temp_template_config().expect("temp config");
    let mut config = load_config(Some(path.to_string_lossy().as_ref())).expect("load config");
    let profile_names = configured_profile_names(&config);

    let mut failures = Vec::<String>::new();
    let mut skipped = Vec::<String>::new();
    // Probe each configured profile independently to surface profile-local regressions.
    for profile in &profile_names {
        eprintln!("[model-regression] profile={profile}");
        match run_profile_probe(&mut config, profile).await {
            Ok(()) => eprintln!("[model-regression] profile={profile} ok"),
            Err(err) if is_skippable_profile_error(&err) => {
                eprintln!("[model-regression] profile={profile} skipped: {err}");
                skipped.push(format!("{profile}: {err}"));
            }
            Err(err) => failures.push(format!("{profile}: {err}")),
        }
    }

    let _ = fs::remove_file(&path);

    if !failures.is_empty() {
        panic!(
            "model regression failures:\n{}\n\nskipped profiles:\n{}",
            failures
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            if skipped.is_empty() {
                "- none".to_string()
            } else {
                skipped
                    .iter()
                    .map(|line| format!("- {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        );
    }
}

#[tokio::test]
#[ignore = "network regression suite; run explicitly"]
async fn default_template_profiles_accept_tool_error_history() {
    // Validate provider-side protocol compatibility for tool-error recovery
    // histories across all default template profiles.
    assert_no_global_runtime_overrides().expect("clean runtime env required");

    let path = write_temp_template_config().expect("temp config");
    let mut config = load_config(Some(path.to_string_lossy().as_ref())).expect("load config");
    let profile_names = configured_profile_names(&config);

    let mut failures = Vec::<String>::new();
    let mut skipped = Vec::<String>::new();
    for profile in &profile_names {
        eprintln!("[model-regression:tool-error-history] profile={profile}");
        match run_profile_tool_error_history_probe(&mut config, profile).await {
            Ok(()) => eprintln!("[model-regression:tool-error-history] profile={profile} ok"),
            Err(err) if is_skippable_profile_error(&err) => {
                eprintln!("[model-regression:tool-error-history] profile={profile} skipped: {err}");
                skipped.push(format!("{profile}: {err}"));
            }
            Err(err) => failures.push(format!("{profile}: {err}")),
        }
    }

    let _ = fs::remove_file(&path);

    if !failures.is_empty() {
        panic!(
            "model regression failures (tool-error-history):\n{}\n\nskipped profiles:\n{}",
            failures
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            if skipped.is_empty() {
                "- none".to_string()
            } else {
                skipped
                    .iter()
                    .map(|line| format!("- {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        );
    }
}

/// Ensure global env overrides are not masking template-profile behavior.
fn assert_no_global_runtime_overrides() -> Result<(), String> {
    let mut blocked = Vec::<String>::new();
    for key in [
        "BUDDY_API_KEY",
        "AGENT_API_KEY",
        "BUDDY_BASE_URL",
        "AGENT_BASE_URL",
        "BUDDY_MODEL",
        "AGENT_MODEL",
    ] {
        if std::env::var(key)
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
        {
            blocked.push(key.to_string());
        }
    }

    if blocked.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unset these overrides before running model regression tests: {}",
            blocked.join(", ")
        ))
    }
}

/// Run a minimal no-tools request against one selected profile.
async fn run_profile_probe(config: &mut Config, profile_name: &str) -> Result<(), String> {
    select_model_profile(config, profile_name)
        .map_err(|err| format!("failed selecting profile: {err}"))?;
    profile_auth_preflight(config, profile_name)?;

    let client = ApiClient::new(
        &config.api,
        Duration::from_secs(config.network.api_timeout_secs),
    );
    let request = ChatRequest {
        model: config.api.model.clone(),
        messages: vec![
            Message::system("You are a regression probe. Reply with exactly: OK"),
            Message::user("Reply with exactly OK."),
        ],
        tools: None,
        temperature: None,
        top_p: None,
    };

    let response = chat_with_probe_retries(&client, &request, "round-trip").await?;

    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "provider returned empty choices".to_string())?;

    if choice
        .message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty())
    {
        return Err("unexpected tool-calls for no-tools probe request".to_string());
    }

    assert_reasoning_payload_hygiene(profile_name, &choice.message)?;

    let text = choice.message.content.clone().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(format!(
            "empty assistant content (finish_reason={:?})",
            choice.finish_reason
        ));
    }

    Ok(())
}

/// Run a minimal probe that injects prior tool-call + tool-error history and
/// verifies the provider can continue with a normal assistant response.
async fn run_profile_tool_error_history_probe(
    config: &mut Config,
    profile_name: &str,
) -> Result<(), String> {
    select_model_profile(config, profile_name)
        .map_err(|err| format!("failed selecting profile: {err}"))?;
    profile_auth_preflight(config, profile_name)?;

    let client = ApiClient::new(
        &config.api,
        Duration::from_secs(config.network.api_timeout_secs),
    );
    let request = ChatRequest {
        model: config.api.model.clone(),
        messages: vec![
            Message::system("You are a regression probe. Reply with exactly: RECOVERED"),
            Message::user("Use a tool and then continue."),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_regression".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "run_shell".to_string(),
                        arguments: "{\"command\":\"false\"}".to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                extra: Default::default(),
            },
            Message::tool_result("call_regression", "Tool error: command failed (regression)"),
            Message::user("Ignoring previous tool error, reply with exactly RECOVERED."),
        ],
        tools: Some(vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "run_shell".to_string(),
                description: "Runs a shell command and returns output".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            },
        }]),
        temperature: None,
        top_p: None,
    };

    let response = chat_with_probe_retries(&client, &request, "tool-error-history").await?;

    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "provider returned empty choices".to_string())?;
    let text = choice.message.content.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(format!(
            "empty assistant content for tool-error-history probe (finish_reason={:?})",
            choice.finish_reason
        ));
    }
    Ok(())
}

/// Validate model-specific reasoning payload hygiene for cross-provider compatibility.
fn assert_reasoning_payload_hygiene(
    profile_name: &str,
    message: &buddy::types::Message,
) -> Result<(), String> {
    let mut violations = Vec::<String>::new();
    for (key, value) in &message.extra {
        let normalized_key = key.to_ascii_lowercase();
        if !(normalized_key.contains("reasoning")
            || normalized_key.contains("thinking")
            || normalized_key.contains("thought"))
        {
            continue;
        }
        if let Some(raw) = value.as_str() {
            let normalized = raw.trim().to_ascii_lowercase();
            if matches!(normalized.as_str(), "null" | "none" | "[]" | "{}") {
                violations.push(format!("{key}={raw:?}"));
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "reasoning payload hygiene failed for profile `{profile_name}`: {}",
            violations.join(", ")
        ))
    }
}

/// Verify auth prerequisites for the selected profile before issuing network requests.
fn profile_auth_preflight(config: &mut Config, profile_name: &str) -> Result<(), String> {
    align_profile_auth_mode_with_model_capabilities(config)?;
    hydrate_profile_api_key_from_fallbacks(config)?;

    let profile = config
        .models
        .get(profile_name)
        .ok_or_else(|| format!("profile `{profile_name}` not found in config"))?;

    if config.api.auth == AuthMode::ApiKey {
        if config.api.api_key.trim().is_empty() {
            let source_hint = if let Some(env_name) = profile.api_key_env.as_deref() {
                format!("set env var `{env_name}`")
            } else if let Some(default_env) = provider_default_api_key_env(&api_key_provider_key(
                config.api.provider,
                &config.api.base_url,
            )) {
                format!(
                    "set env var `{default_env}` (or configure `{profile_name}` api_key/api_key_env/api_key_file)"
                )
            } else if profile.api_key_file.as_deref().is_some() {
                "set configured api_key_file contents".to_string()
            } else {
                format!(
                    "set `{profile_name}` api_key (or use api_key_env/api_key_file in template-derived config)"
                )
            };
            return Err(format!("missing API key ({source_hint})"));
        }
        return Ok(());
    }

    if config.api.auth == AuthMode::Login && config.api.api_key.trim().is_empty() {
        let provider = login_provider_key(config.api.provider, &config.api.base_url)
            .ok_or_else(|| {
                format!(
                    "login auth profile `{profile_name}` has unsupported provider/base URL `{:?}` + `{}`",
                    config.api.provider, config.api.base_url
                )
            })?;

        let tokens = load_provider_tokens(provider)
            .map_err(|err| format!("failed loading saved login for `{provider}`: {err}"))?;

        if tokens.is_none() {
            return Err(format!(
                "missing saved login for provider `{provider}`; run `buddy login {provider}` first"
            ));
        }
    }

    Ok(())
}

/// Align test auth mode with catalog-declared model capability metadata.
///
/// This allows live probes to exercise login-only models without requiring
/// profile-level auth bindings in the template.
fn align_profile_auth_mode_with_model_capabilities(config: &mut Config) -> Result<(), String> {
    let caps = model_auth_capabilities(&config.api.model);
    match config.api.auth {
        AuthMode::ApiKey if !caps.supports_api_key_auth => {
            if caps.supports_login_auth {
                config.api.auth = AuthMode::Login;
                config.api.api_key.clear();
                Ok(())
            } else {
                Err(format!(
                    "model `{}` does not support api-key auth and has no supported alternative auth mode",
                    config.api.model
                ))
            }
        }
        AuthMode::Login if !caps.supports_login_auth => Err(format!(
            "model `{}` does not support login auth; configure this profile for api-key auth",
            config.api.model
        )),
        _ => Ok(()),
    }
}

/// Hydrate `config.api.api_key` from provider-level fallbacks when profiles do
/// not set explicit key-source fields. This mirrors normal runtime behavior
/// where provider keys can come from encrypted auth storage.
fn hydrate_profile_api_key_from_fallbacks(config: &mut Config) -> Result<(), String> {
    if !config.api.api_key.trim().is_empty() {
        return Ok(());
    }

    let provider_key = api_key_provider_key(config.api.provider, &config.api.base_url);
    if config.api.auth == AuthMode::ApiKey {
        if let Some(stored_key) = load_provider_api_key(&provider_key).map_err(|err| {
            format!("failed loading stored API key for provider `{provider_key}`: {err}")
        })? {
            if !stored_key.trim().is_empty() {
                config.api.api_key = stored_key;
                return Ok(());
            }
        }
    }

    if config.api.auth == AuthMode::ApiKey {
        if let Some(env_name) = provider_default_api_key_env(&provider_key) {
            if let Ok(value) = std::env::var(env_name) {
                if !value.trim().is_empty() {
                    config.api.api_key = value;
                }
            }
        }
    }
    Ok(())
}

/// Resolve conventional provider-level API key environment variable names.
fn provider_default_api_key_env(provider_key: &str) -> Option<&'static str> {
    match provider_key {
        "openai" => Some("OPENAI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "moonshot" => Some("MOONSHOT_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        _ => None,
    }
}

/// Send one regression probe request with small bounded retries for transient
/// provider/network failures.
async fn chat_with_probe_retries(
    client: &ApiClient,
    request: &ChatRequest,
    probe_label: &str,
) -> Result<buddy::types::ChatResponse, String> {
    let mut last_error = String::new();
    for attempt in 1..=2 {
        let result = timeout(Duration::from_secs(90), client.chat(request))
            .await
            .map_err(|_| "request timed out after 90s".to_string())
            .and_then(|result| result.map_err(|err| err.to_string()));
        match result {
            Ok(response) => return Ok(response),
            Err(err) => {
                last_error = err;
                if attempt < 2 && is_retryable_probe_error(&last_error) {
                    eprintln!(
                        "[model-regression] retrying {probe_label} probe after transient error: {}",
                        one_line(&last_error)
                    );
                    continue;
                }
                break;
            }
        }
    }
    Err(last_error)
}

/// Identify transient errors that are worth a single retry in live probes.
fn is_retryable_probe_error(err: &str) -> bool {
    let normalized = err.to_ascii_lowercase();
    normalized.contains("request timed out")
        || normalized.contains("status 429")
        || normalized.contains("status 500")
        || normalized.contains("status 502")
        || normalized.contains("status 503")
        || normalized.contains("status 504")
}

/// Collapse multiline provider errors to one line for compact retry logs.
fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Returns true when a profile failure should be reported as skipped (not failed).
///
/// This is limited to account/profile availability issues (for example a model
/// alias that no longer exists), not credential or protocol regressions.
fn is_skippable_profile_error(err: &str) -> bool {
    let normalized = err.to_ascii_lowercase();
    normalized.contains("model_not_found")
        || (normalized.contains("requested model") && normalized.contains("does not exist"))
}

/// Return configured model profile names from loaded config.
fn configured_profile_names(config: &Config) -> Vec<String> {
    config.models.keys().cloned().collect()
}

/// Materialize the template config into a unique temp file for this test run.
fn write_temp_template_config() -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("clock error: {err}"))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "buddy-model-regression-{}-{nonce}.toml",
        std::process::id()
    ));
    fs::write(&path, TEMPLATE_BUDDY_TOML)
        .map_err(|err| format!("failed writing temp config {}: {err}", path.display()))?;
    Ok(path)
}

#[test]
fn provider_default_api_key_env_matches_supported_providers() {
    assert_eq!(
        provider_default_api_key_env("openai"),
        Some("OPENAI_API_KEY")
    );
    assert_eq!(
        provider_default_api_key_env("openrouter"),
        Some("OPENROUTER_API_KEY")
    );
    assert_eq!(
        provider_default_api_key_env("moonshot"),
        Some("MOONSHOT_API_KEY")
    );
    assert_eq!(
        provider_default_api_key_env("anthropic"),
        Some("ANTHROPIC_API_KEY")
    );
    assert_eq!(provider_default_api_key_env("other"), None);
}

#[test]
fn skippable_profile_error_only_matches_model_unavailable_cases() {
    assert!(is_skippable_profile_error(
        "status 400: {\"error\":{\"code\":\"model_not_found\"}}"
    ));
    assert!(is_skippable_profile_error(
        "The requested model 'x' does not exist."
    ));
    assert!(!is_skippable_profile_error(
        "missing API key (set env var `ANTHROPIC_API_KEY`)"
    ));
    assert!(!is_skippable_profile_error("request timed out after 90s"));
}

#[test]
fn retryable_probe_error_matches_timeouts_and_5xx() {
    assert!(is_retryable_probe_error("request timed out after 90s"));
    assert!(is_retryable_probe_error("status 429: {\"error\":\"rate\"}"));
    assert!(is_retryable_probe_error("status 503: upstream unavailable"));
    assert!(!is_retryable_probe_error("status 400: model_not_found"));
}

#[test]
fn align_profile_auth_mode_switches_login_only_models() {
    let mut config = Config::default();
    config.api.model = "gpt-5.3-codex-spark".to_string();
    config.api.auth = AuthMode::ApiKey;
    config.api.api_key = "x".to_string();

    align_profile_auth_mode_with_model_capabilities(&mut config).expect("switch to login");
    assert_eq!(config.api.auth, AuthMode::Login);
    assert!(config.api.api_key.is_empty());
}
