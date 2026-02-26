//! Shared text formatting helpers used by terminal rendering.

/// A clipped text preview used for compact block rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetPreview<'a> {
    pub lines: Vec<&'a str>,
    pub remaining_lines: usize,
}

/// Count visible character width (single-cell approximation).
pub fn visible_width(s: &str) -> usize {
    s.chars().count()
}

/// Truncate text for single-line display and replace newlines with spaces.
pub fn truncate_single_line(s: &str, max_len: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.len() > max_len {
        format!("{}...", &flat[..max_len])
    } else {
        flat
    }
}

/// Return up to `max_lines` lines from `text` and count the lines omitted.
pub fn snippet_preview(text: &str, max_lines: usize) -> SnippetPreview<'_> {
    let all_lines: Vec<&str> = text.lines().collect();
    if all_lines.is_empty() {
        return SnippetPreview {
            lines: Vec::new(),
            remaining_lines: 0,
        };
    }

    let shown = all_lines.len().min(max_lines);
    SnippetPreview {
        lines: all_lines[..shown].to_vec(),
        remaining_lines: all_lines.len().saturating_sub(shown),
    }
}

/// Clip a string to at most `max_width` visible characters.
pub fn clip_to_width(s: &str, max_width: usize) -> String {
    s.chars().take(max_width).collect()
}

/// Wrap a single line to fit `max_width`.
///
/// This prefers whitespace boundaries when possible and falls back to hard
/// wrapping long words/tokens.
pub fn wrap_for_block(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return Vec::new();
    }
    if line.is_empty() {
        return vec![String::new()];
    }

    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;

    while start < chars.len() {
        let end = (start + max_width).min(chars.len());
        if end == chars.len() {
            out.push(chars[start..end].iter().collect());
            break;
        }
        if chars[end].is_whitespace() {
            out.push(chars[start..end].iter().collect());
            start = end;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
            continue;
        }

        let mut split = None;
        for idx in (start + 1..end).rev() {
            if chars[idx].is_whitespace() {
                split = Some(idx);
                break;
            }
        }

        if let Some(split_idx) = split {
            let row: String = chars[start..split_idx].iter().collect();
            out.push(row);
            start = split_idx;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
            continue;
        }

        out.push(chars[start..end].iter().collect());
        start = end;
    }

    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_flattens_and_clips() {
        let out = truncate_single_line("hello\nworld", 8);
        assert_eq!(out, "hello wo...");
    }

    #[test]
    fn snippet_preview_limits_lines_and_counts_remaining() {
        let input = "1\n2\n3\n4";
        let preview = snippet_preview(input, 2);
        assert_eq!(preview.lines, vec!["1", "2"]);
        assert_eq!(preview.remaining_lines, 2);
    }

    #[test]
    fn snippet_preview_handles_empty_input() {
        let preview = snippet_preview("", 10);
        assert!(preview.lines.is_empty());
        assert_eq!(preview.remaining_lines, 0);
    }

    #[test]
    fn clip_to_width_limits_by_chars() {
        assert_eq!(clip_to_width("abcdef", 3), "abc");
    }

    #[test]
    fn wrap_for_block_prefers_word_boundaries() {
        let wrapped = wrap_for_block("one two three", 7);
        assert_eq!(wrapped, vec!["one two".to_string(), "three".to_string()]);
    }

    #[test]
    fn wrap_for_block_falls_back_to_hard_wrap() {
        let wrapped = wrap_for_block("superlongtoken", 5);
        assert_eq!(
            wrapped,
            vec!["super".to_string(), "longt".to_string(), "oken".to_string()]
        );
    }

    #[test]
    fn visible_width_counts_chars() {
        assert_eq!(visible_width("abc"), 3);
    }
}
