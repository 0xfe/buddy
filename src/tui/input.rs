//! Interactive REPL input with history, multiline editing, and autocomplete.
//!
//! The editor intentionally exposes a minimal API (`read_repl_line_with_interrupt`)
//! so the caller can own task-state decisions while this module owns terminal
//! editing mechanics.

use crate::tui::commands::{matching_slash_commands, SlashCommand};
use crate::tui::input_buffer::{
    char_count, delete_char_at_cursor, delete_char_before_cursor, delete_char_range, history_down,
    history_up, insert_char_at_cursor, line_end_char_index, line_start_char_index,
    previous_word_start, InputDraft,
};
use crate::tui::input_layout::{
    compute_input_layout, suggestion_rows, terminal_columns, wrapped_rows,
};
use crate::tui::prompt::{
    primary_prompt_text, write_continuation_prompt, write_primary_prompt, write_status_line,
    ApprovalPrompt, PromptMode,
};
use crate::tui::settings;
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Print, PrintStyledContent, Stylize};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::QueueableCommand;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

pub use crate::tui::input_buffer::ReplState;

/// Result of reading one interactive input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Line(String),
    Eof,
    Cancelled,
    Interrupted,
}

/// Input-loop poll result (interrupt signal + optional status line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadPoll {
    pub interrupt: bool,
    pub status_line: Option<String>,
}

/// Read one REPL line, but allow an external caller to interrupt input.
///
/// The callback is polled periodically while waiting for key events. When it
/// reports `interrupt = true`, the current editor surface is cleared and
/// `Interrupted` is returned so the caller can render higher-priority UI.
pub fn read_repl_line_with_interrupt<F>(
    color: bool,
    state: &mut ReplState,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    mut poll: F,
) -> io::Result<ReadOutcome>
where
    F: FnMut() -> ReadPoll,
{
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return read_line_fallback(
            color,
            ssh_target,
            context_used_percent,
            prompt_mode,
            approval_prompt,
            &mut poll,
        );
    }
    read_line_interactive(
        color,
        state,
        ssh_target,
        context_used_percent,
        prompt_mode,
        approval_prompt,
        &mut poll,
    )
}

fn read_line_fallback<F>(
    color: bool,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    poll: &mut F,
) -> io::Result<ReadOutcome>
where
    F: FnMut() -> ReadPoll,
{
    let poll_state = poll();
    if poll_state.interrupt {
        return Ok(ReadOutcome::Interrupted);
    }
    if let Some(status_line) = poll_state.status_line {
        write_status_line(
            &mut io::stderr(),
            color,
            &status_line,
            settings::STATUS_LINE_NEWLINE_FALLBACK,
        )?;
        io::stderr().flush()?;
    }

    write_primary_prompt(
        &mut io::stderr(),
        color,
        ssh_target,
        context_used_percent,
        prompt_mode,
        approval_prompt,
    )?;
    io::stderr().flush()?;

    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        eprintln!();
        return Ok(ReadOutcome::Eof);
    }

    Ok(ReadOutcome::Line(
        line.trim_end_matches(['\n', '\r']).to_string(),
    ))
}

