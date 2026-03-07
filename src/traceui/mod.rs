//! Interactive trace viewer for Buddy JSONL runtime traces.
//!
//! The viewer keeps parsing/formatting logic independent from CLI wiring so it
//! can be extracted into a separate binary later without changing the core UI.

mod event;
mod stream;

use crate::traceui::event::TraceEvent;
use crate::traceui::stream::TraceEventSource;
use crate::ui::terminal::text::{clip_to_width, truncate_single_line};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
    let mut renderer = TraceUiRenderer::default();

    loop {
        if state.stream_enabled {
            match source.read_new() {
                Ok(new_events) => state.ingest(new_events),
                Err(err) => state.set_status(format!("stream error: {err}")),
            }
        }

        if state.needs_redraw {
            render(&mut stderr, &mut renderer, &state)
                .map_err(|err| format!("failed to render traceui: {err}"))?;
            state.needs_redraw = false;
        }

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
                if !handle_key(&mut state, key) {
                    break;
                }
            }
            Event::Resize(_, _) => state.mark_dirty(),
            _ => {}
        }
    }

    Ok(())
}

/// Raw-mode / alternate-screen lifetime guard for the trace viewer.
#[derive(Debug)]
struct TerminalUiGuard;

impl TerminalUiGuard {
    /// Enter the interactive terminal mode needed by the viewer.
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

/// Whether the stream stays pinned to the latest event or the operator is browsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Follow,
    Inspect,
}

/// Mutable interaction state for the trace viewer.
#[derive(Debug)]
struct TraceUiState {
    file: PathBuf,
    stream_enabled: bool,
    mode: ViewMode,
    events: Vec<TraceEvent>,
    selected: usize,
    list_scroll: usize,
    detail_scroll: usize,
    pending_while_paused: usize,
    status: Option<String>,
    needs_redraw: bool,
}

