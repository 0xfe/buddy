//! Login auth helpers and secure token storage.
//!
//! This module currently implements OpenAI device-code login, token refresh,
//! and local credential persistence under `~/.config/buddy/auth.json`.

use crate::config::config_root_dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OPENAI_ACCOUNTS_API_BASE: &str = "https://auth.openai.com/api/accounts";
const OPENAI_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_DEVICE_LOGIN_URL: &str = "https://auth.openai.com/codex/device";
const OPENAI_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const OPENAI_CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_PROVIDER_KEY: &str = "openai";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const REFRESH_SAFETY_WINDOW_SECS: i64 = 90;

/// Errors surfaced by the login/auth subsystem.
#[derive(Debug)]
pub enum AuthError {
    Io(std::io::Error),
    Http(reqwest::Error),
    Status(u16, String),
    Invalid(String),
    Unsupported(String),
    LoginExpired,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io: {err}"),
            Self::Http(err) => write!(f, "http: {err}"),
            Self::Status(code, body) => write!(f, "status {code}: {body}"),
            Self::Invalid(msg) => write!(f, "{msg}"),
            Self::Unsupported(msg) => write!(f, "{msg}"),
            Self::LoginExpired => {
                write!(
                    f,
                    "saved login has expired or was revoked; run `buddy login` again"
                )
            }
        }
    }
}

impl std::error::Error for AuthError {}

impl From<std::io::Error> for AuthError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<reqwest::Error> for AuthError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

/// Stored OAuth tokens used for `auth = "login"` model profiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_unix: i64,
}

impl OAuthTokens {
    pub fn is_expiring_soon(&self) -> bool {
        unix_now_secs().saturating_add(REFRESH_SAFETY_WINDOW_SECS) >= self.expires_at_unix
    }
}

/// Device-code login session details presented to the user.
#[derive(Debug, Clone)]
pub struct OpenAiDeviceLogin {
    pub verification_url: String,
    pub user_code: String,
    device_auth_id: String,
    interval_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AuthStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, OAuthTokens>,
    // Legacy profile-scoped token storage from older buddy builds.
    #[serde(default)]
    profiles: BTreeMap<String, OAuthTokens>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(deserialize_with = "deserialize_interval", default)]
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[serde(deserialize_with = "deserialize_i64_option", default)]
    expires_in: Option<i64>,
}

/// Returns true when the model base URL appears to target OpenAI.
pub fn supports_openai_login(base_url: &str) -> bool {
    let normalized = base_url.trim().to_ascii_lowercase();
    normalized.contains("api.openai.com") || normalized.contains("chatgpt.com/backend-api/codex")
}

/// Resolve login provider key for a configured base URL.
pub fn login_provider_key_for_base_url(base_url: &str) -> Option<&'static str> {
    if supports_openai_login(base_url) {
        Some(OPENAI_PROVIDER_KEY)
    } else {
        None
    }
}

/// Resolve the runtime request base URL for OpenAI login auth.
///
/// OpenAI login tokens are accepted by ChatGPT Codex backend endpoints.
pub fn openai_login_runtime_base_url(base_url: &str) -> String {
    let normalized = base_url.trim().to_ascii_lowercase();
    if normalized.contains("api.openai.com") {
        OPENAI_CHATGPT_CODEX_BASE_URL.to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    }
}

/// Returns the default auth file path (`~/.config/buddy/auth.json`) when available.
pub fn default_auth_store_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("auth.json"))
}

/// Load saved tokens for a provider.
///
/// Prefers provider-scoped storage and falls back to legacy profile-scoped
/// records so existing users are not forced to re-login after upgrades.
pub fn load_provider_tokens(provider: &str) -> Result<Option<OAuthTokens>, AuthError> {
    let Some(path) = default_auth_store_path() else {
        return Ok(None);
    };
    let store = load_store(&path)?;
    Ok(resolve_provider_tokens(&store, provider))
}

/// Save tokens for a provider.
pub fn save_provider_tokens(provider: &str, tokens: OAuthTokens) -> Result<(), AuthError> {
    let Some(path) = default_auth_store_path() else {
        return Err(AuthError::Invalid(
            "unable to resolve config root for auth token storage".to_string(),
        ));
    };
    let mut store = load_store(&path)?;
    store.version = 2;
    store.providers.insert(provider.to_string(), tokens);
    write_store(&path, &store)?;
    Ok(())
}

/// Legacy compatibility shim for older integrations that still call the
/// profile-scoped API. Login tokens are now provider-scoped.
pub fn load_profile_tokens(_profile: &str) -> Result<Option<OAuthTokens>, AuthError> {
    load_provider_tokens(OPENAI_PROVIDER_KEY)
}

/// Legacy compatibility shim for older integrations that still call the
/// profile-scoped API. Login tokens are now provider-scoped.
pub fn save_profile_tokens(_profile: &str, tokens: OAuthTokens) -> Result<(), AuthError> {
    save_provider_tokens(OPENAI_PROVIDER_KEY, tokens)
}

