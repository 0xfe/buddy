# Auth Storage

Buddy stores login credentials in `~/.config/buddy/auth.json`.

## Security Model

- Credentials are encrypted at rest.
- A random data-encryption key (DEK) encrypts provider token records.
- The DEK is wrapped by a machine-derived key-encryption key (KEK).
- The KEK is derived with `scrypt` from machine/user host attributes plus a per-store random salt.
- Encryption uses `AES-256-GCM-SIV` with unique random nonces.

This is intended to protect tokens at rest on the same machine (similar threat model to local SSH key files + file permissions), while remaining cross-platform without keychain dependencies.

## Format and Migration

- Current on-disk format is encrypted (`version = 3`).
- Legacy plaintext stores are migrated automatically on first successful load.
- Provider-scoped records are preferred.
- Legacy profile-scoped records are still read for compatibility.

## Operations

- `buddy login --check [profile]`
  - Shows credential health for the provider (saved/not saved, expiry, expiring soon).
- `buddy login --reset [profile]`
  - Removes saved credentials for the provider, then continues login.
- `buddy login [profile]`
  - Shows health and runs device login flow.

## Failure Recovery

If decryption fails (for example, machine identity changes, corruption, or tampering), Buddy returns an actionable error and recommends:

1. `buddy login --reset [profile]`
2. `buddy login [profile]`

## File Permissions

On Unix:

- `~/.config/buddy` is created with mode `0700` (best effort).
- `auth.json` is written with mode `0600` (best effort).