fn read_line_interactive(
    color: bool,
    state: &mut ReplState,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    poll: &mut dyn FnMut() -> ReadPoll,
) -> io::Result<ReadOutcome> {
    let _guard = RawModeGuard::acquire()?;
    let mut stderr = io::stderr();
    let primary_prompt = primary_prompt_text(
        ssh_target,
        context_used_percent,
        prompt_mode,
        approval_prompt,
    );

    let draft = state.take_draft(prompt_mode);
    let mut buffer = draft.buffer;
    let mut cursor = draft.cursor; // cursor in char indices
    let mut selected = draft.selected;
    let mut previous_cursor_row = 0usize;
    let mut history_index = draft.history_index.filter(|idx| *idx < state.history_len());
    let mut history_draft = draft.history_draft;

    loop {
        let poll_state = poll();
        let matches = matching_slash_commands(&buffer);
        if selected >= matches.len() {
            selected = 0;
        }
        previous_cursor_row = render_editor(
            &mut stderr,
            color,
            ssh_target,
            context_used_percent,
            prompt_mode,
            approval_prompt,
            &primary_prompt,
            poll_state.status_line.as_deref(),
            &buffer,
            cursor,
            &matches,
            selected,
            previous_cursor_row,
        )?;

        if poll_state.interrupt {
            state.save_draft(
                prompt_mode,
                InputDraft {
                    buffer,
                    cursor,
                    selected,
                    history_index,
                    history_draft,
                },
            );
            clear_editor_surface(&mut stderr, previous_cursor_row)?;
            return Ok(ReadOutcome::Interrupted);
        }

        if !event::poll(Duration::from_millis(settings::REPL_EVENT_POLL_MS))? {
            continue;
        }

        let evt = event::read()?;
        let Event::Key(key) = evt else {
            continue;
        };
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            continue;
        }

        match key.code {
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                insert_char_at_cursor(&mut buffer, &mut cursor, '\n');
                selected = 0;
                history_index = None;
            }
            KeyCode::Enter => {
                state.clear_draft(prompt_mode);
                finalize_editor(
                    &mut stderr,
                    color,
                    ssh_target,
                    context_used_percent,
                    prompt_mode,
                    approval_prompt,
                    &buffer,
                    previous_cursor_row,
                )?;
                return Ok(ReadOutcome::Line(buffer));
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if buffer.is_empty() {
                    state.clear_draft(prompt_mode);
                    finalize_editor(
                        &mut stderr,
                        color,
                        ssh_target,
                        context_used_percent,
                        prompt_mode,
                        approval_prompt,
                        "",
                        previous_cursor_row,
                    )?;
                    return Ok(ReadOutcome::Eof);
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.clear_draft(prompt_mode);
                finalize_editor(
                    &mut stderr,
                    color,
                    ssh_target,
                    context_used_percent,
                    prompt_mode,
                    approval_prompt,
                    "",
                    previous_cursor_row,
                )?;
                return Ok(ReadOutcome::Cancelled);
            }
            KeyCode::Tab => {
                if let Some(choice) = matches.get(selected) {
                    buffer = choice.name.to_string();
                    cursor = char_count(&buffer);
                    selected = 0;
                    history_index = None;
                }
            }
            KeyCode::Up => {
                if buffer.starts_with('/') && !matches.is_empty() {
                    selected = if selected == 0 {
                        matches.len() - 1
                    } else {
                        selected - 1
                    };
                } else {
                    history_up(state, &mut history_index, &mut history_draft, &mut buffer);
                    cursor = char_count(&buffer);
                    selected = 0;
                }
            }
            KeyCode::Down => {
                if buffer.starts_with('/') && !matches.is_empty() {
                    selected = (selected + 1) % matches.len();
                } else {
                    history_down(state, &mut history_index, &history_draft, &mut buffer);
                    cursor = char_count(&buffer);
                    selected = 0;
                }
            }
            KeyCode::Left => {
                cursor = cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let len = char_count(&buffer);
                if cursor < len {
                    cursor += 1;
                }
            }
            KeyCode::Home => {
                cursor = line_start_char_index(&buffer, cursor);
            }
            KeyCode::End => {
                cursor = line_end_char_index(&buffer, cursor);
            }
            KeyCode::Backspace => {
                if cursor > 0 {
                    delete_char_before_cursor(&mut buffer, &mut cursor);
                    selected = 0;
                    history_index = None;
                }
            }
            KeyCode::Delete => {
                if cursor < char_count(&buffer) {
                    delete_char_at_cursor(&mut buffer, cursor);
                    selected = 0;
                    history_index = None;
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                cursor = line_start_char_index(&buffer, cursor);
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                cursor = line_end_char_index(&buffer, cursor);
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                cursor = cursor.saturating_sub(1);
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if cursor < char_count(&buffer) {
                    cursor += 1;
                }
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let end = line_end_char_index(&buffer, cursor);
                delete_char_range(&mut buffer, cursor, end);
                selected = 0;
                history_index = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let start = line_start_char_index(&buffer, cursor);
                delete_char_range(&mut buffer, start, cursor);
                cursor = start;
                selected = 0;
                history_index = None;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let start = previous_word_start(&buffer, cursor);
                delete_char_range(&mut buffer, start, cursor);
                cursor = start;
                selected = 0;
                history_index = None;
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                history_up(state, &mut history_index, &mut history_draft, &mut buffer);
                cursor = char_count(&buffer);
                selected = 0;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                history_down(state, &mut history_index, &history_draft, &mut buffer);
                cursor = char_count(&buffer);
                selected = 0;
            }
            KeyCode::Char(ch) => {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    continue;
                }
                insert_char_at_cursor(&mut buffer, &mut cursor, ch);
                selected = 0;
                history_index = None;
            }
            _ => {}
        }
    }
}

