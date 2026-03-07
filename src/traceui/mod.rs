//! Interactive trace viewer for Buddy JSONL runtime traces.
//!
//! The viewer keeps parsing/formatting logic independent from CLI wiring so it
//! can be extracted into a separate binary later without changing the core UI.

mod event;
mod stream;

use crate::traceui::event::TraceEvent;
use crate::traceui::stream::TraceEventSource;
use crate::ui::terminal::text::{clip_to_width, truncate_single_line, wrap_for_block};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEventKind};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{ExecutableCommand, QueueableCommand};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

/// CLI options for the interactive trace viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceUiOptions {
    /// Path to the JSONL trace file.
    pub file: PathBuf,
    /// Whether to keep watching for appended events.
    pub stream: bool,
}

/// Run the interactive trace UI until the user exits.
pub fn run(options: TraceUiOptions) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("traceui requires an interactive terminal".to_string());
    }

    let mut source = TraceEventSource::new(options.file.clone());
    let events = source.load_all()?;
    let mut state = TraceUiState::new(options.file, options.stream, events);
    let _guard = TerminalUiGuard::enter()?;
    let mut stderr = io::stderr();

    loop {
        if state.stream_enabled {
            match source.read_new() {
                Ok(new_events) => state.ingest(new_events),
                Err(err) => state.set_status(format!("stream error: {err}")),
            }
        }

        render(&mut stderr, &state).map_err(|err| format!("failed to render traceui: {err}"))?;

        let timeout = if state.stream_enabled {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(500)
        };
        if !ct_event::poll(timeout).map_err(|err| format!("failed to poll terminal: {err}"))? {
            continue;
        }
        match ct_event::read().map_err(|err| format!("failed to read terminal event: {err}"))? {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
                    continue;
                }
                if !handle_key(&mut state, key.code) {
                    break;
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    Ok(())
}

#[derive(Debug)]
struct TerminalUiGuard;

impl TerminalUiGuard {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|err| format!("failed to enable raw mode: {err}"))?;
        let mut stderr = io::stderr();
        stderr
            .execute(EnterAlternateScreen)
            .map_err(|err| format!("failed to enter alternate screen: {err}"))?;
        stderr
            .execute(Hide)
            .map_err(|err| format!("failed to hide cursor: {err}"))?;
        Ok(Self)
    }
}

impl Drop for TerminalUiGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stderr = io::stderr();
        let _ = stderr.execute(Show);
        let _ = stderr.execute(LeaveAlternateScreen);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Follow,
    Inspect,
}

#[derive(Debug)]
struct TraceUiState {
    file: PathBuf,
    stream_enabled: bool,
    mode: ViewMode,
    events: Vec<TraceEvent>,
    selected: usize,
    list_scroll: usize,
    detail_expanded: bool,
    pending_while_paused: usize,
    status: Option<String>,
}

impl TraceUiState {
    fn new(file: PathBuf, stream_enabled: bool, events: Vec<TraceEvent>) -> Self {
        let selected = events.len().saturating_sub(1);
        let mode = if stream_enabled {
            ViewMode::Follow
        } else {
            ViewMode::Inspect
        };
        Self {
            file,
            stream_enabled,
            mode,
            events,
            selected,
            list_scroll: 0,
            detail_expanded: false,
            pending_while_paused: 0,
            status: None,
        }
    }

    fn ingest(&mut self, new_events: Vec<TraceEvent>) {
        if new_events.is_empty() {
            return;
        }
        let added = new_events.len();
        self.events.extend(new_events);
        if self.mode == ViewMode::Follow {
            self.selected = self.events.len().saturating_sub(1);
            self.pending_while_paused = 0;
        } else {
            self.pending_while_paused = self.pending_while_paused.saturating_add(added);
        }
    }

    fn set_status(&mut self, status: String) {
        self.status = Some(status);
    }

    fn selected_event(&self) -> Option<&TraceEvent> {
        self.events.get(self.selected)
    }

