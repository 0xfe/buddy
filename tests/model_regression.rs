//! Live model regression probes.
//!
//! This suite is intentionally `#[ignore]` and is never run by default.
//! It validates that the default model profiles from `src/templates/buddy.toml`
//! can execute a tiny prompt end-to-end with current auth semantics.
//!
//! Run explicitly:
//! `cargo test --test model_regression -- --ignored --nocapture`

use buddy::api::ApiClient;
use buddy::auth::{load_provider_tokens, login_provider_key_for_base_url};
use buddy::config::{load_config, select_model_profile, AuthMode, Config};
use buddy::types::{ChatRequest, Message};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

const TEMPLATE_BUDDY_TOML: &str = include_str!("../src/templates/buddy.toml");

#[tokio::test]
#[ignore = "network regression suite; run explicitly"]
async fn default_template_profiles_round_trip() {
    assert_no_global_runtime_overrides().expect("clean runtime env required");

    let path = write_temp_template_config().expect("temp config");
    let mut config = load_config(Some(path.to_string_lossy().as_ref())).expect("load config");
    let profile_names = configured_profile_names(&config);

    let mut failures = Vec::<String>::new();
    for profile in &profile_names {
        eprintln!("[model-regression] profile={profile}");
        match run_profile_probe(&mut config, profile).await {
            Ok(()) => eprintln!("[model-regression] profile={profile} ok"),
            Err(err) => failures.push(format!("{profile}: {err}")),
        }
    }

    let _ = fs::remove_file(&path);

    if !failures.is_empty() {
        panic!(
            "model regression failures:\n{}",
            failures
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

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

async fn run_profile_probe(config: &mut Config, profile_name: &str) -> Result<(), String> {
    select_model_profile(config, profile_name)
        .map_err(|err| format!("failed selecting profile: {err}"))?;
    profile_auth_preflight(config, profile_name)?;

    let client = ApiClient::new(&config.api);
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

    let response = timeout(Duration::from_secs(90), client.chat(&request))
        .await
        .map_err(|_| "request timed out after 90s".to_string())
        .and_then(|result| result.map_err(|err| err.to_string()))?;

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

    let text = choice.message.content.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(format!(
            "empty assistant content (finish_reason={:?})",
            choice.finish_reason
        ));
    }

    Ok(())
}

fn profile_auth_preflight(config: &Config, profile_name: &str) -> Result<(), String> {
    let profile = config
        .models
        .get(profile_name)
        .ok_or_else(|| format!("profile `{profile_name}` not found in config"))?;

    if config.api.auth == AuthMode::ApiKey {
        if config.api.api_key.trim().is_empty() {
            let source_hint = if let Some(env_name) = profile.api_key_env.as_deref() {
                format!("set env var `{env_name}`")
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
        let provider = login_provider_key_for_base_url(&config.api.base_url).ok_or_else(|| {
            format!(
                "login auth profile `{profile_name}` has unsupported base URL `{}`",
                config.api.base_url
            )
        })?;

        let tokens = load_provider_tokens(provider)
            .map_err(|err| format!("failed loading saved login for `{provider}`: {err}"))?;

        if tokens.is_none() {
            return Err(format!(
                "missing saved login for provider `{provider}`; run `buddy login {profile_name}` first"
            ));
        }
    }

    Ok(())
}

fn configured_profile_names(config: &Config) -> Vec<String> {
    config.models.keys().cloned().collect()
}

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
