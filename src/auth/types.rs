//! Public auth model types.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const REFRESH_SAFETY_WINDOW_SECS: i64 = 90;

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
    pub(crate) device_auth_id: String,
    pub(crate) interval_secs: u64,
}

/// Health summary for stored provider credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLoginHealth {
    pub provider: String,
    pub has_tokens: bool,
    pub expiring_soon: bool,
    pub expires_at_unix: Option<i64>,
}

pub(crate) fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