impl TraceUiState {
    /// Build initial viewer state from the loaded trace contents.
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
            detail_scroll: 0,
            pending_while_paused: 0,
            status: None,
            needs_redraw: true,
        }
    }

    /// Append freshly tailed events and preserve operator browsing mode.
    fn ingest(&mut self, new_events: Vec<TraceEvent>) {
        if new_events.is_empty() {
            return;
        }
        let added = new_events.len();
        self.events.extend(new_events);
        if self.mode == ViewMode::Follow {
            self.selected = self.events.len().saturating_sub(1);
            self.detail_scroll = 0;
            self.pending_while_paused = 0;
        } else {
            self.pending_while_paused = self.pending_while_paused.saturating_add(added);
        }
        self.needs_redraw = true;
    }

    /// Update the footer status message only when it actually changed.
    fn set_status(&mut self, status: String) {
        if self.status.as_deref() != Some(status.as_str()) {
            self.status = Some(status);
            self.needs_redraw = true;
        }
    }

    /// Clear one-shot status text so the footer falls back to selection metadata.
    fn clear_status(&mut self) {
        if self.status.take().is_some() {
            self.needs_redraw = true;
        }
    }

    /// Mark the current screen as stale.
    fn mark_dirty(&mut self) {
        self.needs_redraw = true;
    }

    /// Fetch the currently selected event.
    fn selected_event(&self) -> Option<&TraceEvent> {
        self.events.get(self.selected)
    }

    /// Move the selected event and reset detail scroll to the top of the new event.
    fn move_selection(&mut self, delta: isize) {
        if self.events.is_empty() {
            self.selected = 0;
            return;
        }
        let mode_changed = self.mode != ViewMode::Inspect;
        self.mode = ViewMode::Inspect;
        let max = self.events.len().saturating_sub(1) as isize;
        let next = (self.selected as isize + delta).clamp(0, max) as usize;
        if next != self.selected || mode_changed {
            if next != self.selected {
                self.selected = next;
                self.detail_scroll = 0;
            }
            self.clear_status();
            self.needs_redraw = true;
        }
    }

    /// Page through the event list by roughly half a screen.
    fn page_move(&mut self, delta: isize, viewport_rows: usize) {
        let jump = viewport_rows.max(1) as isize;
        self.move_selection(delta.saturating_mul(jump));
    }

    /// Jump to the first event.
    fn go_top(&mut self) {
        if self.events.is_empty() {
            return;
        }
        let mode_changed = self.mode != ViewMode::Inspect;
        self.mode = ViewMode::Inspect;
        if self.selected != 0 || mode_changed {
            self.selected = 0;
            self.detail_scroll = 0;
            self.clear_status();
            self.needs_redraw = true;
        }
    }

    /// Jump to the latest event and resume follow mode if streaming is active.
    fn go_bottom(&mut self) {
        if self.events.is_empty() {
            return;
        }
        let next = self.events.len().saturating_sub(1);
        let changed =
            self.selected != next || self.mode != ViewMode::Follow || self.detail_scroll != 0;
        self.selected = next;
        self.detail_scroll = 0;
        self.pending_while_paused = 0;
        self.mode = if self.stream_enabled {
            ViewMode::Follow
        } else {
            ViewMode::Inspect
        };
        if changed {
            self.clear_status();
            self.needs_redraw = true;
        }
    }

    /// Resume live-follow mode without disrupting the existing screen otherwise.
    fn resume_follow(&mut self) {
        if !self.stream_enabled {
            return;
        }
        self.mode = ViewMode::Follow;
        self.selected = self.events.len().saturating_sub(1);
        self.detail_scroll = 0;
        self.pending_while_paused = 0;
        self.status = Some("resumed follow mode".to_string());
        self.needs_redraw = true;
    }

    /// Scroll the detail pane by a line delta, clamped to visible content.
    fn scroll_detail_lines(&mut self, delta: isize, viewport_rows: usize, detail_width: usize) {
        let max = self.detail_max_scroll(viewport_rows, detail_width);
        let next = (self.detail_scroll as isize + delta).clamp(0, max as isize) as usize;
        let mode_changed = self.mode != ViewMode::Inspect;
        if next != self.detail_scroll || mode_changed {
            self.mode = ViewMode::Inspect;
            self.detail_scroll = next;
            self.clear_status();
            self.needs_redraw = true;
        }
    }

    /// Scroll the detail pane by a larger page-sized delta.
    fn page_detail(&mut self, delta: isize, viewport_rows: usize, detail_width: usize) {
        let jump = viewport_rows.max(1) as isize / 2;
        self.scroll_detail_lines(
            delta.saturating_mul(jump.max(1)),
            viewport_rows,
            detail_width,
        );
    }

    /// Human-readable viewer mode for the header line.
    fn mode_label(&self) -> &'static str {
        match self.mode {
            ViewMode::Follow => "follow",
            ViewMode::Inspect => "inspect",
        }
    }

    /// Maximum vertical scroll offset for the selected event detail pane.
    fn detail_max_scroll(&self, viewport_rows: usize, detail_width: usize) -> usize {
        let Some(event) = self.selected_event() else {
            return 0;
        };
        let visible = viewport_rows.saturating_sub(2);
        if visible == 0 {
            return 0;
        }
        detail_body_lines(event, detail_width)
            .len()
            .saturating_sub(visible)
    }
}

/// One styled text span inside a rendered terminal row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StyledSpan {
    text: String,
    color: Color,
    bold: bool,
}

impl StyledSpan {
    /// Create a styled span from one owned string.
    fn new(text: impl Into<String>, color: Color, bold: bool) -> Self {
        Self {
            text: text.into(),
            color,
            bold,
        }
    }
}

/// One terminal row composed from multiple styled spans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StyledLine {
    spans: Vec<StyledSpan>,
}

impl StyledLine {
    /// Build a single-span line.
    fn plain(text: impl Into<String>, color: Color, bold: bool) -> Self {
        Self {
            spans: vec![StyledSpan::new(text, color, bold)],
        }
    }

    /// Append one span when it contains visible content.
    fn push(&mut self, text: impl Into<String>, color: Color, bold: bool) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.spans.push(StyledSpan::new(text, color, bold));
    }
}

/// Clip one styled line to a fixed width and pad the remainder.
fn fit_styled_line(line: &StyledLine, width: usize, fill_color: Color) -> StyledLine {
    let mut fitted = StyledLine::default();
    let mut used = 0usize;
    for span in &line.spans {
        if used >= width {
            break;
        }
        let remaining = width.saturating_sub(used);
        let clipped = clip_to_width(&span.text, remaining);
        if clipped.is_empty() {
            continue;
        }
        used = used.saturating_add(clipped.chars().count());
        fitted.push(clipped, span.color, span.bold);
    }
    if used < width {
        fitted.push(" ".repeat(width - used), fill_color, false);
    }
    fitted
}

/// Concrete frame snapshot used for diff-based terminal repainting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderFrame {
    cols: u16,
    rows: u16,
    lines: Vec<StyledLine>,
}