    fn move_selection(&mut self, delta: isize) {
        if self.events.is_empty() {
            self.selected = 0;
            return;
        }
        self.mode = ViewMode::Inspect;
        let max = self.events.len().saturating_sub(1) as isize;
        let next = (self.selected as isize + delta).clamp(0, max) as usize;
        self.selected = next;
    }

    fn page_move(&mut self, delta: isize, viewport_rows: usize) {
        let jump = viewport_rows.max(1) as isize;
        self.move_selection(delta.saturating_mul(jump));
    }

    fn go_top(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.mode = ViewMode::Inspect;
        self.selected = 0;
    }

    fn go_bottom(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.selected = self.events.len().saturating_sub(1);
        self.pending_while_paused = 0;
        self.mode = if self.stream_enabled {
            ViewMode::Follow
        } else {
            ViewMode::Inspect
        };
    }

    fn resume_follow(&mut self) {
        if !self.stream_enabled {
            return;
        }
        self.mode = ViewMode::Follow;
        self.selected = self.events.len().saturating_sub(1);
        self.pending_while_paused = 0;
        self.status = Some("resumed follow mode".to_string());
    }

    fn toggle_expanded(&mut self) {
        self.detail_expanded = !self.detail_expanded;
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ViewMode::Follow => "follow",
            ViewMode::Inspect => "inspect",
        }
    }
}

fn handle_key(state: &mut TraceUiState, code: KeyCode) -> bool {
    let (_, rows) = terminal::size().unwrap_or((120, 40));
    let viewport_rows = rows.saturating_sub(4) as usize;
    match code {
        KeyCode::Char('q') => return false,
        KeyCode::Up | KeyCode::Char('k') => state.move_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => state.move_selection(1),
        KeyCode::PageUp | KeyCode::Char('b') => state.page_move(-1, viewport_rows / 2),
        KeyCode::PageDown | KeyCode::Char('f') => state.page_move(1, viewport_rows / 2),
        KeyCode::Home | KeyCode::Char('g') => state.go_top(),
        KeyCode::End | KeyCode::Char('G') => state.go_bottom(),
        KeyCode::Char(' ') => state.toggle_expanded(),
        KeyCode::Esc => state.resume_follow(),
        _ => {}
    }
    state.list_scroll = adjusted_list_scroll(state.selected, state.list_scroll, viewport_rows);
    true
}

fn render(stderr: &mut io::Stderr, state: &TraceUiState) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let cols = cols.max(80);
    let rows = rows.max(12);
    let list_width = (cols / 3).clamp(30, 48) as usize;
    let detail_width = cols as usize - list_width - 3;
    let body_rows = rows as usize - 4;

    let list_scroll = adjusted_list_scroll(state.selected, state.list_scroll, body_rows);

    stderr.queue(MoveTo(0, 0))?;
    stderr.queue(Clear(ClearType::All))?;

    // Header.
    write_colored_line(
        stderr,
        0,
        0,
        cols as usize,
        Color::Cyan,
        &format!(
            "buddy traceui  {}  events:{}  mode:{}  stream:{}",
            state.file.display(),
            state.events.len(),
            state.mode_label(),
            if state.stream_enabled { "on" } else { "off" }
        ),
        true,
    )?;
    let mut help =
        "j/k or arrows move  g/G home/end  b/f page  space expand  esc follow  q quit".to_string();
    if state.mode == ViewMode::Inspect && state.pending_while_paused > 0 {
        help.push_str(&format!(
            "  [{} new while paused]",
            state.pending_while_paused
        ));
    }
    write_colored_line(stderr, 0, 1, cols as usize, Color::DarkGrey, &help, false)?;

    // Column divider.
    for row in 2..rows.saturating_sub(1) {
        stderr.queue(MoveTo(list_width as u16 + 1, row))?;
        stderr.queue(SetForegroundColor(Color::DarkGrey))?;
        stderr.queue(Print("│"))?;
    }

    render_list(stderr, state, list_scroll, 2, list_width, body_rows)?;
    render_detail(stderr, state, list_width + 3, 2, detail_width, body_rows)?;

    let footer = state.status.clone().unwrap_or_else(|| {
        if let Some(event) = state.selected_event() {
            let time = event
                .ts_unix_ms
                .map(format_timestamp)
                .unwrap_or_else(|| "time:n/a".to_string());
            format!(
                "selected {}  {}  preview:{}",
                event.list_label(),
                time,
                if state.detail_expanded {
                    "full"
                } else {
                    "500 chars"
                }
            )
        } else {
            "no events loaded".to_string()
        }
    });
    write_colored_line(
        stderr,
        0,
        rows.saturating_sub(1),
        cols as usize,
        Color::DarkGrey,
        &footer,
        false,
    )?;

    stderr.queue(ResetColor)?;
    stderr.flush()
}

