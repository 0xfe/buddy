//! Persistent conversation sessions stored under `.buddyx/` by default.
//!
//! A session snapshot stores message history + token tracker state so a REPL
//! can resume context without rehydrating from the provider.

use crate::agent::AgentSessionSnapshot;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Subdirectory under each session root that contains per-session JSON files.
const SESSIONS_DIR: &str = "sessions";
/// Canonical file extension for persisted sessions.
const SESSION_FILE_EXT: &str = "json";
/// On-disk schema version for [`PersistedSession`].
const SESSION_FILE_VERSION: u32 = 1;
/// Preferred modern session root.
const DEFAULT_SESSION_ROOT: &str = ".buddyx";
/// Legacy session root retained for backward compatibility.
const LEGACY_SESSION_ROOT: &str = ".agentx";

/// True when default-open logic will resolve to the legacy `.agentx` root.
///
/// This is used to surface a one-time migration warning in the CLI startup
/// path without changing storage behavior.
pub fn default_uses_legacy_root() -> bool {
    !Path::new(DEFAULT_SESSION_ROOT).exists() && Path::new(LEGACY_SESSION_ROOT).exists()
}

/// Lightweight listing metadata shown by `/session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// Stable session identifier.
    pub id: String,
    /// Last save timestamp in Unix epoch milliseconds.
    pub updated_at_millis: u64,
}

/// Filesystem-backed storage for named REPL sessions.
#[derive(Debug, Clone)]
pub struct SessionStore {
    /// Directory containing `*.json` session files.
    sessions_dir: PathBuf,
}

/// On-disk payload shape for persisted sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSession {
    /// File-format version for forward compatibility checks.
    version: u32,
    /// Stable session identifier (legacy alias keeps older `name` payloads loadable).
    #[serde(alias = "name")]
    id: String,
    /// Last update timestamp used for "recent sessions" ordering.
    updated_at_millis: u64,
    /// Serialized conversation state snapshot.
    state: AgentSessionSnapshot,
}

impl SessionStore {
    /// Open/create the default local session directory (`.buddyx/sessions`).
    ///
    /// If the legacy `.agentx/` root exists and `.buddyx/` does not, reuse the
    /// legacy location to preserve existing sessions.
    pub fn open_default() -> Result<Self, String> {
        if Path::new(DEFAULT_SESSION_ROOT).exists() {
            return Self::open(DEFAULT_SESSION_ROOT);
        }
        if default_uses_legacy_root() {
            return Self::open(LEGACY_SESSION_ROOT);
        }
        Self::open(DEFAULT_SESSION_ROOT)
    }

    /// Open/create a session store rooted under the given directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        let sessions_dir = root.as_ref().join(SESSIONS_DIR);
        fs::create_dir_all(&sessions_dir).map_err(|e| {
            format!(
                "failed to create session directory {}: {e}",
                sessions_dir.display()
            )
        })?;
        Ok(Self { sessions_dir })
    }

    /// Create and persist a new unique session ID for `state`.
    pub fn create_new_session(&self, state: &AgentSessionSnapshot) -> Result<String, String> {
        for _ in 0..64 {
            let session_id = generate_session_id();
            let path = self.session_path(&session_id);
            if path.exists() {
                continue;
            }
            self.save(&session_id, state)?;
            return Ok(session_id);
        }
        Err("failed to allocate a unique session id".to_string())
    }

    /// Save snapshot state under a stable session ID.
    pub fn save(&self, session_id: &str, state: &AgentSessionSnapshot) -> Result<(), String> {
        validate_session_id(session_id)?;
        let payload = PersistedSession {
            version: SESSION_FILE_VERSION,
            id: session_id.to_string(),
            updated_at_millis: now_unix_millis(),
            state: state.clone(),
        };
        let json = serde_json::to_vec_pretty(&payload)
            .map_err(|e| format!("failed to serialize session {session_id}: {e}"))?;
        let path = self.session_path(session_id);
        // Write to a sibling temporary file first so partial writes do not
        // corrupt the last known-good session snapshot.
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, json).map_err(|e| {
            format!(
                "failed to write temporary session file {}: {e}",
                tmp_path.display()
            )
        })?;
        // Rename is atomic on most filesystems, making this "all or nothing".
        fs::rename(&tmp_path, &path).map_err(|e| {
            format!(
                "failed to move session file into place {}: {e}",
                path.display()
            )
        })?;
        Ok(())
    }

    /// Load a saved session snapshot from disk.
    pub fn load(&self, session_id: &str) -> Result<AgentSessionSnapshot, String> {
        validate_session_id(session_id)?;
        let path = self.session_path(session_id);
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read session {}: {e}", path.display()))?;
        let payload: PersistedSession = serde_json::from_str(&raw)
            .map_err(|e| format!("failed to parse session {}: {e}", path.display()))?;
        if payload.version != SESSION_FILE_VERSION {
            return Err(format!(
                "unsupported session file version {} for {}",
                payload.version,
                path.display()
            ));
        }
        Ok(payload.state)
    }

    /// Return all sessions ordered by most recent use.
    pub fn list(&self) -> Result<Vec<SessionSummary>, String> {
        let mut sessions = Vec::new();
        // Best-effort listing: unreadable or malformed files are skipped so one
        // bad file does not break session browsing.
        for entry in
            fs::read_dir(&self.sessions_dir).map_err(|e| format!("failed to list sessions: {e}"))?
        {
            let entry = entry.map_err(|e| format!("failed to read session entry: {e}"))?;
            let path = entry.path();
            if !is_session_file(&path) {
                continue;
            }

            let raw = match fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            let payload: PersistedSession = match serde_json::from_str(&raw) {
                Ok(payload) => payload,
                Err(_) => continue,
            };
            sessions.push(SessionSummary {
                id: payload.id,
                updated_at_millis: payload.updated_at_millis,
            });
        }

        // Most recently updated first; ID tiebreak keeps output deterministic.
        sessions.sort_by(|a, b| {
            b.updated_at_millis
                .cmp(&a.updated_at_millis)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(sessions)
    }

    /// Resolve `"last"` to the most recently used session.
    pub fn resolve_last(&self) -> Result<Option<String>, String> {
        Ok(self.list()?.into_iter().next().map(|s| s.id))
    }

    /// Build the on-disk path for a session identifier.
    fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir
            .join(format!("{session_id}.{SESSION_FILE_EXT}"))
    }
}