/// Renderer cache that suppresses redundant row repaints.
#[derive(Debug, Default)]
struct TraceUiRenderer {
    last_frame: Option<RenderFrame>,
}

/// Handle one keyboard event and report whether the viewer should continue.
fn handle_key(state: &mut TraceUiState, key: KeyEvent) -> bool {
    let (cols, rows) = terminal::size().unwrap_or((120, 40));
    let (_, detail_width, body_rows) = layout(cols, rows);
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) => return false,
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => state.move_selection(-1),
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => state.move_selection(1),
        (KeyCode::PageUp, _) | (KeyCode::Char('b'), _) => state.page_move(-1, body_rows / 2),
        (KeyCode::PageDown, _) | (KeyCode::Char('f'), _) => state.page_move(1, body_rows / 2),
        (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => state.go_top(),
        (KeyCode::End, _) | (KeyCode::Char('G'), _) => state.go_bottom(),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            state.page_detail(-1, body_rows, detail_width)
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            state.page_detail(1, body_rows, detail_width)
        }
        (KeyCode::Char('u'), _) | (KeyCode::Char('U'), _) => {
            state.page_detail(-1, body_rows, detail_width)
        }
        (KeyCode::Char('d'), _) | (KeyCode::Char('D'), _) => {
            state.page_detail(1, body_rows, detail_width)
        }
        (KeyCode::Left, _) => state.scroll_detail_lines(-1, body_rows, detail_width),
        (KeyCode::Right, _) => state.scroll_detail_lines(1, body_rows, detail_width),
        (KeyCode::Esc, _) => state.resume_follow(),
        _ => {}
    }
    state.list_scroll = adjusted_list_scroll(state.selected, state.list_scroll, body_rows);
    true
}

/// Build a frame snapshot and apply only the rows that changed since the last draw.
fn render(
    stderr: &mut io::Stderr,
    renderer: &mut TraceUiRenderer,
    state: &TraceUiState,
) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let frame = build_frame(state, cols.max(80), rows.max(12));
    paint_frame(stderr, renderer, &frame)
}

/// Convert the current viewer state into a complete styled frame.
fn build_frame(state: &TraceUiState, cols: u16, rows: u16) -> RenderFrame {
    let (list_width, detail_width, body_rows) = layout(cols, rows);
    let list_scroll = adjusted_list_scroll(state.selected, state.list_scroll, body_rows);
    let detail_scroll = state
        .detail_scroll
        .min(state.detail_max_scroll(body_rows, detail_width));

    let mut lines = vec![StyledLine::default(); rows as usize];
    lines[0] = StyledLine::plain(
        format!(
            "buddy traceui  {}  events:{}  mode:{}  stream:{}",
            state.file.display(),
            state.events.len(),
            state.mode_label(),
            if state.stream_enabled { "on" } else { "off" }
        ),
        Color::Cyan,
        true,
    );

    let mut help = "j/k arrows move  g/G home/end  b/f page  u/d detail scroll  esc follow  q quit"
        .to_string();
    if state.mode == ViewMode::Inspect && state.pending_while_paused > 0 {
        help.push_str(&format!(
            "  [{} new while paused]",
            state.pending_while_paused
        ));
    }
    lines[1] = StyledLine::plain(help, Color::DarkGrey, false);

    let list_lines = list_panel_lines(state, list_scroll, list_width, body_rows);
    let detail_lines = detail_panel_lines(state, detail_width, body_rows, detail_scroll);
    for row_idx in 0..body_rows {
        let target_row = row_idx + 2;
        if target_row >= lines.len().saturating_sub(1) {
            break;
        }
        let mut line = StyledLine::default();
        if let Some(left) = list_lines.get(row_idx) {
            line.spans
                .extend(fit_styled_line(left, list_width, Color::White).spans);
        } else {
            line.push(" ".repeat(list_width), Color::White, false);
        }
        line.push(" ", Color::DarkGrey, false);
        line.push("│", Color::DarkGrey, false);
        line.push(" ", Color::DarkGrey, false);
        if let Some(right) = detail_lines.get(row_idx) {
            line.spans
                .extend(fit_styled_line(right, detail_width, Color::White).spans);
        } else {
            line.push(" ".repeat(detail_width), Color::White, false);
        }
        lines[target_row] = line;
    }

    let footer = state.status.clone().unwrap_or_else(|| {
        if let Some(event) = state.selected_event() {
            let time = event
                .ts_unix_ms
                .map(format_timestamp)
                .unwrap_or_else(|| "time:n/a".to_string());
            format!(
                "selected {}  {}  detail:{}/{}",
                event.list_label(),
                time,
                detail_scroll,
                state.detail_max_scroll(body_rows, detail_width)
            )
        } else {
            "no events loaded".to_string()
        }
    });
    lines[rows.saturating_sub(1) as usize] = StyledLine::plain(footer, Color::DarkGrey, false);

    RenderFrame { cols, rows, lines }
}

