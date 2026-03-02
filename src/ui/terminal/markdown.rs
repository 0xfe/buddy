//! Markdown-to-terminal rendering helpers.
//!
//! We prefer `termimad` because it produces terminal-friendly markdown layout
//! (lists, code fences, blockquotes, tables) without requiring a full TUI
//! markdown view.
//!
//! Some markdown renderers flatten headings and remove the leading `#` markers.
//! Buddy's line-level styler relies on those markers to render heading titles
//! with stronger visual emphasis. When heading markers are lost, we fall back to
//! source-preserving text so headings stay recognizable.

use termimad::MadSkin;

/// Render markdown into plain terminal text with structure preserved.
///
/// The output intentionally contains no ANSI styling; the outer block renderer
/// controls colors/tints for consistent UI.
pub fn render_markdown_for_terminal(input: &str) -> String {
    let skin = MadSkin::no_style();
    let formatted = skin.text(input, None).to_string();
    if has_markdown_heading(input) && !has_markdown_heading(&formatted) {
        return trim_trailing_blank_lines(input);
    }
    trim_trailing_blank_lines(&formatted)
}

fn trim_trailing_blank_lines(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

fn has_markdown_heading(s: &str) -> bool {
    s.lines().any(is_markdown_heading_line)
}

fn is_markdown_heading_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hash_count = trimmed.chars().take_while(|ch| *ch == '#').count();
    hash_count > 0 && trimmed.chars().nth(hash_count) == Some(' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_list_layout() {
        // Lists/headings should survive markdown rendering in readable form.
        let md = "# Title\n\n- a\n- b";
        let out = render_markdown_for_terminal(md);
        assert!(out.contains("Title"));
        assert!(out.contains("a"));
        assert!(out.contains("b"));
    }

    #[test]
    fn keeps_code_content() {
        // Code fence contents should remain intact after rendering.
        let md = "```rust\nfn main() {}\n```";
        let out = render_markdown_for_terminal(md);
        assert!(out.contains("fn main() {}"));
    }

    #[test]
    fn preserves_heading_markers_when_renderer_flattens_them() {
        // If the markdown formatter strips leading `#`, fall back to source so
        // heading styling in the outer renderer can still detect title lines.
        let md = "# Title\n\nParagraph";
        let out = render_markdown_for_terminal(md);
        assert!(out.lines().next().unwrap_or_default().starts_with("# "));
    }
}
