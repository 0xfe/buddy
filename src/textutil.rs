//! Shared UTF-8-safe truncation helpers.
//!
//! Several modules truncate text for previews and tool output limits. Using
//! byte slicing directly can panic when the cut falls inside a multi-byte
//! character. These helpers centralize safe truncation behavior.

/// Return a UTF-8-safe prefix whose byte length is at most `max_bytes`.
pub fn safe_prefix_by_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Truncate by bytes and append `suffix` when truncation occurs.
pub fn truncate_with_suffix_by_bytes(text: &str, max_bytes: usize, suffix: &str) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let prefix = safe_prefix_by_bytes(text, max_bytes);
    format!("{prefix}{suffix}")
}

/// Truncate by characters and append `suffix` when truncation occurs.
pub fn truncate_with_suffix_by_chars(text: &str, max_chars: usize, suffix: &str) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let prefix: String = text.chars().take(max_chars).collect();
    format!("{prefix}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_prefix_by_bytes_keeps_full_ascii_when_short() {
        assert_eq!(safe_prefix_by_bytes("hello", 10), "hello");
    }

    #[test]
    fn safe_prefix_by_bytes_avoids_mid_codepoint_cut() {
        let s = "aé🙂";
        assert_eq!(safe_prefix_by_bytes(s, 2), "a");
        assert_eq!(safe_prefix_by_bytes(s, 3), "aé");
    }

    #[test]
    fn truncate_with_suffix_by_bytes_handles_unicode() {
        let s = "🙂🙂🙂";
        let out = truncate_with_suffix_by_bytes(s, 5, "...[truncated]");
        assert_eq!(out, "🙂...[truncated]");
    }

    #[test]
    fn truncate_with_suffix_by_chars_limits_by_character_count() {
        let out = truncate_with_suffix_by_chars("ab🙂cd", 3, "...");
        assert_eq!(out, "ab🙂...");
    }
}
