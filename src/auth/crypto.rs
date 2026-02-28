//! Machine-derived encryption-at-rest for auth token storage.

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use scrypt::{scrypt, Params as ScryptParams};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::error::AuthError;
use super::store::AuthStore;
use super::types::OAuthTokens;

/// On-disk auth store version used for encrypted credential records.
pub(crate) const AUTH_STORE_VERSION_ENCRYPTED: u32 = 3;
/// Random salt bytes used for machine-key derivation.
const AUTH_STORE_SALT_LEN: usize = 16;
/// AEAD nonce bytes for wrapped DEK and token record encryption.
const AUTH_STORE_NONCE_LEN: usize = 12;
/// Symmetric key length for AES-256.
const AUTH_STORE_KEY_LEN: usize = 32;
/// Domain-separation label mixed into machine-derived key material.
const AUTH_MACHINE_KEY_CONTEXT: &str = "buddy-auth-machine-kek-v1";

/// Serialized encrypted auth store payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EncryptedAuthStore {
    /// Encrypted store schema version.
    #[serde(default)]
    pub(crate) version: u32,
    /// Envelope containing wrapped DEK metadata.
    #[serde(default)]
    pub(crate) encryption: EncryptedAuthEnvelope,
    /// Provider-scoped encrypted token records.
    #[serde(default)]
    pub(crate) providers: BTreeMap<String, EncryptedTokenRecord>,
    /// Legacy profile-scoped encrypted token records.
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, EncryptedTokenRecord>,
}

/// Envelope holding DEK wrapping parameters and ciphertext.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EncryptedAuthEnvelope {
    /// Base64-encoded KDF salt.
    #[serde(default)]
    pub(crate) salt: String,
    /// Base64-encoded nonce used when wrapping the DEK.
    #[serde(default)]
    pub(crate) wrapped_dek_nonce: String,
    /// Base64-encoded wrapped DEK ciphertext.
    #[serde(default)]
    pub(crate) wrapped_dek_ciphertext: String,
}

/// Encrypted OAuth token record stored under provider/profile keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EncryptedTokenRecord {
    /// Base64-encoded nonce used for this token record.
    #[serde(default)]
    pub(crate) nonce: String,
    /// Base64-encoded encrypted token payload.
    #[serde(default)]
    pub(crate) ciphertext: String,
}

/// Detect whether a JSON value appears to be an encrypted auth store payload.
pub(crate) fn looks_encrypted_store(value: &serde_json::Value) -> bool {
    value
        .get("encryption")
        .and_then(|inner| inner.as_object())
        .is_some()
}

