//! Runtime-event trace file helpers.
//!
//! This module implements best-effort JSONL tracing for runtime envelopes so
//! operators can inspect and replay model/tool interactions after the fact.

use crate::cli::Args;
use buddy::runtime::RuntimeEventEnvelope;
use serde_json::Value;
use std::env;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Environment variable used to set the runtime trace file path.
pub(crate) const TRACE_ENV_VAR: &str = "BUDDY_TRACE_FILE";

/// Resolve optional trace file path from CLI flag and environment.
///
/// Precedence: CLI `--trace` > `BUDDY_TRACE_FILE`.
pub(crate) fn resolve_trace_path(args: &Args) -> Option<PathBuf> {
    resolve_trace_path_with(args.trace.as_deref(), |key| env::var_os(key))
}

/// Test seam for trace-path resolution with injected environment lookup.
fn resolve_trace_path_with<F>(cli_trace: Option<&str>, env_lookup: F) -> Option<PathBuf>
where
    F: FnOnce(&str) -> Option<std::ffi::OsString>,
{
    let cli = cli_trace.and_then(|value| non_empty_path(Some(value)));
    if cli.is_some() {
        return cli;
    }
    env_lookup(TRACE_ENV_VAR).and_then(|value| non_empty_path(value.to_str()))
}

/// Normalize optional string into a non-empty path.
fn non_empty_path(value: Option<&str>) -> Option<PathBuf> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Best-effort JSONL writer for runtime trace envelopes.
///
/// On first write failure, the writer disables itself and returns a warning so
/// callers can continue normal operation without repeated noise.
pub(crate) struct RuntimeTraceWriter {
    /// Output path for operator diagnostics.
    path: PathBuf,
    /// Buffered file writer for JSONL line output.
    writer: BufWriter<File>,
    /// Last sequence number written (used to guard duplicate writes).
    last_seq: Option<u64>,
    /// Whether writing has been permanently disabled after an error.
    disabled: bool,
}

impl RuntimeTraceWriter {
    /// Open or create the trace file in append mode.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                create_dir_all(parent).map_err(|err| {
                    format!(
                        "failed to create trace directory {}: {err}",
                        parent.display()
                    )
                })?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|err| format!("failed to open trace file {}: {err}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: BufWriter::new(file),
            last_seq: None,
            disabled: false,
        })
    }

    /// Append one runtime envelope as a JSON line.
    ///
    /// Returns a warning message once when tracing is disabled due to I/O
    /// errors. Duplicate envelope sequence ids are ignored.
    pub(crate) fn write_envelope(&mut self, envelope: &RuntimeEventEnvelope) -> Option<String> {
        if self.disabled {
            return None;
        }
        if self.last_seq.is_some_and(|last| envelope.seq <= last) {
            return None;
        }
        if let Err(err) = self.write_line(envelope) {
            self.disabled = true;
            return Some(format!(
                "trace disabled after write failure to {}: {err}",
                self.path.display()
            ));
        }
        self.last_seq = Some(envelope.seq);
        None
    }

    /// Serialize one envelope and flush the newline-delimited record.
    fn write_line(&mut self, envelope: &RuntimeEventEnvelope) -> Result<(), String> {
        let mut redacted = serde_json::to_value(envelope)
            .map_err(|err| format!("failed to serialize runtime trace envelope: {err}"))?;
        redact_json_value(None, &mut redacted);
        serde_json::to_writer(&mut self.writer, &redacted)
            .map_err(|err| format!("failed to encode redacted runtime trace envelope: {err}"))?;
        self.writer
            .write_all(b"\n")
            .map_err(|err| format!("failed to append runtime trace newline: {err}"))?;
        self.writer
            .flush()
            .map_err(|err| format!("failed to flush runtime trace file: {err}"))?;
        Ok(())
    }
}

