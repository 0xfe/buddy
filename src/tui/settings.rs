//! Centralized, hardcoded UI settings for the terminal interface.
//!
//! This is the single place to tweak prompt strings, glyphs, colors,
//! indentation, and spinner behavior.

use crossterm::style::Color;

// ---------------------------------------------------------------------------
// Layout / indentation
// ---------------------------------------------------------------------------

pub const INDENT_1: &str = "  ";
pub const INDENT_2: &str = "    ";
pub const SNIPPET_PREVIEW_LINES: usize = 10;
pub const BLOCK_FALLBACK_COLUMNS: usize = 100;
pub const BLOCK_RIGHT_MARGIN: usize = 2;

// ---------------------------------------------------------------------------
// Prompt strings
// ---------------------------------------------------------------------------

pub const PROMPT_LOCAL_PRIMARY: &str = "> ";
pub const PROMPT_LOCAL_APPROVAL: &str = "• approve? [y/n] ";
pub const PROMPT_APPROVAL_SEPARATOR: &str = " \u{2022} ";
pub const PROMPT_APPROVAL_QUERY: &str = "approve?";
pub const PROMPT_CONTINUATION_LABEL: &str = "......";
pub const PROMPT_CONTINUATION: &str = "...... ";
pub const PROMPT_SYMBOL: &str = ">";
pub const PROMPT_SPACER: &str = " ";

pub const SSH_PREFIX: &str = "(ssh ";
pub const SSH_SUFFIX: &str = ")";

pub const STATUS_LINE_NEWLINE_FALLBACK: &str = "\n";
pub const STATUS_LINE_NEWLINE_INTERACTIVE: &str = "\r\n";

// ---------------------------------------------------------------------------
// Sections / labels
// ---------------------------------------------------------------------------

pub const LABEL_AGENT: &str = "buddy";
pub const LABEL_WARNING: &str = "warning:";
pub const LABEL_ERROR: &str = "error:";
pub const LABEL_THINKING: &str = "thinking";
pub const LABEL_TOKENS: &str = "tokens";

pub const GLYPH_SECTION_BULLET: &str = "•";
pub const GLYPH_TOOL_CALL: &str = "▶";
pub const GLYPH_TOOL_RESULT: &str = "\u{2190}";
pub const GLYPH_TOOL_CALL_PLAIN: &str = ">";
pub const GLYPH_TOOL_RESULT_PLAIN: &str = "<-";

// ---------------------------------------------------------------------------
// Autocomplete UI
// ---------------------------------------------------------------------------

pub const AUTOCOMPLETE_PREFIX: &str = INDENT_1;
pub const AUTOCOMPLETE_SELECTED_COLOR: &str = "▶";
pub const AUTOCOMPLETE_UNSELECTED_COLOR: &str = "·";
pub const AUTOCOMPLETE_SELECTED_PLAIN: &str = ">";
pub const AUTOCOMPLETE_UNSELECTED_PLAIN: &str = "-";

// ---------------------------------------------------------------------------
// Spinner / progress
// ---------------------------------------------------------------------------

pub const PROGRESS_CLEAR_LINE: &str = "\r\x1b[2K";
pub const PROGRESS_FRAMES: [char; 4] = ['|', '/', '-', '\\'];
pub const PROGRESS_TICK_MS: u64 = 100;

pub const REPL_EVENT_POLL_MS: u64 = 80;

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

pub const COLOR_PROMPT_HOST: Color = Color::Grey;
pub const COLOR_PROMPT_SYMBOL: Color = Color::White;
pub const COLOR_PROMPT_APPROVAL_QUERY: Color = Color::Yellow;
pub const COLOR_PROMPT_APPROVAL_COMMAND: Color = Color::White;
pub const COLOR_PROMPT_APPROVAL_BG: Color = Color::Rgb {
    r: 58,
    g: 26,
    b: 26,
};
pub const COLOR_PROMPT_APPROVAL_SEPARATOR: Color = Color::DarkRed;
pub const COLOR_STATUS_LINE: Color = Color::DarkGrey;
pub const COLOR_CONTINUATION_PROMPT: Color = Color::DarkGrey;

pub const COLOR_AGENT_LABEL: Color = Color::Green;
pub const COLOR_MODEL_NAME: Color = Color::DarkGrey;

pub const COLOR_TOOL_CALL_GLYPH: Color = Color::DarkYellow;
pub const COLOR_TOOL_CALL_NAME: Color = Color::Yellow;
pub const COLOR_TOOL_CALL_ARGS: Color = Color::DarkGrey;

pub const COLOR_TOOL_RESULT_GLYPH: Color = Color::DarkGrey;
pub const COLOR_TOOL_RESULT_TEXT: Color = Color::DarkGrey;

