//! Persistent auth token store helpers.

use crate::config::config_root_dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::crypto::{decrypt_store, encrypt_store, looks_encrypted_store};
use super::error::AuthError;
use super::provider::OPENAI_PROVIDER_KEY;
use super::types::{OAuthTokens, ProviderLoginHealth};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AuthStore {
    /// Schema version for on-disk auth records.
    #[serde(default)]
    pub(crate) version: u32,
    /// Provider-scoped token map (`providers.<name>`).
    #[serde(default)]
    pub(crate) providers: BTreeMap<String, OAuthTokens>,
    // Legacy profile-scoped token storage from older buddy builds.
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, OAuthTokens>,
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

/// True when the auth store still contains legacy profile-scoped records.
///
/// Buddy now stores login credentials under `providers.<name>`. Legacy
/// `profiles.<name>` entries still work for compatibility, but should be
/// migrated by re-running `buddy login`.
pub fn has_legacy_profile_token_records() -> Result<bool, AuthError> {
    let Some(path) = default_auth_store_path() else {
        return Ok(false);
    };
    let store = load_store(&path)?;
    Ok(!store.profiles.is_empty())
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

/// Inspect stored credentials for a provider without modifying them.
pub fn provider_login_health(provider: &str) -> Result<ProviderLoginHealth, AuthError> {
    let Some(path) = default_auth_store_path() else {
        return Ok(ProviderLoginHealth {
            provider: provider.to_string(),
            has_tokens: false,
            expiring_soon: false,
            expires_at_unix: None,
        });
    };
    let store = load_store(&path)?;
    let tokens = resolve_provider_tokens(&store, provider);
    Ok(ProviderLoginHealth {
        provider: provider.to_string(),
        has_tokens: tokens.is_some(),
        expiring_soon: tokens.as_ref().is_some_and(OAuthTokens::is_expiring_soon),
        expires_at_unix: tokens.map(|value| value.expires_at_unix),
    })
}

/// Remove saved credentials for a provider.
///
/// Returns `true` when credentials were removed.
pub fn reset_provider_tokens(provider: &str) -> Result<bool, AuthError> {
    let Some(path) = default_auth_store_path() else {
        return Ok(false);
    };
    let mut store = load_store(&path)?;
    let mut removed = store.providers.remove(provider).is_some();
    if provider == OPENAI_PROVIDER_KEY {
        for legacy_key in ["openai", "gpt-codex", "gpt-spark"] {
            removed |= store.profiles.remove(legacy_key).is_some();
        }
    }
    if removed {
        write_store(&path, &store)?;
    }
    Ok(removed)
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

/// Resolve provider tokens with compatibility fallback to legacy profile records.
pub(crate) fn resolve_provider_tokens(store: &AuthStore, provider: &str) -> Option<OAuthTokens> {
    if let Some(tokens) = store.providers.get(provider) {
        return Some(tokens.clone());
    }
    resolve_legacy_profile_tokens(store, provider)
}

/// Resolve legacy profile-scoped tokens for providers that predate provider keys.
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

/// Load and decode the auth store from disk, including plaintext migration.
pub(crate) fn load_store(path: &Path) -> Result<AuthStore, AuthError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let value: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
                AuthError::Invalid(format!(
                    "failed to parse auth store `{}`: {err}",
                    path.display()
                ))
            })?;

            if looks_encrypted_store(&value) {
                let encrypted: super::crypto::EncryptedAuthStore = serde_json::from_value(value)
                    .map_err(|err| {
                        AuthError::Invalid(format!(
                            "failed to parse encrypted auth store `{}`: {err}",
                            path.display()
                        ))
                    })?;
                return decrypt_store(&encrypted);
            }

            // Legacy plaintext format migration path.
            let parsed: AuthStore = serde_json::from_value(value).map_err(|err| {
                AuthError::Invalid(format!(
                    "failed to parse auth store `{}`: {err}",
                    path.display()
                ))
            })?;
            if !parsed.providers.is_empty() || !parsed.profiles.is_empty() {
                // Best-effort migration. If re-write fails, keep loading plaintext.
                let _ = write_store(path, &parsed);
            }
            Ok(parsed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AuthStore::default()),
        Err(err) => Err(AuthError::Io(err)),
    }
}

/// Encrypt and persist the auth store to disk with restrictive permissions.
pub(crate) fn write_store(path: &Path, store: &AuthStore) -> Result<(), AuthError> {
    if let Some(parent) = path.parent() {
        // Ensure config directory exists and is private.
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    // Always write encrypted credentials, even when loaded from legacy plaintext.
    let encrypted = encrypt_store(store)?;
    let text = serde_json::to_string_pretty(&encrypted).map_err(|err| {
        AuthError::Invalid(format!("failed to serialize encrypted auth store: {err}"))
    })?;
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
        // Re-assert secure file permissions after write.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