/// Compute stable panel dimensions for the current terminal size.
fn layout(cols: u16, rows: u16) -> (usize, usize, usize) {
    let list_width = (cols / 3).clamp(30, 48) as usize;
    let detail_width = cols as usize - list_width - 3;
    let body_rows = rows.saturating_sub(4) as usize;
    (list_width, detail_width, body_rows)
}

/// Paint only the changed rows for the current frame.
fn paint_frame(
    stderr: &mut io::Stderr,
    renderer: &mut TraceUiRenderer,
    frame: &RenderFrame,
) -> io::Result<()> {
    let size_changed = renderer
        .last_frame
        .as_ref()
        .map(|last| last.cols != frame.cols || last.rows != frame.rows)
        .unwrap_or(true);
    if size_changed {
        stderr.queue(MoveTo(0, 0))?;
        stderr.queue(Clear(ClearType::All))?;
    }

    for (row, line) in frame.lines.iter().enumerate() {
        let changed = size_changed
            || renderer
                .last_frame
                .as_ref()
                .and_then(|last| last.lines.get(row))
                != Some(line);
        if changed {
            draw_line(stderr, row as u16, frame.cols as usize, line)?;
        }
    }

    stderr.queue(ResetColor)?;
    stderr.queue(SetAttribute(Attribute::Reset))?;
    stderr.flush()?;
    renderer.last_frame = Some(frame.clone());
    Ok(())
}

/// Render one styled terminal row with clipping and padding.
fn draw_line(stderr: &mut io::Stderr, row: u16, width: usize, line: &StyledLine) -> io::Result<()> {
    stderr.queue(MoveTo(0, row))?;
    stderr.queue(Clear(ClearType::CurrentLine))?;

    let mut used = 0usize;
    for span in &line.spans {
        if used >= width {
            break;
        }
        let remaining = width.saturating_sub(used);
        let clipped = clip_to_width(&span.text, remaining);
        if clipped.is_empty() {
            continue;
        }
        stderr.queue(SetForegroundColor(span.color))?;
        stderr.queue(SetAttribute(if span.bold {
            Attribute::Bold
        } else {
            Attribute::Reset
        }))?;
        stderr.queue(Print(clipped.clone()))?;
        used = used.saturating_add(clipped.chars().count());
    }

    if used < width {
        stderr.queue(SetForegroundColor(Color::White))?;
        stderr.queue(SetAttribute(Attribute::Reset))?;
        stderr.queue(Print(" ".repeat(width - used)))?;
    }
    stderr.queue(ResetColor)?;
    stderr.queue(SetAttribute(Attribute::Reset))?;
    Ok(())
}

/// Build the compact left-panel event summary list.
fn list_panel_lines(
    state: &TraceUiState,
    list_scroll: usize,
    width: usize,
    max_rows: usize,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    for event_idx in list_scroll..state.events.len() {
        if lines.len() >= max_rows {
            break;
        }
        let event = &state.events[event_idx];
        let selected = event_idx == state.selected;
        let tone = color_for_family(&event.family, event.parse_error);
        let marker = if selected { ">" } else { " " };
        let label = truncate_single_line(&event.list_label(), width.saturating_sub(2));
        let title = truncate_single_line(&event.title, width.saturating_sub(2));
        let summary = truncate_single_line(&event.summary, width.saturating_sub(2));

        let mut line = StyledLine::default();
        line.push(
            marker,
            if selected {
                Color::White
            } else {
                Color::DarkGrey
            },
            selected,
        );
        line.push(" ", Color::DarkGrey, false);
        line.push(label, tone, true);
        line.push("  ", Color::DarkGrey, false);
        line.push(
            title,
            if selected { Color::White } else { Color::Grey },
            selected,
        );
        line.push("  ", Color::DarkGrey, false);
        line.push(summary, Color::DarkGrey, false);
        lines.push(line);
    }

    if lines.is_empty() {
        lines.push(StyledLine::plain(
            "no trace events found yet",
            Color::DarkGrey,
            false,
        ));
    }

    lines
}

