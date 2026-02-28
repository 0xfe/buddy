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

pub(crate) const AUTH_STORE_VERSION_ENCRYPTED: u32 = 3;
const AUTH_STORE_SALT_LEN: usize = 16;
const AUTH_STORE_NONCE_LEN: usize = 12;
const AUTH_STORE_KEY_LEN: usize = 32;
const AUTH_MACHINE_KEY_CONTEXT: &str = "buddy-auth-machine-kek-v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EncryptedAuthStore {
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) encryption: EncryptedAuthEnvelope,
    #[serde(default)]
    pub(crate) providers: BTreeMap<String, EncryptedTokenRecord>,
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, EncryptedTokenRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EncryptedAuthEnvelope {
    #[serde(default)]
    pub(crate) salt: String,
    #[serde(default)]
    pub(crate) wrapped_dek_nonce: String,
    #[serde(default)]
    pub(crate) wrapped_dek_ciphertext: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EncryptedTokenRecord {
    #[serde(default)]
    pub(crate) nonce: String,
    #[serde(default)]
    pub(crate) ciphertext: String,
}

pub(crate) fn looks_encrypted_store(value: &serde_json::Value) -> bool {
    value
        .get("encryption")
        .and_then(|inner| inner.as_object())
        .is_some()
}

pub(crate) fn encrypt_store(store: &AuthStore) -> Result<EncryptedAuthStore, AuthError> {
    let mut salt = [0u8; AUTH_STORE_SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let kek = derive_machine_kek(&salt)?;

    let mut dek = [0u8; AUTH_STORE_KEY_LEN];
    rand::thread_rng().fill_bytes(&mut dek);
    let (wrapped_dek_nonce, wrapped_dek_ciphertext) = encrypt_blob(&kek, &dek)?;

    let mut providers = BTreeMap::new();
    for (provider, tokens) in &store.providers {
        let record = encrypt_token_record(&dek, tokens)?;
        providers.insert(provider.clone(), record);
    }

    let mut profiles = BTreeMap::new();
    for (profile, tokens) in &store.profiles {
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

fn decode_bytes(value: &str, field: &str) -> Result<Vec<u8>, AuthError> {
    B64.decode(value).map_err(|err| {
        AuthError::Invalid(format!(
            "failed to decode auth store field `{field}`: {err}"
        ))
    })
}

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
