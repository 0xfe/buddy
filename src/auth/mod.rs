//! Login auth helpers and secure token storage.
//!
//! This module implements OpenAI device-code login, token refresh,
//! and local credential persistence under `~/.config/buddy/auth.json`.

mod browser;
mod crypto;
mod error;
mod openai;
mod provider;
mod store;
mod types;

pub use browser::try_open_browser;
pub use error::AuthError;
pub use openai::{
    complete_openai_device_login, refresh_openai_tokens, refresh_openai_tokens_with_client,
    start_openai_device_login,
};
pub use provider::{
    login_provider_key_for_base_url, openai_login_runtime_base_url, supports_openai_login,
};
pub use store::{
    default_auth_store_path, has_legacy_profile_token_records, load_profile_tokens,
    load_provider_tokens, provider_login_health, reset_provider_tokens, save_profile_tokens,
    save_provider_tokens,
};
pub use types::{OAuthTokens, OpenAiDeviceLogin, ProviderLoginHealth};

#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use store::AuthStore;

/// Test-only visibility shim for provider token resolution behavior.
#[cfg(test)]
fn resolve_provider_tokens(store: &AuthStore, provider: &str) -> Option<OAuthTokens> {
    store::resolve_provider_tokens(store, provider)
}

/// Test-only loader shim for auth store fixtures.
#[cfg(test)]
fn load_store(path: &Path) -> Result<AuthStore, AuthError> {
    store::load_store(path)
}

/// Test-only writer shim for auth store fixtures.
#[cfg(test)]
fn write_store(path: &Path, store: &AuthStore) -> Result<(), AuthError> {
    store::write_store(path, store)
}

/// Test-only shim for deterministic token expiry assertions.
#[cfg(test)]
fn unix_now_secs() -> i64 {
    types::unix_now_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Monotonic id source used to avoid temp-path collisions in tests.
    static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(1);

    /// Build an isolated temp auth-store path for one test case.
    fn temp_auth_store_path() -> PathBuf {
        let mut root = std::env::temp_dir();
        let id = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        root.push(format!("buddy-auth-test-{id}-{now}"));
        let _ = std::fs::create_dir_all(&root);
        root.join("auth.json")
    }

    // Verifies OpenAI login host detection accepts known OpenAI endpoints only.
    #[test]
    fn openai_login_support_detection_matches_known_hosts() {
        assert!(supports_openai_login("https://api.openai.com/v1"));
        assert!(supports_openai_login(
            "https://chatgpt.com/backend-api/codex"
        ));
        assert!(!supports_openai_login("https://openrouter.ai/api/v1"));
    }

    // Verifies OpenAI API host rewrites to the ChatGPT Codex runtime endpoint.
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

    // Verifies expiry guard marks near-expiry credentials for refresh.
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

    // Verifies provider-key mapping for known OpenAI and non-OpenAI URLs.
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

    // Verifies provider-scoped records override legacy profile-scoped fallbacks.
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

    // Verifies legacy profile records are still readable for OpenAI providers.
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

    // Verifies non-OpenAI providers can still reuse very old legacy token stores.
    #[test]
    fn resolve_provider_tokens_falls_back_to_first_legacy_profile_for_unknown_provider() {
        let legacy_tokens = OAuthTokens {
            access_token: "legacy-access".into(),
            refresh_token: "legacy-refresh".into(),
            expires_at_unix: unix_now_secs() + 3600,
        };
        let mut store = AuthStore::default();
        store
            .profiles
            .insert("some-legacy-profile".to_string(), legacy_tokens.clone());

        let resolved = resolve_provider_tokens(&store, "non-openai-provider").expect("tokens");
        assert_eq!(resolved, legacy_tokens);
    }

    // Verifies encrypted writes keep token plaintext out of the persisted file.
    #[test]
    fn write_store_encrypts_tokens_on_disk() {
        let path = temp_auth_store_path();
        let tokens = OAuthTokens {
            access_token: "access-plain-text".into(),
            refresh_token: "refresh-plain-text".into(),
            expires_at_unix: unix_now_secs() + 3600,
        };
        let mut store = AuthStore::default();
        store.providers.insert("openai".to_string(), tokens.clone());

        write_store(&path, &store).expect("write encrypted store");
        let raw = std::fs::read_to_string(&path).expect("read encrypted file");
        assert!(raw.contains("\"encryption\""), "raw: {raw}");
        assert!(
            !raw.contains("access-plain-text"),
            "token leaked in encrypted auth file"
        );

        let loaded = load_store(&path).expect("load encrypted store");
        assert_eq!(loaded.providers.get("openai"), Some(&tokens));
    }

    // Verifies legacy plaintext stores are migrated to encrypted format on load.
    #[test]
    fn load_store_migrates_plaintext_store_to_encrypted_format() {
        let path = temp_auth_store_path();
        let mut store = AuthStore::default();
        store.providers.insert(
            "openai".to_string(),
            OAuthTokens {
                access_token: "legacy-access".into(),
                refresh_token: "legacy-refresh".into(),
                expires_at_unix: unix_now_secs() + 3600,
            },
        );
        let plaintext = serde_json::to_string_pretty(&store).expect("serialize plaintext");
        std::fs::write(&path, plaintext).expect("write plaintext fixture");

        let loaded = load_store(&path).expect("load + migrate plaintext");
        assert_eq!(
            loaded
                .providers
                .get("openai")
                .map(|value| value.access_token.as_str()),
            Some("legacy-access")
        );

        let migrated = std::fs::read_to_string(&path).expect("read migrated auth store");
        assert!(migrated.contains("\"encryption\""), "raw: {migrated}");
        assert!(
            !migrated.contains("legacy-access"),
            "plaintext token remained after migration"
        );
    }

    // Verifies decryption errors surface when encrypted payloads are tampered.
    #[test]
    fn load_store_reports_tampered_encrypted_payload() {
        let path = temp_auth_store_path();
        let mut store = AuthStore::default();
        store.providers.insert(
            "openai".to_string(),
            OAuthTokens {
                access_token: "token-a".into(),
                refresh_token: "token-b".into(),
                expires_at_unix: unix_now_secs() + 3600,
            },
        );
        write_store(&path, &store).expect("write encrypted store");

        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read encrypted"))
                .expect("parse encrypted json");
        let ciphertext = value["providers"]["openai"]["ciphertext"]
            .as_str()
            .expect("ciphertext")
            .to_string();
        value["providers"]["openai"]["ciphertext"] =
            serde_json::Value::String(format!("{ciphertext}AA"));
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&value).expect("serialize tampered"),
        )
        .expect("write tampered");

        let err = load_store(&path).expect_err("tampered payload should fail");
        assert!(err.to_string().contains("failed to decrypt"));
    }

    // Verifies missing auth-store files resolve to an empty default state.
    #[test]
    fn load_store_missing_path_returns_default_store() {
        let path = temp_auth_store_path();
        let loaded = load_store(&path).expect("missing file should load default store");
        assert!(loaded.providers.is_empty());
        assert!(loaded.profiles.is_empty());
    }
}