fn clear_editor_surface(stderr: &mut io::Stderr, previous_cursor_row: usize) -> io::Result<()> {
    if previous_cursor_row > 0 {
        stderr.queue(MoveUp(previous_cursor_row as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;
    stderr.flush()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_editor(
    stderr: &mut io::Stderr,
    color: bool,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    primary_prompt: &str,
    status_line: Option<&str>,
    buffer: &str,
    cursor: usize,
    matches: &[SlashCommand],
    selected: usize,
    previous_cursor_row: usize,
) -> io::Result<usize> {
    if previous_cursor_row > 0 {
        stderr.queue(MoveUp(previous_cursor_row as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;

    let cols = terminal_columns();
    let status_prefix_rows = if let Some(status_line) = status_line {
        write_status_line(
            stderr,
            color,
            status_line,
            settings::STATUS_LINE_NEWLINE_INTERACTIVE,
        )?;
        wrapped_rows(status_line, cols)
    } else {
        0
    };

    render_buffer(
        stderr,
        color,
        ssh_target,
        context_used_percent,
        prompt_mode,
        approval_prompt,
        buffer,
    )?;

    for (idx, cmd) in matches.iter().enumerate() {
        stderr.queue(Print("\r\n"))?;
        let marker = settings::suggestion_marker(idx == selected, color);
        if color {
            let marker_color = if idx == selected {
                settings::COLOR_AUTOCOMPLETE_SELECTED
            } else {
                settings::COLOR_AUTOCOMPLETE_UNSELECTED
            };
            stderr.queue(Print(settings::AUTOCOMPLETE_PREFIX))?;
            stderr.queue(PrintStyledContent(marker.with(marker_color)))?;
            stderr.queue(Print(" "))?;
            stderr.queue(PrintStyledContent(
                cmd.name.with(settings::COLOR_AUTOCOMPLETE_COMMAND).bold(),
            ))?;
            stderr.queue(Print(" "))?;
            stderr.queue(PrintStyledContent(
                cmd.description
                    .with(settings::COLOR_AUTOCOMPLETE_DESCRIPTION),
            ))?;
        } else {
            stderr.queue(Print(format!(
                "{}{marker} {} {}",
                settings::AUTOCOMPLETE_PREFIX,
                cmd.name,
                cmd.description
            )))?;
        }
    }

    let input_layout = compute_input_layout(buffer, cursor, cols, primary_prompt);
    let suggestion_rows = suggestion_rows(matches, selected, color, cols);
    let bottom_row = status_prefix_rows.saturating_add(
        input_layout
            .total_rows
            .saturating_sub(1)
            .saturating_add(suggestion_rows),
    );
    let cursor_row = status_prefix_rows.saturating_add(input_layout.cursor_row);
    let lines_to_move_up = bottom_row.saturating_sub(cursor_row);
    if lines_to_move_up > 0 {
        stderr.queue(MoveUp(lines_to_move_up as u16))?;
    }

    stderr.queue(MoveToColumn(input_layout.cursor_col as u16))?;
    stderr.flush()?;

    Ok(cursor_row)
}

fn finalize_editor(
    stderr: &mut io::Stderr,
    color: bool,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    buffer: &str,
    previous_cursor_row: usize,
) -> io::Result<()> {
    if previous_cursor_row > 0 {
        stderr.queue(MoveUp(previous_cursor_row as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;
    render_buffer(
        stderr,
        color,
        ssh_target,
        context_used_percent,
        prompt_mode,
        approval_prompt,
        buffer,
    )?;
    stderr.queue(Print("\r\n"))?;
    stderr.flush()?;
    Ok(())
}

fn render_buffer(
    stderr: &mut io::Stderr,
    color: bool,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    buffer: &str,
) -> io::Result<()> {
    let mut lines = buffer.split('\n');
    let first = lines.next().unwrap_or("");
    write_primary_prompt(
        stderr,
        color,
        ssh_target,
        context_used_percent,
        prompt_mode,
        approval_prompt,
    )?;
    stderr.queue(Print(first))?;

    for line in lines {
        stderr.queue(Print("\r\n"))?;
        write_continuation_prompt(stderr, color)?;
        stderr.queue(Print(line))?;
    }

    Ok(())
}

/// Raw mode lifetime guard so terminal state is restored on any return path.
struct RawModeGuard;

impl RawModeGuard {
    fn acquire() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_reader_can_be_interrupted_before_blocking() {
        let mut poll = || ReadPoll {
            interrupt: true,
            status_line: None,
        };
        let outcome =
            read_line_fallback(false, None, None, PromptMode::Normal, None, &mut poll).unwrap();
        assert_eq!(outcome, ReadOutcome::Interrupted);
    }
}
