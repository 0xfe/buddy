//! Lightweight syntax highlighting helpers for small preview blocks.
//!
//! We only highlight short snippets (for example the first few lines returned
//! by `read_file`) to keep rendering fast and predictable in the TUI.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

/// A highlighted text fragment with display attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledToken {
    /// Source text fragment for this style span.
    pub text: String,
    /// RGB foreground color to apply.
    pub rgb: (u8, u8, u8),
    /// Bold attribute flag.
    pub bold: bool,
    /// Italic attribute flag.
    pub italic: bool,
    /// Underline attribute flag.
    pub underline: bool,
}

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

/// Pick a deterministic preferred theme with a stable fallback.
fn preferred_theme(theme_set: &ThemeSet) -> Option<&Theme> {
    theme_set
        .themes
        .get("base16-ocean.dark")
        .or_else(|| theme_set.themes.values().next())
}

/// Highlight lines for a file path based on extension/name.
///
/// Returns `None` when no meaningful syntax is detected (plain text) or if
/// highlighting fails.
pub fn highlight_lines_for_path(path: &str, lines: &[&str]) -> Option<Vec<Vec<StyledToken>>> {
    if lines.is_empty() {
        return Some(Vec::new());
    }

    let syntaxes = syntax_set();
    let syntax = syntaxes
        .find_syntax_for_file(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    if syntax.name == "Plain Text" {
        return None;
    }

    let theme = preferred_theme(theme_set())?;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut highlighted = Vec::with_capacity(lines.len());

    for line in lines {
        let ranges = highlighter.highlight_line(line, syntaxes).ok()?;
        let mut tokens = Vec::with_capacity(ranges.len());
        for (style, fragment) in ranges {
            if fragment.is_empty() {
                continue;
            }
            tokens.push(StyledToken {
                text: fragment.to_string(),
                rgb: (style.foreground.r, style.foreground.g, style.foreground.b),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
            });
        }
        highlighted.push(tokens);
    }

    Some(highlighted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_code_when_extension_is_known() {
        // Known extensions should resolve to a syntax and produce style spans.
        let lines = vec!["fn main() {", "    println!(\"hi\");", "}"];
        let highlighted = highlight_lines_for_path("demo.rs", &lines);
        assert!(highlighted.is_some());
        assert!(!highlighted.unwrap().is_empty());
    }

    #[test]
    fn returns_none_for_plain_text_files() {
        // Unknown extensions should remain unhighlighted to avoid false coloring.
        let lines = vec!["just text"];
        let highlighted = highlight_lines_for_path("notes.unknownext", &lines);
        assert!(highlighted.is_none());
    }
}
