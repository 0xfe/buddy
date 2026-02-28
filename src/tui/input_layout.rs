//! Terminal row/column layout helpers for the interactive editor.

use crate::tui::commands::SlashCommand;
use crate::tui::settings;
use crossterm::terminal;

/// Computed layout for prompt + input buffer on the terminal surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputLayout {
    /// Total terminal rows consumed by prompt + buffer.
    pub(crate) total_rows: usize,
    /// Row index where the cursor should be placed.
    pub(crate) cursor_row: usize,
    /// Column index where the cursor should be placed.
    pub(crate) cursor_col: usize,
}

/// Read terminal width or fallback to 80 columns.
pub(crate) fn terminal_columns() -> usize {
    terminal::size()
        .ok()
        .map(|(cols, _)| cols as usize)
        .filter(|cols| *cols > 0)
        .unwrap_or(80)
}

/// Compute wrapped rows consumed by a one-line status string.
pub(crate) fn wrapped_rows(text: &str, cols: usize) -> usize {
    let mut row = 0usize;
    let mut col = 0usize;
    advance_text(text, cols, &mut row, &mut col);
    row + 1
}

/// Compute terminal layout for prompt + input and current cursor position.
pub(crate) fn compute_input_layout(
    buffer: &str,
    cursor: usize,
    cols: usize,
    primary_prompt: &str,
) -> InputLayout {
    let mut row = 0usize;
    let mut col = 0usize;
    let mut cursor_pos: Option<(usize, usize)> = None;

    advance_text(primary_prompt, cols, &mut row, &mut col);

    for (idx, ch) in buffer.chars().enumerate() {
        if idx == cursor {
            cursor_pos = Some((row, col));
        }
        if ch == '\n' {
            row += 1;
            col = 0;
            advance_text(settings::PROMPT_CONTINUATION, cols, &mut row, &mut col);
        } else {
            advance_char(cols, &mut row, &mut col);
        }
    }

    let (cursor_row, cursor_col) = cursor_pos.unwrap_or((row, col));
    InputLayout {
        total_rows: row + 1,
        cursor_row,
        cursor_col,
    }
}

/// Compute how many rows autocomplete suggestions consume.
pub(crate) fn suggestion_rows(
    matches: &[SlashCommand],
    selected: usize,
    color: bool,
    cols: usize,
) -> usize {
    let mut rows = 0usize;
    for (idx, cmd) in matches.iter().enumerate() {
        // Printed as "\r\n{line}".
        rows += 1;
        let text = suggestion_text(cmd, idx == selected, color);
        let mut row = 0usize;
        let mut col = 0usize;
        advance_text(&text, cols, &mut row, &mut col);
        rows += row;
    }
    rows
}

/// Build one unstyled suggestion line for layout estimation.
pub(crate) fn suggestion_text(cmd: &SlashCommand, is_selected: bool, color: bool) -> String {
    let marker = settings::suggestion_marker(is_selected, color);
    format!(
        "{}{marker} {} {}",
        settings::AUTOCOMPLETE_PREFIX,
        cmd.name,
        cmd.description
    )
}

fn advance_text(text: &str, cols: usize, row: &mut usize, col: &mut usize) {
    for ch in text.chars() {
        if ch == '\n' {
            *row += 1;
            *col = 0;
        } else {
            advance_char(cols, row, col);
        }
    }
}