fn resolve_provider_tokens(store: &AuthStore, provider: &str) -> Option<OAuthTokens> {
    if let Some(tokens) = store.providers.get(provider) {
        return Some(tokens.clone());
    }
    resolve_legacy_profile_tokens(store, provider)
}

fn resolve_legacy_profile_tokens(store: &AuthStore, provider: &str) -> Option<OAuthTokens> {
    if store.profiles.is_empty() {
        return None;
    }

    if provider == OPENAI_PROVIDER_KEY {
        // Common legacy profile names for OpenAI-backed configs.
        for key in ["openai", "gpt-codex", "gpt-spark"] {
            if let Some(tokens) = store.profiles.get(key) {
                return Some(tokens.clone());
            }
        }
    }

    // Fallback for very old stores: login auth was OpenAI-only, so any saved
    // profile token can be treated as OpenAI provider credentials.
    store.profiles.values().next().cloned()
}

/// Begin the OpenAI device-code login flow.
pub async fn start_openai_device_login() -> Result<OpenAiDeviceLogin, AuthError> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{OPENAI_ACCOUNTS_API_BASE}/deviceauth/usercode"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "client_id": OPENAI_CLIENT_ID }))
        .send()
        .await?;

    if !response.status().is_success() {
        let code = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(AuthError::Status(code, body));
    }

    let payload: DeviceCodeResponse = response.json().await?;
    let interval_secs = payload.interval.max(1);
    Ok(OpenAiDeviceLogin {
        verification_url: OPENAI_DEVICE_LOGIN_URL.to_string(),
        user_code: payload.user_code,
        device_auth_id: payload.device_auth_id,
        interval_secs,
    })
}

/// Complete device-code login by polling for authorization and exchanging it for tokens.
pub async fn complete_openai_device_login(
    login: &OpenAiDeviceLogin,
) -> Result<OAuthTokens, AuthError> {
    let code = poll_openai_device_code(login).await?;
    exchange_openai_code(&code.authorization_code, &code.code_verifier, None).await
}

/// Refresh an OpenAI login token.
pub async fn refresh_openai_tokens(current: &OAuthTokens) -> Result<OAuthTokens, AuthError> {
    let client = reqwest::Client::new();
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", current.refresh_token.as_str()),
        ("client_id", OPENAI_CLIENT_ID),
        ("scope", "openid profile email"),
    ];

    let response = client
        .post(OPENAI_OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await?;

    if response.status().as_u16() == 401 {
        return Err(AuthError::LoginExpired);
    }
    if !response.status().is_success() {
        let code = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(AuthError::Status(code, body));
    }

    let payload: OAuthTokenResponse = response.json().await?;
    let access_token = payload.access_token.unwrap_or_default().trim().to_string();
    if access_token.is_empty() {
        return Err(AuthError::Invalid(
            "token refresh response did not include access_token".to_string(),
        ));
    }
    let refresh_token = payload
        .refresh_token
        .unwrap_or_else(|| current.refresh_token.clone())
        .trim()
        .to_string();
    if refresh_token.is_empty() {
        return Err(AuthError::Invalid(
            "token refresh response did not include refresh_token".to_string(),
        ));
    }

    let expires_in = payload.expires_in.unwrap_or(3600).max(60);
    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at_unix: unix_now_secs().saturating_add(expires_in),
    })
}

/// Best-effort browser opener used by `/login` and `buddy login`.
pub fn try_open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open")
            .arg(url)
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .is_ok_and(|status| status.success());
    }
    #[allow(unreachable_code)]
    false
}

async fn poll_openai_device_code(
    login: &OpenAiDeviceLogin,
) -> Result<DeviceTokenResponse, AuthError> {
    let client = reqwest::Client::new();
    let started = std::time::Instant::now();
    let poll_interval = Duration::from_secs(login.interval_secs.max(1));

    loop {
        let response = client
            .post(format!("{OPENAI_ACCOUNTS_API_BASE}/deviceauth/token"))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "device_auth_id": login.device_auth_id,
                "user_code": login.user_code,
            }))
            .send()
            .await?;

        if response.status().is_success() {
            let payload: DeviceTokenResponse = response.json().await?;
            return Ok(payload);
        }

        let status = response.status().as_u16();
        if status == 403 || status == 404 {
            if started.elapsed() >= LOGIN_TIMEOUT {
                return Err(AuthError::Invalid(
                    "device login timed out after 15 minutes".to_string(),
                ));
            }
            tokio::time::sleep(poll_interval).await;
            continue;
        }

        let body = response.text().await.unwrap_or_default();
        return Err(AuthError::Status(status, body));
    }
}