pub const COLOR_TOKEN_LABEL: Color = Color::DarkGrey;
pub const COLOR_TOKEN_VALUE: Color = Color::DarkCyan;
pub const COLOR_TOKEN_SESSION: Color = Color::Cyan;

pub const COLOR_REASONING_LABEL: Color = Color::Magenta;
pub const COLOR_REASONING_META: Color = Color::DarkGrey;
pub const COLOR_REASONING_TEXT: Color = Color::DarkGrey;
pub const COLOR_ACTIVITY_TEXT: Color = Color::DarkGrey;

pub const COLOR_WARNING: Color = Color::Yellow;
pub const COLOR_ERROR: Color = Color::Red;

pub const COLOR_SECTION_BULLET: Color = Color::DarkGrey;
pub const COLOR_SECTION_TITLE: Color = Color::Cyan;
pub const COLOR_FIELD_KEY: Color = Color::DarkGrey;
pub const COLOR_FIELD_VALUE: Color = Color::White;

pub const COLOR_PROGRESS_FRAME: Color = Color::Cyan;
pub const COLOR_PROGRESS_LABEL: Color = Color::DarkGrey;
pub const COLOR_PROGRESS_ELAPSED: Color = Color::DarkGrey;

pub const COLOR_AUTOCOMPLETE_SELECTED: Color = Color::DarkYellow;
pub const COLOR_AUTOCOMPLETE_UNSELECTED: Color = Color::DarkGrey;
pub const COLOR_AUTOCOMPLETE_COMMAND: Color = Color::Yellow;
pub const COLOR_AUTOCOMPLETE_DESCRIPTION: Color = Color::DarkGrey;

pub const COLOR_SNIPPET_TOOL_BG: Color = Color::Rgb {
    r: 34,
    g: 56,
    b: 44,
};
pub const COLOR_SNIPPET_TOOL_TEXT: Color = Color::Rgb {
    r: 242,
    g: 248,
    b: 244,
};
pub const COLOR_SNIPPET_REASONING_BG: Color = Color::Rgb {
    r: 30,
    g: 50,
    b: 39,
};
pub const COLOR_SNIPPET_REASONING_TEXT: Color = Color::Rgb {
    r: 184,
    g: 191,
    b: 186,
};
pub const COLOR_SNIPPET_APPROVAL_BG: Color = Color::Rgb {
    r: 60,
    g: 24,
    b: 24,
};
pub const COLOR_SNIPPET_APPROVAL_TEXT: Color = Color::Rgb {
    r: 244,
    g: 208,
    b: 208,
};

pub const COLOR_SNIPPET_ASSISTANT_BG: Color = Color::Rgb {
    r: 34,
    g: 56,
    b: 44,
};
pub const COLOR_SNIPPET_ASSISTANT_TEXT: Color = Color::White;
pub const COLOR_SNIPPET_TRUNCATED: Color = Color::Grey;
pub const RGB_SNIPPET_ASSISTANT_TEXT: (u8, u8, u8) = (242, 248, 244);
pub const RGB_SNIPPET_ASSISTANT_MD_HEADING: (u8, u8, u8) = (210, 236, 190);
pub const RGB_SNIPPET_ASSISTANT_MD_MARKER: (u8, u8, u8) = (166, 206, 172);
pub const RGB_SNIPPET_ASSISTANT_MD_QUOTE: (u8, u8, u8) = (184, 210, 196);
pub const RGB_SNIPPET_ASSISTANT_MD_CODE: (u8, u8, u8) = (238, 224, 188);

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

pub fn normal_prompt_text(ssh_target: Option<&str>, context_used_percent: Option<u16>) -> String {
    if ssh_target.is_none() && context_used_percent.is_none() {
        return PROMPT_LOCAL_PRIMARY.to_string();
    }

    let mut out = String::new();
    if let Some(target) = ssh_target {
        out.push_str(&format!("{SSH_PREFIX}{target}{SSH_SUFFIX}"));
    }
    if let Some(used) = context_used_percent {
        if ssh_target.is_some() {
            out.push(' ');
        }
        out.push_str(&format!("({used}% used)"));
    }
    out.push_str(PROMPT_SYMBOL);
    out.push_str(PROMPT_SPACER);
    out
}

pub fn approval_prompt_text() -> &'static str {
    PROMPT_LOCAL_APPROVAL
}

pub fn suggestion_marker(is_selected: bool, color: bool) -> &'static str {
    match (is_selected, color) {
        (true, true) => AUTOCOMPLETE_SELECTED_COLOR,
        (false, true) => AUTOCOMPLETE_UNSELECTED_COLOR,
        (true, false) => AUTOCOMPLETE_SELECTED_PLAIN,
        (false, false) => AUTOCOMPLETE_UNSELECTED_PLAIN,
    }
}