/// Advance by one printable cell, wrapping to the next row when needed.
fn advance_char(cols: usize, row: &mut usize, col: &mut usize) {
    if cols == 0 {
        return;
    }
    if *col + 1 >= cols {
        *row += 1;
        *col = 0;
    } else {
        *col += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_PRIMARY_PROMPT: &str = settings::PROMPT_LOCAL_PRIMARY;

    fn lines_to_move_up_for(
        buffer: &str,
        cursor: usize,
        matches: &[SlashCommand],
        selected: usize,
        color: bool,
        cols: usize,
    ) -> usize {
        // Mirrors the render-time cursor math used by `render_editor`.
        let input_layout = compute_input_layout(buffer, cursor, cols, LOCAL_PRIMARY_PROMPT);
        let suggestion_rows = suggestion_rows(matches, selected, color, cols);
        let bottom_row = input_layout
            .total_rows
            .saturating_sub(1)
            .saturating_add(suggestion_rows);
        bottom_row.saturating_sub(input_layout.cursor_row)
    }

    #[test]
    fn input_layout_tracks_soft_wraps() {
        // Long single lines should soft-wrap into additional rows.
        let layout = compute_input_layout("abcdefghij", 10, 8, LOCAL_PRIMARY_PROMPT);
        assert_eq!(layout.total_rows, 2);
        assert_eq!(layout.cursor_row, 1);
    }

    #[test]
    fn input_layout_tracks_multiline_and_cursor_position() {
        // Newlines must advance rows while preserving cursor row correctness.
        let layout = compute_input_layout("ab\ncde", 4, 20, LOCAL_PRIMARY_PROMPT);
        assert_eq!(layout.cursor_row, 1);
        assert_eq!(layout.total_rows, 2);
    }

    #[test]
    fn input_layout_wraps_exactly_at_terminal_edge() {
        // Exact-edge cases should not introduce spurious extra rows.
        let layout = compute_input_layout("abcd", 4, 11, LOCAL_PRIMARY_PROMPT);
        assert_eq!(layout.cursor_row, 0);
        assert_eq!(layout.cursor_col, 6);
        assert_eq!(layout.total_rows, 1);
    }

    #[test]
    fn input_layout_handles_very_narrow_terminal() {
        // Narrow widths should remain stable for empty and short buffers.
        let empty = compute_input_layout("", 0, 4, LOCAL_PRIMARY_PROMPT);
        assert_eq!(empty.cursor_row, 0);
        assert_eq!(empty.cursor_col, 2);
        assert_eq!(empty.total_rows, 1);

        let one_char = compute_input_layout("x", 1, 4, LOCAL_PRIMARY_PROMPT);
        assert_eq!(one_char.cursor_row, 0);
        assert_eq!(one_char.cursor_col, 3);
        assert_eq!(one_char.total_rows, 1);
    }

    #[test]
    fn input_layout_handles_wrapped_continuation_prompt() {
        // Continuation prompts may themselves wrap in very narrow terminals.
        let layout = compute_input_layout("\na", 2, 4, LOCAL_PRIMARY_PROMPT);
        assert_eq!(layout.cursor_row, 3);
        assert_eq!(layout.cursor_col, 0);
        assert_eq!(layout.total_rows, 4);
    }

    #[test]
    fn input_layout_cursor_can_be_mid_buffer() {
        // Cursor position should be accurate for mid-buffer edits.
        let layout = compute_input_layout("abcdef", 2, 10, LOCAL_PRIMARY_PROMPT);
        assert_eq!(layout.cursor_row, 0);
        assert_eq!(layout.cursor_col, 4);
        assert_eq!(layout.total_rows, 1);
    }

    #[test]
    fn suggestion_rows_include_wrapped_lines() {
        // Suggestion row counting includes wrapped description text.
        const LONG_DESC: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let cmd = SlashCommand {
            name: "/status",
            description: LONG_DESC,
        };
        let rows = suggestion_rows(&[cmd], 0, false, 16);
        assert!(rows > 1);
    }

    #[test]
    fn suggestion_rows_scale_with_multiple_commands() {
        // Total suggestion rows should grow with command count.
        const LONG_DESC: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let cmds = [
            SlashCommand {
                name: "/status",
                description: LONG_DESC,
            },
            SlashCommand {
                name: "/context",
                description: LONG_DESC,
            },
            SlashCommand {
                name: "/help",
                description: "short",
            },
        ];
        let rows = suggestion_rows(&cmds, 1, true, 20);
        assert!(rows >= cmds.len());
    }

    #[test]
    fn lines_to_move_up_matches_visual_distance() {
        // Cursor reset distance should match the visible suggestion footprint.
        const LONG_DESC: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let cmds = [SlashCommand {
            name: "/status",
            description: LONG_DESC,
        }];

        let lines = lines_to_move_up_for("abcdefghij", 1, &cmds, 0, false, 8);
        assert!(lines > 1);
    }

    #[test]
    fn lines_to_move_up_zero_at_bottom_without_suggestions() {
        // With no suggestions and cursor at end, no upward cursor move is needed.
        let lines = lines_to_move_up_for("hello", 5, &[], 0, false, 80);
        assert_eq!(lines, 0);
    }
}