/// Validate user/model provided session IDs before touching the filesystem.
fn validate_session_id(session_id: &str) -> Result<(), String> {
    let trimmed = session_id.trim();
    if trimmed.is_empty() {
        return Err("session id cannot be empty".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("session id cannot be '.' or '..'".to_string());
    }
    if trimmed
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
    {
        return Err(
            "session id can only contain ASCII letters, numbers, '.', '-', '_'".to_string(),
        );
    }
    Ok(())
}

/// Return true when the path has the canonical session extension.
fn is_session_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(SESSION_FILE_EXT)
}

/// Current Unix timestamp in milliseconds.
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Generate a unique-ish hex session id (`xxxx-xxxx-xxxx-xxxx`).
pub fn generate_session_id() -> String {
    let mut bytes = [0u8; 8];
    // OS RNG is sufficient for low-collision opaque IDs.
    OsRng.fill_bytes(&mut bytes);
    let hex = format!("{:016x}", u64::from_be_bytes(bytes));
    format!(
        "{}-{}-{}-{}",
        &hex[0..4],
        &hex[4..8],
        &hex[8..12],
        &hex[12..16]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::TokenTrackerSnapshot;
    use crate::types::Message;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// Per-process counter to avoid temp-dir name collisions in fast test runs.
    static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(1);

    /// Build a minimal session snapshot fixture.
    fn test_snapshot() -> AgentSessionSnapshot {
        AgentSessionSnapshot {
            messages: vec![Message::user("hello")],
            tracker: TokenTrackerSnapshot {
                context_limit: 8192,
                total_prompt_tokens: 12,
                total_completion_tokens: 34,
                last_prompt_tokens: 12,
                last_completion_tokens: 34,
            },
        }
    }

    /// Build an isolated temporary session store for each test.
    fn test_store() -> SessionStore {
        let unique = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("buddy-session-test-{}-{unique}", now_unix_millis()));
        SessionStore::open(root).expect("temp store should open")
    }

    // Ensures persisted snapshots round-trip through disk serialization.
    #[test]
    fn save_and_load_round_trip() {
        let store = test_store();
        let snapshot = test_snapshot();
        store.save("demo", &snapshot).expect("save should succeed");
        let loaded = store.load("demo").expect("load should succeed");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content.as_deref(), Some("hello"));
        assert_eq!(loaded.tracker.last_completion_tokens, 34);
    }

    // Ensures listing order prefers most recently written sessions.
    #[test]
    fn list_orders_by_last_update() {
        let store = test_store();
        store.save("a", &test_snapshot()).expect("save a");
        std::thread::sleep(Duration::from_millis(15));
        store.save("b", &test_snapshot()).expect("save b");

        let sessions = store.list().expect("list should succeed");
        assert!(!sessions.is_empty());
        assert_eq!(sessions[0].id, "b");
    }

    // Ensures `resolve_last` mirrors list ordering semantics.
    #[test]
    fn resolve_last_returns_latest_session() {
        let store = test_store();
        store.save("first", &test_snapshot()).expect("save first");
        std::thread::sleep(Duration::from_millis(15));
        store.save("second", &test_snapshot()).expect("save second");
        let last = store.resolve_last().expect("resolve should succeed");
        assert_eq!(last.as_deref(), Some("second"));
    }

    // Ensures invalid identifiers are rejected before filesystem writes.
    #[test]
    fn invalid_session_id_is_rejected() {
        let store = test_store();
        let err = store
            .save("bad/name", &test_snapshot())
            .expect_err("must fail");
        assert!(err.contains("session id"));
    }

    // Ensures generated IDs use the documented grouped-hex shape.
    #[test]
    fn generate_session_id_is_hex_groups() {
        let id = generate_session_id();
        let parts = id.split('-').collect::<Vec<_>>();
        assert_eq!(parts.len(), 4);
        assert!(parts.iter().all(|part| part.len() == 4));
        assert!(parts
            .iter()
            .all(|part| part.chars().all(|ch| ch.is_ascii_hexdigit())));
    }

    // Ensures ID allocation retries until a distinct on-disk file path is found.
    #[test]
    fn create_new_session_allocates_distinct_ids() {
        let store = test_store();
        let snapshot = test_snapshot();
        let first = store.create_new_session(&snapshot).expect("create first");
        let second = store.create_new_session(&snapshot).expect("create second");
        assert_ne!(first, second);
    }
}