/// Build the detail pane rows for the selected event and current vertical scroll.
fn detail_panel_lines(
    state: &TraceUiState,
    width: usize,
    max_rows: usize,
    detail_scroll: usize,
) -> Vec<StyledLine> {
    let Some(event) = state.selected_event() else {
        return vec![StyledLine::plain(
            "waiting for events",
            Color::DarkGrey,
            false,
        )];
    };

    let header_color = color_for_family(&event.family, event.parse_error);
    let mut lines = vec![
        StyledLine::plain(event.family_variant_label(), header_color, true),
        StyledLine::plain(
            truncate_single_line(&event.title, width),
            Color::White,
            true,
        ),
    ];
    let body = detail_body_lines(event, width);
    let visible = max_rows.saturating_sub(2);
    for line in body.into_iter().skip(detail_scroll).take(visible) {
        lines.push(line);
    }
    lines
}

/// Parse and wrap one event detail block into styled rows for the right pane.
fn detail_body_lines(event: &TraceEvent, width: usize) -> Vec<StyledLine> {
    let header_color = color_for_family(&event.family, event.parse_error);
    let mut lines = Vec::new();
    for raw_line in event.detail_full.lines() {
        let styled = style_detail_line(raw_line, header_color);
        lines.extend(wrap_styled_line(&styled, width.max(1)));
    }
    if lines.is_empty() {
        lines.push(StyledLine::plain("", Color::White, false));
    }
    lines
}

/// Apply semantic coloring to one structured detail line.
fn style_detail_line(line: &str, header_color: Color) -> StyledLine {
    let trimmed = line.trim_start();
    let indent = line.len().saturating_sub(trimmed.len());
    let indent_text = " ".repeat(indent);

    if trimmed.is_empty() {
        return StyledLine::plain("", Color::White, false);
    }
    if indent == 0 {
        return StyledLine::plain(trimmed, header_color, true);
    }
    if let Some(rest) = trimmed.strip_prefix("- ") {
        let mut styled = StyledLine::default();
        styled.push(indent_text, Color::DarkGrey, false);
        styled.push("- ", Color::DarkGrey, false);
        append_value_segments(&mut styled, rest.trim());
        return styled;
    }
    if trimmed == "-" {
        let mut styled = StyledLine::default();
        styled.push(indent_text, Color::DarkGrey, false);
        styled.push("-", Color::DarkGrey, false);
        return styled;
    }
    if let Some((key, value)) = trimmed.split_once(':') {
        let mut styled = StyledLine::default();
        styled.push(indent_text, Color::DarkGrey, false);
        styled.push(key, Color::Cyan, true);
        styled.push(":", Color::DarkGrey, false);
        if value.is_empty() {
            return styled;
        }
        styled.push(" ", Color::DarkGrey, false);
        append_value_segments(&mut styled, value.trim_start());
        return styled;
    }

    let mut styled = StyledLine::default();
    styled.push(indent_text, Color::DarkGrey, false);
    append_value_segments(&mut styled, trimmed);
    styled
}

/// Append a scalar-ish value with type-aware colors to a detail line.
fn append_value_segments(line: &mut StyledLine, value: &str) {
    let (color, bold) = classify_value_style(value);
    line.push(value, color, bold);
}

/// Choose a terminal style for a rendered scalar value.
fn classify_value_style(value: &str) -> (Color, bool) {
    if value.starts_with('"') && value.ends_with('"') {
        (Color::Green, false)
    } else if value == "null" {
        (Color::DarkGrey, false)
    } else if value == "true" || value == "false" {
        (Color::Magenta, true)
    } else if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        (Color::Yellow, false)
    } else if value.starts_with('[') || value.starts_with('{') {
        (Color::DarkCyan, false)
    } else {
        (Color::White, false)
    }
}

/// Hard-wrap one styled line while preserving span colors and weights.
fn wrap_styled_line(line: &StyledLine, width: usize) -> Vec<StyledLine> {
    if width == 0 {
        return Vec::new();
    }
    if line.spans.is_empty() {
        return vec![StyledLine::plain("", Color::White, false)];
    }

    let mut wrapped = Vec::new();
    let mut current = StyledLine::default();
    let mut current_width = 0usize;

    for span in &line.spans {
        for ch in span.text.chars() {
            if current_width == width {
                wrapped.push(current);
                current = StyledLine::default();
                current_width = 0;
            }
            push_wrapped_char(&mut current, ch, span.color, span.bold);
            current_width += 1;
        }
    }

    if !current.spans.is_empty() {
        wrapped.push(current);
    }
    if wrapped.is_empty() {
        wrapped.push(StyledLine::plain("", Color::White, false));
    }
    wrapped
}

