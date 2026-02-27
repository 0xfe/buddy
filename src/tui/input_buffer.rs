//! Editable input buffer and history helpers.

use crate::tui::prompt::PromptMode;
use std::fs;
use std::io;
use std::path::Path;

const MAX_HISTORY: usize = 1000;

/// Persistent REPL state across input reads.
#[derive(Debug, Clone, Default)]
pub struct ReplState {
    history: Vec<String>,
    normal_draft: Option<InputDraft>,
    approval_draft: Option<InputDraft>,
}

/// Snapshot of an in-progress interactive input line.
///
/// This is used to preserve user typing when the UI temporarily interrupts input
/// to render background task output or approval requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InputDraft {
    pub(crate) buffer: String,
    pub(crate) cursor: usize,
    pub(crate) selected: usize,
    pub(crate) history_index: Option<usize>,
    pub(crate) history_draft: String,
}

impl ReplState {
    /// Add a command to input history.
    pub fn push_history(&mut self, entry: &str) {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return;
        }

        if self.history.last().map(|s| s.as_str()) == Some(entry) {
            return;
        }

        self.history.push(entry.to_string());
        if self.history.len() > MAX_HISTORY {
            let overflow = self.history.len() - MAX_HISTORY;
            self.history.drain(0..overflow);
        }
    }

    /// Load persisted history entries from disk.
    ///
    /// Supports both JSON array (`["cmd1", "cmd2"]`) and legacy line-based
    /// text files. Unknown/empty entries are ignored.
    pub fn load_history_file(&mut self, path: &Path) -> io::Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let raw = fs::read_to_string(path)?;
        self.history.clear();

        if raw.trim().is_empty() {
            return Ok(());
        }

        if let Ok(entries) = serde_json::from_str::<Vec<String>>(&raw) {
            for entry in entries {
                self.push_history(&entry);
            }
            return Ok(());
        }

        // Backward/robust fallback for plain text history files.
        for line in raw.lines() {
            self.push_history(line);
        }
        Ok(())
    }

    /// Persist history entries to disk as a compact JSON array.
    pub fn save_history_file(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let encoded = serde_json::to_string(&self.history).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to encode history: {err}"),
            )
        })?;
        fs::write(path, format!("{encoded}\n"))
    }

    /// Return the saved draft for this prompt mode, if one exists.
    pub(crate) fn take_draft(&mut self, mode: PromptMode) -> InputDraft {
        let draft = match mode {
            PromptMode::Normal => self.normal_draft.take(),
            PromptMode::Approval => self.approval_draft.take(),
        };
        draft.unwrap_or_default().sanitize(self.history.len())
    }

    /// Persist the in-progress draft for this prompt mode.
    pub(crate) fn save_draft(&mut self, mode: PromptMode, draft: InputDraft) {
        let draft = draft.sanitize(self.history.len());
        match mode {
            PromptMode::Normal => self.normal_draft = Some(draft),
            PromptMode::Approval => self.approval_draft = Some(draft),
        }
    }

    /// Clear any saved draft for this prompt mode.
    pub(crate) fn clear_draft(&mut self, mode: PromptMode) {
        match mode {
            PromptMode::Normal => self.normal_draft = None,
            PromptMode::Approval => self.approval_draft = None,
        }
    }

    /// Current command history length.
    pub(crate) fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Expose history for test assertions.
    #[cfg(test)]
    pub(crate) fn history(&self) -> &[String] {
        &self.history
    }
}

impl InputDraft {
    /// Normalize cursor/history indexes against current buffer/history bounds.
    fn sanitize(mut self, history_len: usize) -> Self {
        let char_len = char_count(&self.buffer);
        self.cursor = self.cursor.min(char_len);
        if self.selected > 0 && !self.buffer.starts_with('/') {
            self.selected = 0;
        }
        self.history_index = self.history_index.filter(|idx| *idx < history_len);
        self
    }
}

/// Move the history cursor one entry up and replace the current buffer.
pub(crate) fn history_up(
    state: &ReplState,
    history_index: &mut Option<usize>,
    history_draft: &mut String,
    buffer: &mut String,
) {
    if state.history.is_empty() {
        return;
    }

    match history_index {
        Some(idx) => {
            if *idx > 0 {
                *idx -= 1;
            }
        }
        None => {
            *history_draft = buffer.clone();
            *history_index = Some(state.history.len() - 1);
        }
    }

    if let Some(idx) = history_index {
        *buffer = state.history[*idx].clone();
    }
}

/// Move the history cursor one entry down and replace the current buffer.
pub(crate) fn history_down(
    state: &ReplState,
    history_index: &mut Option<usize>,
    history_draft: &str,
    buffer: &mut String,
) {
    let Some(idx) = history_index else {
        return;
    };

    if *idx + 1 < state.history.len() {
        *idx += 1;
        *buffer = state.history[*idx].clone();
        return;
    }

    *history_index = None;
    *buffer = history_draft.to_string();
}

/// Insert one char at the current cursor position.
pub(crate) fn insert_char_at_cursor(buffer: &mut String, cursor: &mut usize, ch: char) {
    let byte_idx = byte_index_at_char(buffer, *cursor);
    buffer.insert(byte_idx, ch);
    *cursor += 1;
}

/// Delete one char immediately before cursor.
pub(crate) fn delete_char_before_cursor(buffer: &mut String, cursor: &mut usize) {
    let start = byte_index_at_char(buffer, *cursor - 1);
    let end = byte_index_at_char(buffer, *cursor);
    buffer.replace_range(start..end, "");
    *cursor -= 1;
}