/// Redact sensitive fields and token-like strings recursively.
fn redact_json_value(parent_key: Option<&str>, value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map.iter_mut() {
                redact_json_value(Some(key.as_str()), nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(parent_key, item);
            }
        }
        Value::String(text) => {
            if is_sensitive_key(parent_key) {
                *text = "[REDACTED]".to_string();
            } else {
                *text = redact_secret_like_text(text);
            }
        }
        _ => {}
    }
}

/// Return true when a JSON key strongly implies a secret value.
fn is_sensitive_key(parent_key: Option<&str>) -> bool {
    let Some(key) = parent_key else {
        return false;
    };
    let lowered = key.to_ascii_lowercase();
    lowered.contains("api_key")
        || lowered.contains("password")
        || lowered.contains("secret")
        || lowered.contains("access_token")
        || lowered.contains("refresh_token")
}

/// Scrub obvious secret-like token patterns from free-form text.
fn redact_secret_like_text(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    if contains_secret_marker(input) {
        return "[REDACTED]".to_string();
    }
    input.to_string()
}

/// Heuristic matcher for obvious secret/token markers.
fn contains_secret_marker(input: &str) -> bool {
    let lowered = input.to_ascii_lowercase();
    if lowered.contains("bearer ")
        || lowered.contains("-----begin")
        || lowered.contains("openai_api_key")
        || lowered.contains("authorization:")
    {
        return true;
    }
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i + 3 < bytes.len() {
        if bytes[i] == b's' && bytes[i + 1] == b'k' && bytes[i + 2] == b'-' {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j.saturating_sub(i + 3) >= 16 {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::runtime::{LifecycleEvent, RuntimeEvent, RuntimeEventEnvelope};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Verifies CLI path takes precedence over env trace path.
    #[test]
    fn resolve_trace_prefers_cli() {
        let path = resolve_trace_path_with(Some("  /tmp/a.jsonl "), |_| {
            Some(std::ffi::OsString::from("/tmp/b.jsonl"))
        });
        assert_eq!(path, Some(PathBuf::from("/tmp/a.jsonl")));
    }

    // Verifies env trace path is used when CLI does not provide one.
    #[test]
    fn resolve_trace_falls_back_to_env() {
        let path =
            resolve_trace_path_with(None, |_| Some(std::ffi::OsString::from("/tmp/b.jsonl")));
        assert_eq!(path, Some(PathBuf::from("/tmp/b.jsonl")));
    }

    // Verifies writer appends JSONL envelopes and skips duplicate seq writes.
    #[test]
    fn runtime_trace_writer_writes_jsonl_lines() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("buddy-trace-{unique}.jsonl"));
        let mut writer = RuntimeTraceWriter::open(&path).expect("open trace writer");

        let first =
            RuntimeEventEnvelope::new(1, RuntimeEvent::Lifecycle(LifecycleEvent::RuntimeStarted));
        let second =
            RuntimeEventEnvelope::new(2, RuntimeEvent::Lifecycle(LifecycleEvent::RuntimeStopped));
        assert!(writer.write_envelope(&first).is_none());
        assert!(writer.write_envelope(&first).is_none());
        assert!(writer.write_envelope(&second).is_none());

        let text = std::fs::read_to_string(&path).expect("read trace output");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"seq\":1"));
        assert!(lines[1].contains("\"seq\":2"));

        let _ = std::fs::remove_file(path);
    }

    // Verifies sensitive key names and token markers are redacted.
    #[test]
    fn redact_json_masks_sensitive_values() {
        let mut value = serde_json::json!({
            "api_key": "sk-abcdefghijklmnopqrstuvwxyz",
            "nested": {
                "note": "Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz"
            },
            "safe": "hello"
        });
        redact_json_value(None, &mut value);

        assert_eq!(value["api_key"], Value::String("[REDACTED]".to_string()));
        assert_eq!(
            value["nested"]["note"],
            Value::String("[REDACTED]".to_string())
        );
        assert_eq!(value["safe"], Value::String("hello".to_string()));
    }
}