/// Append one wrapped character, coalescing adjacent spans with the same style.
fn push_wrapped_char(line: &mut StyledLine, ch: char, color: Color, bold: bool) {
    if let Some(last) = line.spans.last_mut() {
        if last.color == color && last.bold == bold {
            last.text.push(ch);
            return;
        }
    }
    line.spans
        .push(StyledSpan::new(ch.to_string(), color, bold));
}

/// Pick the family color used across the event list and right-pane headers.
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

/// Format raw trace timestamps without bringing in a time dependency.
fn format_timestamp(ts_unix_ms: u64) -> String {
    let seconds = ts_unix_ms / 1000;
    let millis = ts_unix_ms % 1000;
    format!("t={}s.{:03}", seconds, millis)
}

/// Keep the selected event visible inside the left-panel viewport.
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
    use super::{
        build_frame, detail_body_lines, TraceEvent, TraceUiRenderer, TraceUiState, ViewMode,
    };
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
            detail_full: format!(
                "Event\n  payload:\n    line: {seq}\n    text: \"{}\"",
                "x".repeat(160)
            ),
            detail_preview: format!("preview {seq}"),
            parse_error: false,
        }
    }

    fn styled_line_to_plain(line: &super::StyledLine) -> String {
        line.spans.iter().map(|span| span.text.as_str()).collect()
    }

    #[test]
    fn stream_follow_mode_tracks_latest_event() {
        let mut state = TraceUiState::new(PathBuf::from("trace.jsonl"), true, vec![event(1)]);
        state.ingest(vec![event(2), event(3)]);
        assert_eq!(state.mode, ViewMode::Follow);
        assert_eq!(state.selected, 2);
        assert_eq!(state.pending_while_paused, 0);
        assert_eq!(state.detail_scroll, 0);
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
        assert_eq!(state.detail_scroll, 0);
    }

    #[test]
    fn detail_scroll_moves_without_changing_selection() {
        let mut state = TraceUiState::new(PathBuf::from("trace.jsonl"), false, vec![event(1)]);
        let before = state.selected;
        state.page_detail(1, 6, 24);
        assert_eq!(state.selected, before);
        assert!(state.detail_scroll > 0);
    }

    #[test]
    fn detail_scroll_resets_on_selection_change() {
        let mut state = TraceUiState::new(
            PathBuf::from("trace.jsonl"),
            false,
            vec![event(1), event(2), event(3)],
        );
        state.detail_scroll = 10;
        state.move_selection(-1);
        assert_eq!(state.detail_scroll, 0);
    }

    #[test]
    fn boundary_navigation_still_exits_follow_mode() {
        let mut state = TraceUiState::new(PathBuf::from("trace.jsonl"), true, vec![event(1)]);
        state.move_selection(-1);
        assert_eq!(state.mode, ViewMode::Inspect);
        assert!(state.needs_redraw);
    }

    #[test]
    fn detail_body_wraps_long_content_into_multiple_rows() {
        let lines = detail_body_lines(&event(9), 24);
        assert!(lines.len() > 4);
    }

    #[test]
    fn frame_help_mentions_detail_scroll_and_not_space_expand() {
        let state = TraceUiState::new(PathBuf::from("trace.jsonl"), true, vec![event(1)]);
        let frame = build_frame(&state, 120, 24);
        let help = styled_line_to_plain(&frame.lines[1]);
        assert!(help.contains("detail scroll"));
        assert!(!help.contains("space expand"));
    }

    #[test]
    fn renderer_skips_unchanged_rows() {
        let mut renderer = TraceUiRenderer::default();
        let state = TraceUiState::new(PathBuf::from("trace.jsonl"), false, vec![event(1)]);
        let frame = build_frame(&state, 100, 20);
        renderer.last_frame = Some(frame.clone());
        assert_eq!(renderer.last_frame.as_ref(), Some(&frame));
    }

    #[test]
    fn long_left_panel_rows_do_not_hide_right_panel() {
        let mut event = event(1);
        event.title = "title ".repeat(20);
        event.summary = "summary ".repeat(30);
        let state = TraceUiState::new(PathBuf::from("trace.jsonl"), false, vec![event]);
        let frame = build_frame(&state, 90, 16);
        let first_body_row = styled_line_to_plain(&frame.lines[2]);
        assert!(first_body_row.contains("│ Tool/result"));
    }
}