/// Delete one char at the current cursor position.
pub(crate) fn delete_char_at_cursor(buffer: &mut String, cursor: usize) {
    let start = byte_index_at_char(buffer, cursor);
    let end = byte_index_at_char(buffer, cursor + 1);
    buffer.replace_range(start..end, "");
}

/// Delete a char range represented in char indices.
pub(crate) fn delete_char_range(buffer: &mut String, start_char: usize, end_char: usize) {
    if start_char >= end_char {
        return;
    }
    let start = byte_index_at_char(buffer, start_char);
    let end = byte_index_at_char(buffer, end_char);
    buffer.replace_range(start..end, "");
}

/// Return the char index where the previous word starts.
pub(crate) fn previous_word_start(buffer: &str, cursor: usize) -> usize {
    let mut idx = cursor;
    while idx > 0 {
        let ch = char_at(buffer, idx - 1);
        if !ch.is_whitespace() {
            break;
        }
        idx -= 1;
    }
    while idx > 0 {
        let ch = char_at(buffer, idx - 1);
        if ch.is_whitespace() {
            break;
        }
        idx -= 1;
    }
    idx
}

/// Return the char index for the start of the current line.
pub(crate) fn line_start_char_index(buffer: &str, cursor: usize) -> usize {
    let mut idx = cursor;
    while idx > 0 {
        if char_at(buffer, idx - 1) == '\n' {
            break;
        }
        idx -= 1;
    }
    idx
}

/// Return the char index for the end of the current line.
pub(crate) fn line_end_char_index(buffer: &str, cursor: usize) -> usize {
    let len = char_count(buffer);
    let mut idx = cursor;
    while idx < len {
        if char_at(buffer, idx) == '\n' {
            break;
        }
        idx += 1;
    }
    idx
}

/// Convert a char index to a byte index, preserving UTF-8 boundaries.
pub(crate) fn byte_index_at_char(s: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }

    let total_chars = s.chars().count();
    if char_idx >= total_chars {
        return s.len();
    }

    s.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

/// Return the char value at index, or NUL when out of range.
pub(crate) fn char_at(s: &str, char_idx: usize) -> char {
    s.chars().nth(char_idx).unwrap_or('\0')
}

/// Return total char count for a UTF-8 string.
pub(crate) fn char_count(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trip() {
        let mut state = ReplState::default();
        state.push_history("one");
        state.push_history("two");
        state.push_history("two");
        assert_eq!(state.history(), &["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn history_persistence_round_trip_json() {
        let temp = std::env::temp_dir().join(format!(
            "buddy-repl-history-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&temp);

        let mut state = ReplState::default();
        state.push_history("one");
        state.push_history("two");
        state.save_history_file(&temp).expect("save history");

        let mut restored = ReplState::default();
        restored.load_history_file(&temp).expect("load history");
        assert_eq!(
            restored.history(),
            &["one".to_string(), "two".to_string()],
            "restored history should match saved entries"
        );

        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn history_loader_supports_line_fallback() {
        let temp = std::env::temp_dir().join(format!(
            "buddy-repl-history-lines-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&temp);
        std::fs::write(&temp, "alpha\nbeta\n").expect("write fallback history");

        let mut restored = ReplState::default();
        restored.load_history_file(&temp).expect("load history");
        assert_eq!(
            restored.history(),
            &["alpha".to_string(), "beta".to_string()],
            "line fallback should parse entries"
        );

        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn line_boundaries_for_multiline_buffer() {
        let buffer = "abc\ndef";
        assert_eq!(line_start_char_index(buffer, 1), 0);
        assert_eq!(line_end_char_index(buffer, 1), 3);
        assert_eq!(line_start_char_index(buffer, 5), 4);
        assert_eq!(line_end_char_index(buffer, 5), 7);
    }

    #[test]
    fn byte_index_respects_utf8_boundaries() {
        let s = "aéz";
        assert_eq!(byte_index_at_char(s, 0), 0);
        assert_eq!(byte_index_at_char(s, 1), 1);
        assert_eq!(byte_index_at_char(s, 2), 3);
        assert_eq!(byte_index_at_char(s, 3), s.len());
    }

    #[test]
    fn drafts_are_saved_per_prompt_mode() {
        let mut state = ReplState::default();
        state.push_history("one");
        state.save_draft(
            PromptMode::Normal,
            InputDraft {
                buffer: "/ps".to_string(),
                cursor: 3,
                selected: 1,
                history_index: Some(0),
                history_draft: String::new(),
            },
        );
        state.save_draft(
            PromptMode::Approval,
            InputDraft {
                buffer: "y".to_string(),
                cursor: 1,
                selected: 0,
                history_index: None,
                history_draft: String::new(),
            },
        );

        let normal = state.take_draft(PromptMode::Normal);
        let approval = state.take_draft(PromptMode::Approval);
        assert_eq!(normal.buffer, "/ps");
        assert_eq!(approval.buffer, "y");
    }

    #[test]
    fn draft_sanitize_clamps_invalid_indexes() {
        let mut state = ReplState::default();
        state.push_history("first");
        state.save_draft(
            PromptMode::Normal,
            InputDraft {
                buffer: "abc".to_string(),
                cursor: 99,
                selected: 4,
                history_index: Some(9),
                history_draft: "tmp".to_string(),
            },
        );
        let draft = state.take_draft(PromptMode::Normal);
        assert_eq!(draft.cursor, 3);
        assert_eq!(draft.selected, 0);
        assert_eq!(draft.history_index, None);
    }
}
