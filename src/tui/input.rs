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
use crate::tui::text::clip_to_width;
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
    /// User submitted a full line.
    Line(String),
    /// End-of-file (`Ctrl-D` on empty buffer / stdin EOF).
    Eof,
    /// User cancelled input (`Ctrl-C`).
    Cancelled,
    /// External interrupt requested by the poll callback.
    Interrupted,
}

/// Input-loop poll result (interrupt signal + optional status line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadPoll {
    /// Request that the read loop exits with `ReadOutcome::Interrupted`.
    pub interrupt: bool,
    /// Optional transient status line to draw above the prompt.
    pub status_line: Option<String>,
}

/// Present an interactive list picker and return the selected index.
///
/// In TTY mode, use arrow keys and Enter to select, or Esc to cancel.
/// In non-interactive mode, a numeric selection prompt is shown.
pub fn pick_from_list(
    color: bool,
    title: &str,
    help: &str,
    options: &[String],
    initial_selection: usize,
) -> io::Result<Option<usize>> {
    if options.is_empty() {
        return Ok(None);
    }

    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return pick_from_list_fallback(title, options);
    }

    pick_from_list_interactive(color, title, help, options, initial_selection)
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
    // Non-TTY path: probe once for interrupt/status, then block on stdin.
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
    // Interactive editor state uses character indices to preserve UTF-8 safety.
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
    let mut last_render_signature: Option<String> = None;
    let mut history_index = draft.history_index.filter(|idx| *idx < state.history_len());
    let mut history_draft = draft.history_draft;

    loop {
        let poll_state = poll();
        let matches = matching_slash_commands(&buffer);
        if selected >= matches.len() {
            selected = 0;
        }
        let signature = editor_render_signature(
            poll_state.status_line.as_deref(),
            &buffer,
            cursor,
            selected,
            &matches,
        );
        if last_render_signature.as_deref() != Some(signature.as_str()) {
            // Skip full repaint when nothing visual changed.
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
            last_render_signature = Some(signature);
        }

        if poll_state.interrupt {
            // Preserve the in-progress draft so caller interruptions are lossless.
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
                // Alt+Enter inserts a literal newline.
                insert_char_at_cursor(&mut buffer, &mut cursor, '\n');
                selected = 0;
                history_index = None;
            }
            KeyCode::Enter => {
                // Enter submits the current buffer.
                state.clear_draft(prompt_mode);
                finalize_editor(
                    &mut stderr,
                    PromptChrome {
                        color,
                        ssh_target,
                        context_used_percent,
                    },
                    prompt_mode,
                    approval_prompt,
                    &buffer,
                    previous_cursor_row,
                )?;
                return Ok(ReadOutcome::Line(buffer));
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl-D exits only when no text is present.
                if buffer.is_empty() {
                    state.clear_draft(prompt_mode);
                    finalize_editor(
                        &mut stderr,
                        PromptChrome {
                            color,
                            ssh_target,
                            context_used_percent,
                        },
                        prompt_mode,
                        approval_prompt,
                        "",
                        previous_cursor_row,
                    )?;
                    return Ok(ReadOutcome::Eof);
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl-C cancels the active entry.
                state.clear_draft(prompt_mode);
                finalize_editor(
                    &mut stderr,
                    PromptChrome {
                        color,
                        ssh_target,
                        context_used_percent,
                    },
                    prompt_mode,
                    approval_prompt,
                    "",
                    previous_cursor_row,
                )?;
                return Ok(ReadOutcome::Cancelled);
            }
            KeyCode::Tab => {
                // Tab accepts the selected slash-command suggestion.
                if let Some(choice) = matches.get(selected) {
                    buffer = choice.name.to_string();
                    cursor = char_count(&buffer);
                    selected = 0;
                    history_index = None;
                }
            }
            KeyCode::Up => {
                // Up cycles suggestions for slash commands, otherwise history.
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
                // Down mirrors Up navigation behavior.
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
                // Emacs-style kill-to-end-of-line.
                let end = line_end_char_index(&buffer, cursor);
                delete_char_range(&mut buffer, cursor, end);
                selected = 0;
                history_index = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Emacs-style kill-to-start-of-line.
                let start = line_start_char_index(&buffer, cursor);
                delete_char_range(&mut buffer, start, cursor);
                cursor = start;
                selected = 0;
                history_index = None;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Emacs-style backward-kill-word.
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
                // Ignore control/alt-modified printable keys.
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

fn pick_from_list_fallback(title: &str, options: &[String]) -> io::Result<Option<usize>> {
    eprintln!("• {title}");
    for (idx, option) in options.iter().enumerate() {
        eprintln!("  {}. {}", idx + 1, option);
    }
    eprint!("  pick (empty to cancel): ");
    io::stderr().flush()?;

    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        eprintln!();
        return Ok(None);
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let Ok(index) = trimmed.parse::<usize>() else {
        return Ok(None);
    };
    if index == 0 || index > options.len() {
        return Ok(None);
    }
    Ok(Some(index - 1))
}

/// Full-screen interactive picker used when stdin/stderr are terminals.
fn pick_from_list_interactive(
    color: bool,
    title: &str,
    help: &str,
    options: &[String],
    initial_selection: usize,
) -> io::Result<Option<usize>> {
    let _guard = RawModeGuard::acquire()?;
    let mut stderr = io::stderr();
    let mut selected = initial_selection.min(options.len().saturating_sub(1));
    let mut previous_rows = 0usize;

    loop {
        previous_rows = render_picker(
            &mut stderr,
            color,
            title,
            help,
            options,
            selected,
            previous_rows,
        )?;

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
            KeyCode::Up => {
                selected = if selected == 0 {
                    options.len().saturating_sub(1)
                } else {
                    selected - 1
                };
            }
            KeyCode::Down => {
                selected = (selected + 1) % options.len();
            }
            KeyCode::Enter => {
                clear_editor_surface(&mut stderr, previous_rows)?;
                return Ok(Some(selected));
            }
            KeyCode::Esc => {
                clear_editor_surface(&mut stderr, previous_rows)?;
                return Ok(None);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clear_editor_surface(&mut stderr, previous_rows)?;
                return Ok(None);
            }
            _ => {}
        }
    }
}

/// Draw picker chrome/options and return the last drawn row index.
fn render_picker(
    stderr: &mut io::Stderr,
    color: bool,
    title: &str,
    help: &str,
    options: &[String],
    selected: usize,
    previous_rows: usize,
) -> io::Result<usize> {
    if previous_rows > 0 {
        stderr.queue(MoveUp(previous_rows as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;

    let cols = terminal_columns();
    let mut total_rows = 0usize;
    let title_plain = format!("• {title}");
    total_rows += wrapped_rows(&title_plain, cols);
    if color {
        stderr.queue(PrintStyledContent(
            "•".with(settings::color_section_bullet()),
        ))?;
        stderr.queue(Print(" "))?;
        stderr.queue(PrintStyledContent(
            title.with(settings::color_section_title()).bold(),
        ))?;
    } else {
        stderr.queue(Print(&title_plain))?;
    }
    let help_plain = format!("  {help}");
    stderr.queue(Print("\r\n"))?;
    total_rows += wrapped_rows(&help_plain, cols);
    if color {
        stderr.queue(PrintStyledContent(
            help_plain.as_str().with(settings::color_field_key()),
        ))?;
    } else {
        stderr.queue(Print(&help_plain))?;
    }

    for (idx, option) in options.iter().enumerate() {
        let active = idx == selected;
        let marker = if active { "▶" } else { "·" };
        let line_plain = format!("  {marker} {}", option);
        stderr.queue(Print("\r\n"))?;
        total_rows += wrapped_rows(&line_plain, cols);
        if color {
            let marker_color = if active {
                settings::color_autocomplete_selected()
            } else {
                settings::color_autocomplete_unselected()
            };
            let text_color = if active {
                settings::color_autocomplete_command()
            } else {
                settings::color_field_value()
            };
            stderr.queue(Print("  "))?;
            stderr.queue(PrintStyledContent(marker.with(marker_color)))?;
            stderr.queue(Print(" "))?;
            stderr.queue(PrintStyledContent(option.as_str().with(text_color)))?;
        } else {
            stderr.queue(Print(&line_plain))?;
        }
    }

    stderr.flush()?;
    Ok(total_rows.saturating_sub(1))
}

/// Clear rows previously painted by the interactive editor.
fn clear_editor_surface(stderr: &mut io::Stderr, previous_cursor_row: usize) -> io::Result<()> {
    if previous_cursor_row > 0 {
        stderr.queue(MoveUp(previous_cursor_row as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;
    stderr.flush()?;
    Ok(())
}

/// Build a lightweight render-state signature for redraw suppression.
fn editor_render_signature(
    status_line: Option<&str>,
    buffer: &str,
    cursor: usize,
    selected: usize,
    matches: &[SlashCommand],
) -> String {
    let mut signature = String::new();
    signature.push_str(status_line.unwrap_or_default());
    signature.push('|');
    signature.push_str(buffer);
    signature.push('|');
    signature.push_str(&cursor.to_string());
    signature.push('|');
    signature.push_str(&selected.to_string());
    signature.push('|');
    for cmd in matches {
        signature.push_str(cmd.name);
        signature.push(',');
    }
    signature
}

/// Flatten and clip a status line so it always fits on one terminal row.
fn single_line_status(status_line: &str, cols: usize) -> String {
    let flat = status_line.replace('\n', " ");
    if cols == 0 {
        return flat;
    }
    if flat.chars().count() <= cols {
        return flat;
    }
    if cols <= 3 {
        return clip_to_width(&flat, cols);
    }
    format!("{}...", clip_to_width(&flat, cols - 3))
}

#[allow(clippy::too_many_arguments)]
/// Render status/prompt/input/suggestions and restore cursor to edit position.
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
    // Rendering walkthrough:
    // 1) clear the previous frame,
    // 2) draw status + prompt + buffer,
    // 3) draw autocomplete rows,
    // 4) move cursor back to the buffer location.
    if previous_cursor_row > 0 {
        stderr.queue(MoveUp(previous_cursor_row as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;

    let cols = terminal_columns();
    let status_prefix_rows: usize = if let Some(status_line) = status_line {
        let display = single_line_status(status_line, cols);
        write_status_line(
            stderr,
            color,
            &display,
            settings::STATUS_LINE_NEWLINE_INTERACTIVE,
        )?;
        1
    } else {
        0
    };

    render_buffer(
        stderr,
        PromptChrome {
            color,
            ssh_target,
            context_used_percent,
        },
        prompt_mode,
        approval_prompt,
        buffer,
    )?;

    for (idx, cmd) in matches.iter().enumerate() {
        stderr.queue(Print("\r\n"))?;
        let marker = settings::suggestion_marker(idx == selected, color);
        if color {
            let marker_color = if idx == selected {
                settings::color_autocomplete_selected()
            } else {
                settings::color_autocomplete_unselected()
            };
            stderr.queue(Print(settings::AUTOCOMPLETE_PREFIX))?;
            stderr.queue(PrintStyledContent(marker.with(marker_color)))?;
            stderr.queue(Print(" "))?;
            stderr.queue(PrintStyledContent(
                cmd.name.with(settings::color_autocomplete_command()).bold(),
            ))?;
            stderr.queue(Print(" "))?;
            stderr.queue(PrintStyledContent(
                cmd.description
                    .with(settings::color_autocomplete_description()),
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
    chrome: PromptChrome<'_>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    buffer: &str,
    previous_cursor_row: usize,
) -> io::Result<()> {
    // Redraw a clean final frame and append a newline before returning.
    if previous_cursor_row > 0 {
        stderr.queue(MoveUp(previous_cursor_row as u16))?;
    }
    stderr.queue(MoveToColumn(0))?;
    stderr.queue(Clear(ClearType::FromCursorDown))?;
    render_buffer(stderr, chrome, prompt_mode, approval_prompt, buffer)?;
    stderr.queue(Print("\r\n"))?;
    stderr.flush()?;
    Ok(())
}

#[derive(Clone, Copy)]
struct PromptChrome<'a> {
    /// Whether colored prompt styling is enabled.
    color: bool,
    /// Optional SSH target label shown in the normal prompt.
    ssh_target: Option<&'a str>,
    /// Optional context percentage shown near the prompt.
    context_used_percent: Option<u16>,
}

/// Render prompt chrome plus multiline buffer text.
fn render_buffer(
    stderr: &mut io::Stderr,
    chrome: PromptChrome<'_>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
    buffer: &str,
) -> io::Result<()> {
    let mut lines = buffer.split('\n');
    let first = lines.next().unwrap_or("");
    write_primary_prompt(
        stderr,
        chrome.color,
        chrome.ssh_target,
        chrome.context_used_percent,
        prompt_mode,
        approval_prompt,
    )?;
    stderr.queue(Print(first))?;

    for line in lines {
        stderr.queue(Print("\r\n"))?;
        write_continuation_prompt(stderr, chrome.color)?;
        stderr.queue(Print(line))?;
    }

    Ok(())
}

/// Raw mode lifetime guard so terminal state is restored on any return path.
struct RawModeGuard;

impl RawModeGuard {
    /// Enable terminal raw mode and return a guard that disables it on drop.
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
        // Non-interactive path should honor interrupts before reading stdin.
        let mut poll = || ReadPoll {
            interrupt: true,
            status_line: None,
        };
        let outcome =
            read_line_fallback(false, None, None, PromptMode::Normal, None, &mut poll).unwrap();
        assert_eq!(outcome, ReadOutcome::Interrupted);
    }

    #[test]
    fn single_line_status_truncates_to_terminal_width() {
        // Long status lines should truncate with an ellipsis.
        assert_eq!(single_line_status("abcdef", 4), "a...");
        assert_eq!(single_line_status("abc", 4), "abc");
    }

    #[test]
    fn render_signature_changes_with_status_and_selection() {
        // Signature changes whenever a render-relevant field changes.
        let matches = [SlashCommand {
            name: "/model",
            description: "model switch",
        }];
        let a = editor_render_signature(Some("a"), "/model", 6, 0, &matches);
        let b = editor_render_signature(Some("b"), "/model", 6, 0, &matches);
        let c = editor_render_signature(Some("a"), "/model", 6, 1, &matches);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