async fn exchange_openai_code(
    authorization_code: &str,
    code_verifier: &str,
    refresh_fallback: Option<&str>,
) -> Result<OAuthTokens, AuthError> {
    let client = reqwest::Client::new();
    let form = [
        ("grant_type", "authorization_code"),
        ("code", authorization_code),
        ("redirect_uri", OPENAI_DEVICE_REDIRECT_URI),
        ("client_id", OPENAI_CLIENT_ID),
        ("code_verifier", code_verifier),
        ("scope", "openid profile email offline_access"),
    ];

    let response = client
        .post(OPENAI_OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await?;

    if !response.status().is_success() {
        let code = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(AuthError::Status(code, body));
    }

    let payload: OAuthTokenResponse = response.json().await?;
    let access_token = payload.access_token.unwrap_or_default().trim().to_string();
    if access_token.is_empty() {
        return Err(AuthError::Invalid(
            "token exchange response did not include access_token".to_string(),
        ));
    }

    let refresh_token = payload
        .refresh_token
        .or_else(|| refresh_fallback.map(str::to_string))
        .unwrap_or_default()
        .trim()
        .to_string();
    if refresh_token.is_empty() {
        return Err(AuthError::Invalid(
            "token exchange response did not include refresh_token".to_string(),
        ));
    }

    let expires_in = payload.expires_in.unwrap_or(3600).max(60);
    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at_unix: unix_now_secs().saturating_add(expires_in),
    })
}

fn load_store(path: &Path) -> Result<AuthStore, AuthError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let parsed = serde_json::from_str::<AuthStore>(&text).map_err(|err| {
                AuthError::Invalid(format!(
                    "failed to parse auth store `{}`: {err}",
                    path.display()
                ))
            })?;
            Ok(parsed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AuthStore::default()),
        Err(err) => Err(AuthError::Io(err)),
    }
}

fn write_store(path: &Path, store: &AuthStore) -> Result<(), AuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let text = serde_json::to_string_pretty(store)
        .map_err(|err| AuthError::Invalid(format!("failed to serialize auth store: {err}")))?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(num) => num
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("interval number must be positive")),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|err| serde::de::Error::custom(format!("invalid interval: {err}"))),
        serde_json::Value::Null => Ok(5),
        _ => Err(serde::de::Error::custom(
            "interval must be string or number",
        )),
    }
}

fn deserialize_i64_option<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(num) => num
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("expires_in must be an integer"))
            .map(Some),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|err| serde::de::Error::custom(format!("invalid expires_in: {err}"))),
        _ => Err(serde::de::Error::custom(
            "expires_in must be string, number, or null",
        )),
    }
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_login_support_detection_matches_known_hosts() {
        assert!(supports_openai_login("https://api.openai.com/v1"));
        assert!(supports_openai_login(
            "https://chatgpt.com/backend-api/codex"
        ));
        assert!(!supports_openai_login("https://openrouter.ai/api/v1"));
    }

    #[test]
    fn login_runtime_base_url_rewrites_openai_api_host() {
        assert_eq!(
            openai_login_runtime_base_url("https://api.openai.com/v1"),
            "https://chatgpt.com/backend-api/codex"
        );
        assert_eq!(
            openai_login_runtime_base_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/codex"
        );
    }

    #[test]
    fn token_expiry_guard_triggers_near_expiration() {
        let now = unix_now_secs();
        let almost_expired = OAuthTokens {
            access_token: "a".into(),
            refresh_token: "b".into(),
            expires_at_unix: now + 30,
        };
        assert!(almost_expired.is_expiring_soon());
        let healthy = OAuthTokens {
            access_token: "a".into(),
            refresh_token: "b".into(),
            expires_at_unix: now + 600,
        };
        assert!(!healthy.is_expiring_soon());
    }

    #[test]
    fn provider_key_resolution_matches_openai_hosts() {
        assert_eq!(
            login_provider_key_for_base_url("https://api.openai.com/v1"),
            Some("openai")
        );
        assert_eq!(
            login_provider_key_for_base_url("https://chatgpt.com/backend-api/codex"),
            Some("openai")
        );
        assert_eq!(
            login_provider_key_for_base_url("https://openrouter.ai/api/v1"),
            None
        );
    }

    #[test]
    fn resolve_provider_tokens_prefers_provider_scoped_records() {
        let provider_tokens = OAuthTokens {
            access_token: "provider-access".into(),
            refresh_token: "provider-refresh".into(),
            expires_at_unix: unix_now_secs() + 3600,
        };
        let legacy_tokens = OAuthTokens {
            access_token: "legacy-access".into(),
            refresh_token: "legacy-refresh".into(),
            expires_at_unix: unix_now_secs() + 3600,
        };
        let mut store = AuthStore::default();
        store
            .providers
            .insert("openai".to_string(), provider_tokens.clone());
        store
            .profiles
            .insert("gpt-codex".to_string(), legacy_tokens);

        let resolved = resolve_provider_tokens(&store, "openai").expect("tokens");
        assert_eq!(resolved, provider_tokens);
    }

    #[test]
    fn resolve_provider_tokens_falls_back_to_legacy_profile_records() {
        let legacy_tokens = OAuthTokens {
            access_token: "legacy-access".into(),
            refresh_token: "legacy-refresh".into(),
            expires_at_unix: unix_now_secs() + 3600,
        };
        let mut store = AuthStore::default();
        store
            .profiles
            .insert("gpt-codex".to_string(), legacy_tokens.clone());

        let resolved = resolve_provider_tokens(&store, "openai").expect("tokens");
        assert_eq!(resolved, legacy_tokens);
    }
}
