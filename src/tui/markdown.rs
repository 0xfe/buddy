//! Markdown-to-terminal rendering helpers.
//!
//! We use `termimad` because it produces terminal-friendly markdown layout
//! (lists, headings, code fences, blockquotes, tables) without requiring a
//! full TUI markdown view.

use termimad::MadSkin;

/// Render markdown into plain terminal text with structure preserved.
///
/// The output intentionally contains no ANSI styling; the outer block renderer
/// controls colors/tints for consistent UI.
pub fn render_markdown_for_terminal(input: &str) -> String {
    let skin = MadSkin::no_style();
    let formatted = skin.text(input, None).to_string();
    trim_trailing_blank_lines(&formatted)
}

fn trim_trailing_blank_lines(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_list_layout() {
        let md = "# Title\n\n- a\n- b";
        let out = render_markdown_for_terminal(md);
        assert!(out.contains("Title"));
        assert!(out.contains("a"));
        assert!(out.contains("b"));
    }

    #[test]
    fn keeps_code_content() {
        let md = "```rust\nfn main() {}\n```";
        let out = render_markdown_for_terminal(md);
        assert!(out.contains("fn main() {}"));
    }
}