/// Encrypt an in-memory auth store for secure persistence.
pub(crate) fn encrypt_store(store: &AuthStore) -> Result<EncryptedAuthStore, AuthError> {
    // Derive a machine-bound KEK and use it to wrap a random DEK.
    let mut salt = [0u8; AUTH_STORE_SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let kek = derive_machine_kek(&salt)?;

    let mut dek = [0u8; AUTH_STORE_KEY_LEN];
    rand::thread_rng().fill_bytes(&mut dek);
    let (wrapped_dek_nonce, wrapped_dek_ciphertext) = encrypt_blob(&kek, &dek)?;

    let mut providers = BTreeMap::new();
    for (provider, tokens) in &store.providers {
        // Encrypt each provider token record with the DEK.
        let record = encrypt_token_record(&dek, tokens)?;
        providers.insert(provider.clone(), record);
    }

    let mut profiles = BTreeMap::new();
    for (profile, tokens) in &store.profiles {
        // Encrypt legacy profile token records for compatibility reads.
        let record = encrypt_token_record(&dek, tokens)?;
        profiles.insert(profile.clone(), record);
    }

    Ok(EncryptedAuthStore {
        version: AUTH_STORE_VERSION_ENCRYPTED,
        encryption: EncryptedAuthEnvelope {
            salt: B64.encode(salt),
            wrapped_dek_nonce: B64.encode(wrapped_dek_nonce),
            wrapped_dek_ciphertext: B64.encode(wrapped_dek_ciphertext),
        },
        providers,
        profiles,
    })
}

/// Decrypt an encrypted auth store payload into plaintext in-memory records.
pub(crate) fn decrypt_store(store: &EncryptedAuthStore) -> Result<AuthStore, AuthError> {
    let salt = decode_fixed::<AUTH_STORE_SALT_LEN>(&store.encryption.salt, "salt")?;
    let kek = derive_machine_kek(&salt)?;
    let wrapped_nonce = decode_fixed::<AUTH_STORE_NONCE_LEN>(
        &store.encryption.wrapped_dek_nonce,
        "wrapped_dek_nonce",
    )?;
    let wrapped_dek = decode_bytes(
        &store.encryption.wrapped_dek_ciphertext,
        "wrapped_dek_ciphertext",
    )?;
    // Decrypt the DEK first, then decrypt each record payload with that key.
    let dek_raw = decrypt_blob(&kek, &wrapped_nonce, &wrapped_dek).map_err(|_| {
        AuthError::Invalid(
            "failed to decrypt auth credentials (machine identity may have changed). Run `buddy login --reset` and login again."
                .to_string(),
        )
    })?;
    if dek_raw.len() != AUTH_STORE_KEY_LEN {
        return Err(AuthError::Invalid(
            "invalid encrypted auth key material in auth store".to_string(),
        ));
    }
    let mut dek = [0u8; AUTH_STORE_KEY_LEN];
    dek.copy_from_slice(&dek_raw);

    let mut providers = BTreeMap::new();
    for (provider, record) in &store.providers {
        let tokens = decrypt_token_record(&dek, record).map_err(|_| {
            AuthError::Invalid(format!(
                "failed to decrypt auth credentials for provider `{provider}`. Run `buddy login --reset` and login again."
            ))
        })?;
        providers.insert(provider.clone(), tokens);
    }

    let mut profiles = BTreeMap::new();
    for (profile, record) in &store.profiles {
        let tokens = decrypt_token_record(&dek, record).map_err(|_| {
            AuthError::Invalid(format!(
                "failed to decrypt legacy auth credentials for profile `{profile}`. Run `buddy login --reset` and login again."
            ))
        })?;
        profiles.insert(profile.clone(), tokens);
    }

    Ok(AuthStore {
        version: store.version.max(AUTH_STORE_VERSION_ENCRYPTED),
        providers,
        profiles,
    })
}

/// Encrypt one OAuth token record using the provided DEK.
fn encrypt_token_record(
    key: &[u8; AUTH_STORE_KEY_LEN],
    tokens: &OAuthTokens,
) -> Result<EncryptedTokenRecord, AuthError> {
    let payload = serde_json::to_vec(tokens)
        .map_err(|err| AuthError::Invalid(format!("failed to serialize oauth tokens: {err}")))?;
    let (nonce, ciphertext) = encrypt_blob(key, &payload)?;
    Ok(EncryptedTokenRecord {
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    })
}

/// Decrypt one OAuth token record using the provided DEK.
fn decrypt_token_record(
    key: &[u8; AUTH_STORE_KEY_LEN],
    record: &EncryptedTokenRecord,
) -> Result<OAuthTokens, AuthError> {
    let nonce = decode_fixed::<AUTH_STORE_NONCE_LEN>(&record.nonce, "nonce")?;
    let ciphertext = decode_bytes(&record.ciphertext, "ciphertext")?;
    let payload = decrypt_blob(key, &nonce, &ciphertext)
        .map_err(|_| AuthError::Invalid("failed to decrypt token record".to_string()))?;
    serde_json::from_slice(&payload).map_err(|err| {
        AuthError::Invalid(format!("failed to decode decrypted token record: {err}"))
    })
}

/// Derive a machine-bound key-encryption key (KEK) from host/user material.
fn derive_machine_kek(
    salt: &[u8; AUTH_STORE_SALT_LEN],
) -> Result<[u8; AUTH_STORE_KEY_LEN], AuthError> {
    let mut material = machine_secret_material()?;
    material.extend_from_slice(salt);

    let mut hashed = Sha256::new();
    hashed.update(AUTH_MACHINE_KEY_CONTEXT.as_bytes());
    hashed.update(&material);
    let seed = hashed.finalize();

    let params = ScryptParams::recommended();
    let mut key = [0u8; AUTH_STORE_KEY_LEN];
    scrypt(&seed, salt, &params, &mut key)
        .map_err(|err| AuthError::Invalid(format!("failed to derive machine auth key: {err}")))?;
    Ok(key)
}

/// Build a best-effort machine identity string used for key derivation.
fn machine_secret_material() -> Result<Vec<u8>, AuthError> {
    let hostname = hostname::get()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown-host".to_string());
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string());
    let home = dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let machine_id = read_machine_id().unwrap_or_default();
    let joined = format!(
        "os={}|host={}|user={}|home={}|machine_id={}",
        std::env::consts::OS,
        hostname,
        username,
        home,
        machine_id
    );
    Ok(joined.into_bytes())
}

/// Read a platform machine identifier from common Unix locations.
fn read_machine_id() -> Option<String> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id", "/etc/hostid"] {
        if let Ok(value) = std::fs::read_to_string(path) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Encrypt an arbitrary byte payload with AES-256-GCM-SIV.
fn encrypt_blob(
    key: &[u8; AUTH_STORE_KEY_LEN],
    plaintext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    let cipher = Aes256GcmSiv::new_from_slice(key)
        .map_err(|_| AuthError::Invalid("invalid encryption key length".to_string()))?;
    let mut nonce = [0u8; AUTH_STORE_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| AuthError::Invalid("failed to encrypt auth data".to_string()))?;
    Ok((nonce.to_vec(), ciphertext))
}

/// Decrypt an arbitrary byte payload with AES-256-GCM-SIV.
fn decrypt_blob(
    key: &[u8; AUTH_STORE_KEY_LEN],
    nonce: &[u8; AUTH_STORE_NONCE_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>, AuthError> {
    let cipher = Aes256GcmSiv::new_from_slice(key)
        .map_err(|_| AuthError::Invalid("invalid encryption key length".to_string()))?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| AuthError::Invalid("failed to decrypt auth data".to_string()))
}

/// Decode base64 text for one auth-store field.
fn decode_bytes(value: &str, field: &str) -> Result<Vec<u8>, AuthError> {
    B64.decode(value).map_err(|err| {
        AuthError::Invalid(format!(
            "failed to decode auth store field `{field}`: {err}"
        ))
    })
}

/// Decode base64 text and enforce an exact byte length for one field.
fn decode_fixed<const N: usize>(value: &str, field: &str) -> Result<[u8; N], AuthError> {
    let bytes = decode_bytes(value, field)?;
    if bytes.len() != N {
        return Err(AuthError::Invalid(format!(
            "invalid auth store field `{field}` length: expected {N}, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}
