//! Centralized, hardcoded UI settings for the terminal interface.
//!
//! This is the single place to tweak prompt strings, glyphs, colors,
//! indentation, and spinner behavior.

use crate::ui::theme::{self, ThemeToken};
use crossterm::style::Color;

// ---------------------------------------------------------------------------
// Layout / indentation
// ---------------------------------------------------------------------------

/// First-level indentation used for status/detail rows.
pub const INDENT_1: &str = "  ";
pub const INDENT_2: &str = "    ";
pub const SNIPPET_PREVIEW_LINES: usize = 10;
pub const BLOCK_FALLBACK_COLUMNS: usize = 100;
pub const BLOCK_RIGHT_MARGIN: usize = 2;

// ---------------------------------------------------------------------------
// Prompt strings
// ---------------------------------------------------------------------------

/// Default local prompt shown in non-annotated mode.
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
/// ASCII spinner frames used by the progress indicator.
pub const PROGRESS_FRAMES: [char; 4] = ['|', '/', '-', '\\'];
pub const PROGRESS_TICK_MS: u64 = 100;

pub const REPL_EVENT_POLL_MS: u64 = 80;

// ---------------------------------------------------------------------------
// Theme tokens
// ---------------------------------------------------------------------------

/// Resolve one semantic color token from the active theme.
fn theme_color(token: ThemeToken) -> Color {
    theme::color(token)
}

/// Resolve one semantic RGB token from the active theme.
fn theme_rgb(token: ThemeToken) -> (u8, u8, u8) {
    theme::rgb(token)
}

pub fn color_prompt_host() -> Color {
    theme_color(ThemeToken::PromptHost)
}
pub fn color_prompt_symbol() -> Color {
    theme_color(ThemeToken::PromptSymbol)
}
pub fn color_prompt_approval_query() -> Color {
    theme_color(ThemeToken::PromptApprovalQuery)
}
pub fn color_prompt_approval_command() -> Color {
    theme_color(ThemeToken::PromptApprovalCommand)
}
pub fn color_prompt_approval_privileged() -> Color {
    theme_color(ThemeToken::PromptApprovalPrivileged)
}
pub fn color_prompt_approval_mutation() -> Color {
    theme_color(ThemeToken::PromptApprovalMutation)
}
pub fn color_status_line() -> Color {
    theme_color(ThemeToken::StatusLine)
}
pub fn color_continuation_prompt() -> Color {
    theme_color(ThemeToken::ContinuationPrompt)
}
pub fn color_agent_label() -> Color {
    theme_color(ThemeToken::AgentLabel)
}
pub fn color_model_name() -> Color {
    theme_color(ThemeToken::ModelName)
}
pub fn color_tool_call_glyph() -> Color {
    theme_color(ThemeToken::ToolCallGlyph)
}
pub fn color_tool_call_name() -> Color {
    theme_color(ThemeToken::ToolCallName)
}
pub fn color_tool_call_args() -> Color {
    theme_color(ThemeToken::ToolCallArgs)
}
pub fn color_tool_result_glyph() -> Color {
    theme_color(ThemeToken::ToolResultGlyph)
}
pub fn color_tool_result_text() -> Color {
    theme_color(ThemeToken::ToolResultText)
}
pub fn color_token_label() -> Color {
    theme_color(ThemeToken::TokenLabel)
}
pub fn color_token_value() -> Color {
    theme_color(ThemeToken::TokenValue)
}
pub fn color_token_session() -> Color {
    theme_color(ThemeToken::TokenSession)
}
pub fn color_reasoning_label() -> Color {
    theme_color(ThemeToken::ReasoningLabel)
}
pub fn color_reasoning_meta() -> Color {
    theme_color(ThemeToken::ReasoningMeta)
}
pub fn color_activity_text() -> Color {
    theme_color(ThemeToken::ActivityText)
}
pub fn color_warning() -> Color {
    theme_color(ThemeToken::Warning)
}
pub fn color_error() -> Color {
    theme_color(ThemeToken::Error)
}
pub fn color_section_bullet() -> Color {
    theme_color(ThemeToken::SectionBullet)
}
pub fn color_section_title() -> Color {
    theme_color(ThemeToken::SectionTitle)
}
pub fn color_field_key() -> Color {
    theme_color(ThemeToken::FieldKey)
}
pub fn color_field_value() -> Color {
    theme_color(ThemeToken::FieldValue)
}
pub fn color_progress_frame() -> Color {
    theme_color(ThemeToken::ProgressFrame)
}
pub fn color_progress_label() -> Color {
    theme_color(ThemeToken::ProgressLabel)
}
pub fn color_progress_elapsed() -> Color {
    theme_color(ThemeToken::ProgressElapsed)
}
pub fn color_autocomplete_selected() -> Color {
    theme_color(ThemeToken::AutocompleteSelected)
}
pub fn color_autocomplete_unselected() -> Color {
    theme_color(ThemeToken::AutocompleteUnselected)
}
pub fn color_autocomplete_command() -> Color {
    theme_color(ThemeToken::AutocompleteCommand)
}
pub fn color_autocomplete_description() -> Color {
    theme_color(ThemeToken::AutocompleteDescription)
}
pub fn color_snippet_tool_bg() -> Color {
    theme_color(ThemeToken::BlockToolBg)
}
pub fn color_snippet_tool_text() -> Color {
    theme_color(ThemeToken::BlockToolText)
}
pub fn color_snippet_reasoning_bg() -> Color {
    theme_color(ThemeToken::BlockReasoningBg)
}
pub fn color_snippet_reasoning_text() -> Color {
    theme_color(ThemeToken::BlockReasoningText)
}
pub fn color_snippet_approval_bg() -> Color {
    theme_color(ThemeToken::BlockApprovalBg)
}
pub fn color_snippet_approval_text() -> Color {
    theme_color(ThemeToken::BlockApprovalText)
}
pub fn color_snippet_assistant_bg() -> Color {
    theme_color(ThemeToken::BlockAssistantBg)
}
pub fn color_snippet_assistant_text() -> Color {
    theme_color(ThemeToken::BlockAssistantText)
}
pub fn color_snippet_truncated() -> Color {
    theme_color(ThemeToken::BlockTruncated)
}
pub fn rgb_snippet_assistant_text() -> (u8, u8, u8) {
    theme_rgb(ThemeToken::BlockAssistantText)
}
pub fn rgb_snippet_assistant_md_heading() -> (u8, u8, u8) {
    theme_rgb(ThemeToken::MarkdownHeading)
}
pub fn rgb_snippet_assistant_md_marker() -> (u8, u8, u8) {
    theme_rgb(ThemeToken::MarkdownMarker)
}
pub fn rgb_snippet_assistant_md_quote() -> (u8, u8, u8) {
    theme_rgb(ThemeToken::MarkdownQuote)
}
pub fn rgb_snippet_assistant_md_code() -> (u8, u8, u8) {
    theme_rgb(ThemeToken::MarkdownCode)
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Build the normal prompt text shown for command entry.
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

/// Return the autocomplete marker glyph based on selection and color mode.
pub fn suggestion_marker(is_selected: bool, color: bool) -> &'static str {
    match (is_selected, color) {
        (true, true) => AUTOCOMPLETE_SELECTED_COLOR,
        (false, true) => AUTOCOMPLETE_UNSELECTED_COLOR,
        (true, false) => AUTOCOMPLETE_SELECTED_PLAIN,
        (false, false) => AUTOCOMPLETE_UNSELECTED_PLAIN,
    }
}