fn render_list(
    stderr: &mut io::Stderr,
    state: &TraceUiState,
    list_scroll: usize,
    start_row: u16,
    width: usize,
    max_rows: usize,
) -> io::Result<()> {
    for (row_idx, event_idx) in (list_scroll..state.events.len()).take(max_rows).enumerate() {
        let y = start_row + row_idx as u16;
        let event = &state.events[event_idx];
        let selected = event_idx == state.selected;
        let marker = if selected { '>' } else { ' ' };
        let tone = color_for_family(&event.family, event.parse_error);
        let title = truncate_single_line(&event.title, width.saturating_sub(4));
        let summary = truncate_single_line(&event.summary, width.saturating_sub(4));
        let composed = format!("{marker} {}  {}  {}", event.list_label(), title, summary);

        stderr.queue(MoveTo(0, y))?;
        if selected {
            stderr.queue(SetForegroundColor(Color::White))?;
            stderr.queue(SetAttribute(Attribute::Bold))?;
        } else {
            stderr.queue(SetForegroundColor(Color::DarkGrey))?;
            stderr.queue(SetAttribute(Attribute::Reset))?;
        }
        stderr.queue(SetForegroundColor(tone))?;
        stderr.queue(Print(clip_to_width(&composed, width)))?;
    }

    if state.events.is_empty() {
        write_colored_line(
            stderr,
            0,
            start_row,
            width,
            Color::DarkGrey,
            "no trace events found yet",
            false,
        )?;
    }

    Ok(())
}

fn render_detail(
    stderr: &mut io::Stderr,
    state: &TraceUiState,
    start_col: usize,
    start_row: u16,
    width: usize,
    max_rows: usize,
) -> io::Result<()> {
    let Some(event) = state.selected_event() else {
        write_colored_line(
            stderr,
            start_col as u16,
            start_row,
            width,
            Color::DarkGrey,
            "waiting for events",
            false,
        )?;
        return Ok(());
    };

    let header_color = color_for_family(&event.family, event.parse_error);
    write_colored_line(
        stderr,
        start_col as u16,
        start_row,
        width,
        header_color,
        &format!("{}", event.family_variant_label()),
        true,
    )?;
    write_colored_line(
        stderr,
        start_col as u16,
        start_row + 1,
        width,
        Color::White,
        &truncate_single_line(&event.title, width),
        true,
    )?;

    let mut lines = Vec::new();
    for line in event.detail(state.detail_expanded).lines() {
        let trimmed = if line.trim().is_empty() { "" } else { line };
        let wrapped = wrap_for_block(trimmed, width.max(1));
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }

    for (idx, line) in lines
        .into_iter()
        .take(max_rows.saturating_sub(2))
        .enumerate()
    {
        let y = start_row + 2 + idx as u16;
        let color = detail_line_color(&line, header_color);
        write_colored_line(stderr, start_col as u16, y, width, color, &line, false)?;
    }

    Ok(())
}

