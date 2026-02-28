//! Public auth model types.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds of headroom before expiry when tokens should be refreshed.
const REFRESH_SAFETY_WINDOW_SECS: i64 = 90;

/// Stored OAuth tokens used for `auth = "login"` model profiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthTokens {
    /// Bearer token used for authenticated API requests.
    pub access_token: String,
    /// OAuth refresh token used to mint new access tokens.
    pub refresh_token: String,
    /// Absolute Unix timestamp when the access token expires.
    pub expires_at_unix: i64,
}

impl OAuthTokens {
    /// True when this token should be refreshed before making new requests.
    pub fn is_expiring_soon(&self) -> bool {
        unix_now_secs().saturating_add(REFRESH_SAFETY_WINDOW_SECS) >= self.expires_at_unix
    }
}

/// Device-code login session details presented to the user.
#[derive(Debug, Clone)]
pub struct OpenAiDeviceLogin {
    /// URL the user should open to complete device authorization.
    pub verification_url: String,
    /// User-facing short code to enter on the verification page.
    pub user_code: String,
    /// Opaque device auth session id used during polling.
    pub(crate) device_auth_id: String,
    /// Provider-suggested polling interval in seconds.
    pub(crate) interval_secs: u64,
}

/// Health summary for stored provider credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLoginHealth {
    /// Provider key used to query health (for example, `openai`).
    pub provider: String,
    /// True when any stored token record exists for this provider.
    pub has_tokens: bool,
    /// True when loaded tokens are near expiry and should be refreshed soon.
    pub expiring_soon: bool,
    /// Loaded token expiry timestamp, if token data exists.
    pub expires_at_unix: Option<i64>,
}

/// Current Unix time in seconds.
pub(crate) fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
