//! File loading and incremental tailing helpers for trace JSONL streams.

use crate::traceui::event::TraceEvent;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::SystemTime;

/// Incremental JSONL reader that tolerates appends and partial trailing lines.
#[derive(Debug)]
pub struct TraceEventSource {
    /// File path being tailed.
    path: PathBuf,
    /// Byte offset consumed so far.
    offset: u64,
    /// Buffered partial line waiting for newline termination.
    pending_fragment: String,
    /// Next synthetic line number assigned to parsed records.
    next_line_no: usize,
    /// Last observed file modification time.
    last_modified: Option<SystemTime>,
}

impl TraceEventSource {
    /// Create a new event source for one file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            pending_fragment: String::new(),
            next_line_no: 1,
            last_modified: None,
        }
    }

    /// Read the full file from the beginning.
    pub fn load_all(&mut self) -> Result<Vec<TraceEvent>, String> {
        self.offset = 0;
        self.pending_fragment.clear();
        self.next_line_no = 1;
        self.read_new()
    }

    /// Read only newly appended complete lines.
    pub fn read_new(&mut self) -> Result<Vec<TraceEvent>, String> {
        let path = self.path.clone();
        let mut file = File::open(&path)
            .map_err(|err| format!("failed to open trace file {}: {err}", path.display()))?;
        let metadata = file
            .metadata()
            .map_err(|err| format!("failed to stat trace file {}: {err}", path.display()))?;
        let len = metadata.len();
        let modified = metadata.modified().ok();

        // If the file shrank or rotated, restart from the beginning.
        if len < self.offset
            || (len == self.offset
                && modified.is_some()
                && self.last_modified.is_some()
                && modified != self.last_modified)
        {
            self.offset = 0;
            self.pending_fragment.clear();
            self.next_line_no = 1;
        }

        file.seek(SeekFrom::Start(self.offset))
            .map_err(|err| format!("failed to seek trace file {}: {err}", path.display()))?;

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|err| format!("failed to read trace file {}: {err}", path.display()))?;
        self.offset = self.offset.saturating_add(buf.len() as u64);
        self.last_modified = modified;
        Ok(self.decode_bytes(&buf))
    }

    fn decode_bytes(&mut self, buf: &[u8]) -> Vec<TraceEvent> {
        if buf.is_empty() {
            return Vec::new();
        }

        let chunk = String::from_utf8_lossy(buf);
        let mut combined = String::new();
        combined.push_str(&self.pending_fragment);
        combined.push_str(&chunk);
        self.pending_fragment.clear();

        let mut events = Vec::new();
        let ends_with_newline = combined.ends_with('\n');
        let mut lines = combined.split('\n').peekable();
        while let Some(line) = lines.next() {
            if !ends_with_newline && lines.peek().is_none() {
                self.pending_fragment = line.to_string();
                break;
            }
            let trimmed = line.trim_end_matches('\r');
            if trimmed.trim().is_empty() {
                continue;
            }
            let event = TraceEvent::from_line(self.next_line_no, trimmed);
            self.next_line_no += 1;
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::TraceEventSource;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("buddy-traceui-{unique}.jsonl"))
    }

    #[test]
    fn reads_initial_file_and_incremental_appends() {
        let path = temp_path();
        std::fs::write(
            &path,
            "{\"seq\":1,\"event\":{\"type\":\"Lifecycle\",\"payload\":\"runtime_started\"}}\n",
        )
        .unwrap();

        let mut source = TraceEventSource::new(&path);
        let first = source.load_all().unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].seq, Some(1));

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{{\"seq\":2,\"event\":{{\"type\":\"Lifecycle\",\"payload\":\"runtime_stopped\"}}}}"
        )
        .unwrap();
        drop(file);

        let second = source.read_new().unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].seq, Some(2));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn holds_partial_trailing_line_until_completed() {
        let path = temp_path();
        std::fs::write(
            &path,
            "{\"seq\":1,\"event\":{\"type\":\"Lifecycle\",\"payload\":\"runtime_started\"}}",
        )
        .unwrap();

        let mut source = TraceEventSource::new(&path);
        let initial = source.load_all().unwrap();
        assert!(initial.is_empty());

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file).unwrap();
        drop(file);

        let completed = source.read_new().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].seq, Some(1));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resets_when_file_is_truncated() {
        let path = temp_path();
        std::fs::write(
            &path,
            "{\"seq\":1,\"event\":{\"type\":\"Lifecycle\",\"payload\":\"runtime_started\"}}\n",
        )
        .unwrap();

        let mut source = TraceEventSource::new(&path);
        assert_eq!(source.load_all().unwrap().len(), 1);
        std::fs::write(
            &path,
            "{\"seq\":9,\"event\":{\"type\":\"Lifecycle\",\"payload\":\"runtime_stopped\"}}\n",
        )
        .unwrap();

        let events = source.read_new().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, Some(9));

        let _ = std::fs::remove_file(path);
    }
}