fn write_colored_line(
    stderr: &mut io::Stderr,
    x: u16,
    y: u16,
    width: usize,
    color: Color,
    text: &str,
    bold: bool,
) -> io::Result<()> {
    stderr.queue(MoveTo(x, y))?;
    stderr.queue(SetForegroundColor(color))?;
    if bold {
        stderr.queue(SetAttribute(Attribute::Bold))?;
    } else {
        stderr.queue(SetAttribute(Attribute::Reset))?;
    }
    let clipped = clip_to_width(text, width);
    stderr.queue(Print(clipped.clone()))?;
    let remainder = width.saturating_sub(clipped.chars().count());
    if remainder > 0 {
        stderr.queue(Print(" ".repeat(remainder)))?;
    }
    stderr.queue(SetAttribute(Attribute::Reset))?;
    stderr.queue(ResetColor)?;
    Ok(())
}

fn color_for_family(family: &str, parse_error: bool) -> Color {
    if parse_error {
        return Color::Red;
    }
    match family {
        "Lifecycle" => Color::Cyan,
        "Session" => Color::Green,
        "Task" => Color::Blue,
        "Model" => Color::Magenta,
        "Tool" => Color::Yellow,
        "Metrics" => Color::DarkCyan,
        "Warning" => Color::DarkYellow,
        "Error" => Color::Red,
        _ => Color::White,
    }
}

fn detail_line_color(line: &str, header_color: Color) -> Color {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        Color::White
    } else if !line.starts_with(' ') {
        header_color
    } else if trimmed.ends_with(':') {
        Color::DarkGrey
    } else {
        Color::White
    }
}

fn format_timestamp(ts_unix_ms: u64) -> String {
    let seconds = ts_unix_ms / 1000;
    let millis = ts_unix_ms % 1000;
    format!("t={}s.{:03}", seconds, millis)
}

fn adjusted_list_scroll(selected: usize, current_scroll: usize, viewport_rows: usize) -> usize {
    if viewport_rows == 0 {
        return 0;
    }
    if selected < current_scroll {
        return selected;
    }
    let end = current_scroll.saturating_add(viewport_rows);
    if selected >= end {
        return selected.saturating_sub(viewport_rows.saturating_sub(1));
    }
    current_scroll
}

#[cfg(test)]
mod tests {
    use super::{TraceEvent, TraceUiState, ViewMode};
    use std::path::PathBuf;

    fn event(seq: u64) -> TraceEvent {
        TraceEvent {
            line_no: seq as usize,
            seq: Some(seq),
            ts_unix_ms: Some(seq),
            family: "Tool".to_string(),
            variant: "result".to_string(),
            title: format!("event {seq}"),
            summary: format!("summary {seq}"),
            task_id: Some(1),
            iteration: None,
            session_id: None,
            detail_full: format!("full {seq}"),
            detail_preview: format!("preview {seq}"),
            parse_error: false,
        }
    }

    #[test]
    fn stream_follow_mode_tracks_latest_event() {
        let mut state = TraceUiState::new(PathBuf::from("trace.jsonl"), true, vec![event(1)]);
        state.ingest(vec![event(2), event(3)]);
        assert_eq!(state.mode, ViewMode::Follow);
        assert_eq!(state.selected, 2);
        assert_eq!(state.pending_while_paused, 0);
    }

    #[test]
    fn paused_mode_accumulates_pending_events_without_jumping() {
        let mut state = TraceUiState::new(PathBuf::from("trace.jsonl"), true, vec![event(1)]);
        state.move_selection(-1);
        state.ingest(vec![event(2), event(3)]);
        assert_eq!(state.mode, ViewMode::Inspect);
        assert_eq!(state.selected, 0);
        assert_eq!(state.pending_while_paused, 2);
    }

    #[test]
    fn escape_resumes_follow_mode() {
        let mut state = TraceUiState::new(PathBuf::from("trace.jsonl"), true, vec![event(1)]);
        state.mode = ViewMode::Inspect;
        state.pending_while_paused = 4;
        state.resume_follow();
        assert_eq!(state.mode, ViewMode::Follow);
        assert_eq!(state.pending_while_paused, 0);
        assert_eq!(state.selected, 0);
    }
}
