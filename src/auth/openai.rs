//! OpenAI device flow and token refresh helpers.

use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

use super::error::AuthError;
use super::types::{unix_now_secs, OAuthTokens, OpenAiDeviceLogin};

/// OpenAI Accounts API base used by the device login flow.
const OPENAI_ACCOUNTS_API_BASE: &str = "https://auth.openai.com/api/accounts";
/// OAuth token endpoint used for exchange and refresh.
const OPENAI_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// User-facing verification page for device login.
const OPENAI_DEVICE_LOGIN_URL: &str = "https://auth.openai.com/codex/device";
/// Redirect URI registered for the CLI device flow.
const OPENAI_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// OpenAI public client id used by this login flow.
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Maximum allowed wait while polling for device authorization.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Shared HTTP timeout for auth requests.
const AUTH_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Response payload returned when creating a device login session.
#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    /// Opaque device session id used for polling.
    device_auth_id: String,
    /// Short code shown to the user.
    #[serde(alias = "usercode")]
    user_code: String,
    /// Polling interval hint returned by the provider.
    #[serde(deserialize_with = "deserialize_interval", default)]
    interval: u64,
}

/// Payload returned once device login is approved.
#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    /// Authorization code for final token exchange.
    authorization_code: String,
    /// PKCE verifier bound to the authorization code.
    code_verifier: String,
}

/// OAuth token endpoint response shape.
#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    /// Newly issued access token.
    access_token: Option<String>,
    /// Newly issued refresh token (may be omitted on refresh).
    refresh_token: Option<String>,
    /// Access token lifetime in seconds.
    #[serde(deserialize_with = "deserialize_i64_option", default)]
    expires_in: Option<i64>,
}

/// Begin the OpenAI device-code login flow.
pub async fn start_openai_device_login() -> Result<OpenAiDeviceLogin, AuthError> {
    let client = shared_auth_http_client();
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
    let client = shared_auth_http_client();
    // Poll until the user approves device login, then exchange code for tokens.
    let code = poll_openai_device_code(client, login).await?;
    exchange_openai_code(client, &code.authorization_code, &code.code_verifier, None).await
}

/// Refresh an OpenAI login token.
pub async fn refresh_openai_tokens(current: &OAuthTokens) -> Result<OAuthTokens, AuthError> {
    refresh_openai_tokens_with_client(shared_auth_http_client(), current).await
}

/// Refresh an OpenAI login token using the provided HTTP client.
pub async fn refresh_openai_tokens_with_client(
    client: &reqwest::Client,
    current: &OAuthTokens,
) -> Result<OAuthTokens, AuthError> {
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
    // Validate required fields before persisting refreshed credentials.
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

/// Poll OpenAI device login endpoint until authorization code is issued.
async fn poll_openai_device_code(
    client: &reqwest::Client,
    login: &OpenAiDeviceLogin,
) -> Result<DeviceTokenResponse, AuthError> {
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
            // 403/404 indicate pending authorization during device flow.
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

/// Exchange an authorization code for OAuth access/refresh tokens.
async fn exchange_openai_code(
    client: &reqwest::Client,
    authorization_code: &str,
    code_verifier: &str,
    refresh_fallback: Option<&str>,
) -> Result<OAuthTokens, AuthError> {
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
    // `refresh_fallback` keeps existing refresh token on partial responses.
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

/// Lazily initialized shared HTTP client for auth requests.
fn shared_auth_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(AUTH_HTTP_TIMEOUT)
            .user_agent("buddy/0.1")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Deserialize polling interval values provided as string/number/null.
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

/// Deserialize optional integer durations encoded as string/number/null.
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
