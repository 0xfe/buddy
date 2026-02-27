//! Shared test fixtures for parser/auth/execution test modules.
//!
//! The remediation track adds many new tests across modules. Keeping tiny but
//! reusable helpers here prevents each test module from rebuilding ad-hoc temp
//! dir and SSE fixture code.

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Temporary directory fixture with best-effort cleanup.
///
/// This helper is intentionally simple and std-only so unit tests can use it
/// without introducing new dependencies.
#[derive(Debug)]
pub struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    /// Create a unique temporary directory with a readable prefix.
    pub fn new(prefix: &str) -> Self {
        let suffix = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let dir = std::env::temp_dir().join(format!("buddy-{prefix}-{millis}-{suffix}"));
        fs::create_dir_all(&dir).expect("failed to create temporary fixture directory");
        Self { path: dir }
    }

    /// Root directory path for this fixture.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Build a child path under the fixture root.
    pub fn child(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    /// Write UTF-8 text to a child path, creating parent directories as needed.
    pub fn write_text(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.child(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directories for fixture");
        }
        fs::write(&path, content).expect("failed to write fixture file");
        path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Build one SSE event block with `event:` and `data:` lines.
pub fn sse_event_block(event: &str, data: &str) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

/// SSE stream terminator block used by OpenAI-compatible streams.
pub fn sse_done_block() -> &'static str {
    "data: [DONE]\n\n"
}

/// Build a serialized auth store fixture with one provider-scoped token.
pub fn auth_store_json_fixture(
    provider: &str,
    access_token: &str,
    refresh_token: &str,
    expires_at_unix: i64,
) -> String {
    json!({
        "version": 2,
        "providers": {
            provider: {
                "access_token": access_token,
                "refresh_token": refresh_token,
                "expires_at_unix": expires_at_unix
            }
        },
        "profiles": {}
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_fixture_writes_and_resolves_paths() {
        let fixture = TestTempDir::new("fixture");
        let file = fixture.write_text("nested/file.txt", "hello");
        assert_eq!(fs::read_to_string(file).unwrap(), "hello");
    }

    #[test]
    fn sse_helpers_emit_expected_wire_format() {
        let block = sse_event_block("response.completed", r#"{"id":"resp_1"}"#);
        assert!(block.starts_with("event: response.completed\n"));
        assert!(block.ends_with("\n\n"));
        assert_eq!(sse_done_block(), "data: [DONE]\n\n");
    }

    #[test]
    fn auth_store_fixture_contains_provider_record() {
        let raw = auth_store_json_fixture("openai", "a", "r", 123);
        assert!(raw.contains("\"providers\""));
        assert!(raw.contains("\"openai\""));
    }
}
