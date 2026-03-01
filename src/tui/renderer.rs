//! Terminal output renderer for status and trace messages.

use crate::tui::highlight::{highlight_lines_for_path, StyledToken};
use crate::tui::markdown::render_markdown_for_terminal;
use crate::tui::progress::{set_progress_enabled, start_progress, ProgressHandle, ProgressMetrics};
use crate::tui::settings;
use crate::tui::text::{
    clip_to_width, snippet_preview, truncate_single_line, visible_width, wrap_for_block,
};
use crossterm::style::{Color, Print, PrintStyledContent, Stylize};
use crossterm::terminal;
use crossterm::QueueableCommand;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnippetTone {
    /// Tool/file output blocks.
    Tool,
    /// Model reasoning trace blocks.
    Reasoning,
    /// Approval-related blocks.
    Approval,
    /// Assistant response blocks written to stdout.
    Assistant,
}

impl SnippetTone {
    /// Background color assigned to this tone.
    fn bg(self) -> Color {
        match self {
            Self::Tool => settings::color_snippet_tool_bg(),
            Self::Reasoning => settings::color_snippet_reasoning_bg(),
            Self::Approval => settings::color_snippet_approval_bg(),
            Self::Assistant => settings::color_snippet_assistant_bg(),
        }
    }

    /// Foreground color assigned to this tone.
    fn fg(self) -> Color {
        match self {
            Self::Tool => settings::color_snippet_tool_text(),
            Self::Reasoning => settings::color_snippet_reasoning_text(),
            Self::Approval => settings::color_snippet_approval_text(),
            Self::Assistant => settings::color_snippet_assistant_text(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockWrapMode {
    /// Soft-wrap long lines within the block width.
    Wrap,
    /// Clip long lines at the block width.
    Clip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockTarget {
    /// Write block rows to stdout.
    Stdout,
    /// Write block rows to stderr.
    Stderr,
}

#[derive(Debug, Default)]
struct StreamSpacingState {
    /// Whether stdout currently ends on a blank separator row.
    stdout_blank: bool,
    /// Whether stderr currently ends on a blank separator row.
    stderr_blank: bool,
}

impl StreamSpacingState {
    /// Query blank-state bookkeeping for a specific stream.
    fn is_blank(&self, target: BlockTarget) -> bool {
        match target {
            BlockTarget::Stdout => self.stdout_blank,
            BlockTarget::Stderr => self.stderr_blank,
        }
    }

    /// Update blank-state bookkeeping for a specific stream.
    fn set_blank(&mut self, target: BlockTarget, blank: bool) {
        match target {
            BlockTarget::Stdout => self.stdout_blank = blank,
            BlockTarget::Stderr => self.stderr_blank = blank,
        }
    }
}

fn spacing_state() -> &'static Mutex<StreamSpacingState> {
    static STATE: OnceLock<Mutex<StreamSpacingState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(StreamSpacingState::default()))
}

#[derive(Debug, Clone, Copy)]
struct BlockSpec<'a> {
    /// Visual tone for colors/semantics.
    tone: SnippetTone,
    /// Destination stream.
    target: BlockTarget,
    /// Wrapping policy applied to long rows.
    wrap_mode: BlockWrapMode,
    /// Optional source-line cap before adding truncation marker.
    max_source_lines: Option<usize>,
    /// Optional path hint used for syntax highlighting.
    syntax_path: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RowContent {
    /// Unstyled plain text row.
    Plain(String),
    /// Row carrying pre-split style tokens.
    Highlighted(Vec<StyledToken>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedRow {
    /// Row payload.
    content: RowContent,
    /// Whether this row should render with muted/truncation color.
    muted: bool,
}

impl RenderedRow {
    /// Construct a non-muted plain row.
    fn plain(text: String) -> Self {
        Self {
            content: RowContent::Plain(text),
            muted: false,
        }
    }

    /// Construct a non-muted highlighted row.
    fn highlighted(tokens: Vec<StyledToken>) -> Self {
        Self {
            content: RowContent::Highlighted(tokens),
            muted: false,
        }
    }

    /// Construct a muted plain row.
    fn muted(text: String) -> Self {
        Self {
            content: RowContent::Plain(text),
            muted: true,
        }
    }

    /// Convert highlighted/plain content into plain text fallback.
    fn as_plain_text(&self) -> String {
        match &self.content {
            RowContent::Plain(text) => text.clone(),
            RowContent::Highlighted(tokens) => {
                let mut out = String::new();
                for token in tokens {
                    out.push_str(&token.text);
                }
                out
            }
        }
    }
}

/// Handles all terminal output formatting.
#[derive(Debug, Clone, Copy)]
pub struct Renderer {
    /// Whether ANSI color/style output is enabled.
    color: bool,
}

impl Renderer {
    /// Create a renderer with optional color output.
    pub fn new(color: bool) -> Self {
        Self { color }
    }

    /// Globally enable/disable live progress spinners.
    pub fn set_progress_enabled(enabled: bool) {
        set_progress_enabled(enabled);
    }

    /// Print the user input prompt indicator (to stderr).
    pub fn prompt(&self) {
        if self.color {
            eprint!(
                "{} ",
                settings::PROMPT_SYMBOL
                    .with(settings::color_agent_label())
                    .bold()
            );
        } else {
            eprint!("{}", settings::PROMPT_LOCAL_PRIMARY);
        }
    }

    /// Print the assistant's response as a markdown-friendly wrapped block.
    pub fn assistant_message(&self, content: &str) {
        let rendered = render_markdown_for_terminal(content);
        self.render_block(
            &rendered,
            BlockSpec {
                tone: SnippetTone::Assistant,
                target: BlockTarget::Stdout,
                wrap_mode: BlockWrapMode::Wrap,
                max_source_lines: None,
                syntax_path: None,
            },
        );
    }

    /// Start a spinner with a status label on stderr.
    pub fn progress(&self, label: &str) -> ProgressHandle {
        self.progress_with_metrics(label, ProgressMetrics::default())
    }

    /// Start a spinner with optional metric key/value pairs.
    pub fn progress_with_metrics(&self, label: &str, metrics: ProgressMetrics) -> ProgressHandle {
        start_progress(label.to_string(), self.color, Some(metrics))
    }

    /// Print the model/session header.
    pub fn header(&self, model: &str) {
        if self.color {
            eprintln!(
                "\r{} {}",
                settings::LABEL_AGENT
                    .with(settings::color_agent_label())
                    .bold(),
                model.with(settings::color_model_name()),
            );
        } else {
            eprintln!("\r{} ({model})", settings::LABEL_AGENT);
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print a tool call invocation (to stderr).
    pub fn tool_call(&self, name: &str, args: &str) {
        let preview = truncate_single_line(args, 80);
        if self.color {
            eprintln!(
                "\r{}{} {}({})",
                settings::INDENT_1,
                settings::GLYPH_TOOL_CALL.with(settings::color_tool_call_glyph()),
                name.with(settings::color_tool_call_name()).bold(),
                preview.with(settings::color_tool_call_args()),
            );
        } else {
            eprintln!(
                "\r{}{} {name}({preview})",
                settings::INDENT_1,
                settings::GLYPH_TOOL_CALL_PLAIN
            );
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print a tool result summary (to stderr).
    pub fn tool_result(&self, result: &str) {
        let preview = truncate_single_line(result, 120);
        if self.color {
            eprintln!(
                "\r{}{} {}",
                settings::INDENT_1,
                settings::GLYPH_TOOL_RESULT.with(settings::color_tool_result_glyph()),
                preview.with(settings::color_tool_result_text()),
            );
        } else {
            eprintln!(
                "\r{}{} {preview}",
                settings::INDENT_1,
                settings::GLYPH_TOOL_RESULT_PLAIN
            );
        }
        mark_stream_nonblank(BlockTarget::Stderr);
        if let Some((_, after_stdout)) = result.split_once("\nstdout:\n") {
            let (stdout, stderr) = after_stdout
                .split_once("\nstderr:\n")
                .unwrap_or((after_stdout, ""));
            self.command_output_block(stdout);
            if !stderr.trim().is_empty() {
                self.command_output_block(stderr);
            }
            return;
        }
        if result.contains('\n') {
            self.tool_output_block(result, None);
        }
    }

    /// Print token usage for the current request (to stderr).
    pub fn token_usage(&self, prompt: u64, completion: u64, session_total: u64) {
        if self.color {
            eprintln!(
                "\r{}{} prompt:{} completion:{} session:{}",
                settings::INDENT_1,
                settings::LABEL_TOKENS.with(settings::color_token_label()),
                prompt.to_string().with(settings::color_token_value()),
                completion.to_string().with(settings::color_token_value()),
                session_total
                    .to_string()
                    .with(settings::color_token_session()),
            );
        } else {
            eprintln!(
                "\r{}{} prompt:{prompt} completion:{completion} session:{session_total}",
                settings::INDENT_1,
                settings::LABEL_TOKENS
            );
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print a model-emitted reasoning/thinking trace (to stderr).
    pub fn reasoning_trace(&self, field: &str, trace: &str) {
        if trace.trim().is_empty() {
            return;
        }

        if self.color {
            eprintln!(
                "\r{}{} {}",
                settings::INDENT_1,
                settings::LABEL_THINKING
                    .with(settings::color_reasoning_label())
                    .bold(),
                format!("({field})").with(settings::color_reasoning_meta()),
            );
        } else {
            eprintln!(
                "\r{}{} ({field})",
                settings::INDENT_1,
                settings::LABEL_THINKING
            );
        }
        mark_stream_nonblank(BlockTarget::Stderr);
        self.reasoning_block(trace);
    }

    /// Print a warning (to stderr).
    pub fn warn(&self, msg: &str) {
        if self.color {
            eprintln!(
                "\r{} {msg}",
                settings::LABEL_WARNING
                    .with(settings::color_warning())
                    .bold()
            );
        } else {
            eprintln!("\r{} {msg}", settings::LABEL_WARNING);
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print a small section header in status-style output.
    pub fn section(&self, title: &str) {
        if self.color {
            eprintln!(
                "\r{} {}",
                settings::GLYPH_SECTION_BULLET.with(settings::color_section_bullet()),
                title.with(settings::color_section_title()).bold()
            );
        } else {
            eprintln!("\r{title}:");
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print an activity line (bold gray) for task/prompt lifecycle updates.
    pub fn activity(&self, text: &str) {
        if self.color {
            eprintln!(
                "\r{} {}",
                settings::GLYPH_SECTION_BULLET.with(settings::color_section_bullet()),
                text.with(settings::color_activity_text()).bold()
            );
        } else {
            eprintln!("\r{text}");
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print a key/value line under a status section.
    pub fn field(&self, key: &str, value: &str) {
        if self.color {
            eprintln!(
                "\r{}{} {}",
                settings::INDENT_1,
                format!("{key}:").with(settings::color_field_key()),
                value.with(settings::color_field_value()),
            );
        } else {
            eprintln!("\r{}{key}: {value}", settings::INDENT_1);
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print a simple indented detail line.
    pub fn detail(&self, text: &str) {
        if self.color {
            eprintln!(
                "\r{}{}",
                settings::INDENT_1,
                text.with(settings::color_field_value())
            );
        } else {
            eprintln!("\r{}{text}", settings::INDENT_1);
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Print an error (to stderr).
    pub fn error(&self, msg: &str) {
        if self.color {
            eprintln!(
                "\r{} {msg}",
                settings::LABEL_ERROR.with(settings::color_error()).bold()
            );
        } else {
            eprintln!("\r{} {msg}", settings::LABEL_ERROR);
        }
        mark_stream_nonblank(BlockTarget::Stderr);
    }

    /// Render a clipped, tinted block for file/tool output (wrapped).
    pub fn tool_output_block(&self, text: &str, syntax_path: Option<&str>) {
        self.render_block(
            text,
            BlockSpec {
                tone: SnippetTone::Tool,
                target: BlockTarget::Stderr,
                wrap_mode: BlockWrapMode::Wrap,
                max_source_lines: Some(settings::SNIPPET_PREVIEW_LINES),
                syntax_path,
            },
        );
    }

    /// Render a clipped, tinted block for shell/command output (no wrapping).
    pub fn command_output_block(&self, text: &str) {
        self.render_block(
            text,
            BlockSpec {
                tone: SnippetTone::Tool,
                target: BlockTarget::Stderr,
                wrap_mode: BlockWrapMode::Clip,
                max_source_lines: Some(settings::SNIPPET_PREVIEW_LINES),
                syntax_path: None,
            },
        );
    }

    /// Render a clipped, tinted block for model reasoning traces.
    pub fn reasoning_block(&self, text: &str) {
        self.render_block(
            text,
            BlockSpec {
                tone: SnippetTone::Reasoning,
                target: BlockTarget::Stderr,
                wrap_mode: BlockWrapMode::Wrap,
                max_source_lines: None,
                syntax_path: None,
            },
        );
    }

    /// Render a clipped, tinted block for approval-related text.
    pub fn approval_block(&self, text: &str) {
        self.render_block(
            text,
            BlockSpec {
                tone: SnippetTone::Approval,
                target: BlockTarget::Stderr,
                wrap_mode: BlockWrapMode::Wrap,
                max_source_lines: Some(settings::SNIPPET_PREVIEW_LINES),
                syntax_path: None,
            },
        );
    }

    /// Core block renderer shared by assistant/tool/reasoning output helpers.
    fn render_block(&self, text: &str, spec: BlockSpec<'_>) {
        // Walkthrough:
        // 1) choose source lines (with optional preview truncation),
        // 2) split each source line into render rows with optional highlighting,
        // 3) write rows to target stream with spacing management,
        // 4) fall back to plain printing if queue-based writes fail.
        let preview = match spec.max_source_lines {
            Some(max_lines) => snippet_preview(text, max_lines),
            None => {
                let lines = text.lines().collect::<Vec<_>>();
                let remaining_lines = 0;
                crate::tui::text::SnippetPreview {
                    lines,
                    remaining_lines,
                }
            }
        };
        if preview.lines.is_empty() {
            return;
        }

        let block_width = block_content_width();
        let highlighted = spec
            .syntax_path
            .and_then(|path| highlight_lines_for_path(path, &preview.lines));
        let mut rows = Vec::<RenderedRow>::new();

        for (idx, line) in preview.lines.iter().enumerate() {
            // Prefer syntax-highlighter output, then assistant-markdown styling,
            // and finally plain wrapping/clipping.
            if let Some(tokens_by_line) = &highlighted {
                if let Some(tokens) = tokens_by_line.get(idx) {
                    rows.extend(split_highlighted_line(tokens, block_width, spec.wrap_mode));
                    continue;
                }
            }
            if spec.tone == SnippetTone::Assistant {
                if let Some(tokens) = style_assistant_markdown_line(line) {
                    rows.extend(split_highlighted_line(&tokens, block_width, spec.wrap_mode));
                    continue;
                }
            }
            rows.extend(split_plain_line(line, block_width, spec.wrap_mode));
        }

        if preview.remaining_lines > 0 {
            rows.push(RenderedRow::muted(format!(
                "...{} more lines...",
                preview.remaining_lines
            )));
        }

        let write_result = match spec.target {
            BlockTarget::Stdout => {
                let mut stdout = io::stdout();
                self.prepare_block_spacing(&mut stdout, spec.target)
                    .and_then(|_| self.write_rows(&mut stdout, &rows, block_width, spec.tone))
                    .and_then(|_| {
                        mark_stream_nonblank(spec.target);
                        self.finish_block_spacing(&mut stdout, spec.target)
                    })
            }
            BlockTarget::Stderr => {
                let mut stderr = io::stderr();
                self.prepare_block_spacing(&mut stderr, spec.target)
                    .and_then(|_| self.write_rows(&mut stderr, &rows, block_width, spec.tone))
                    .and_then(|_| {
                        mark_stream_nonblank(spec.target);
                        self.finish_block_spacing(&mut stderr, spec.target)
                    })
            }
        };
        if write_result.is_err() {
            // Degrade gracefully if terminal queueing fails; preserve content.
            if !stream_is_blank(spec.target) {
                match spec.target {
                    BlockTarget::Stdout => println!(),
                    BlockTarget::Stderr => eprintln!(),
                }
            }
            for row in rows {
                match spec.target {
                    BlockTarget::Stdout => {
                        println!("{}{}", settings::INDENT_1, row.as_plain_text())
                    }
                    BlockTarget::Stderr => {
                        eprintln!("{}{}", settings::INDENT_1, row.as_plain_text())
                    }
                }
            }
            match spec.target {
                BlockTarget::Stdout => println!(),
                BlockTarget::Stderr => eprintln!(),
            }
            mark_stream_blank(spec.target);
        }
    }

    fn write_rows<W: Write + QueueableCommand>(
        &self,
        out: &mut W,
        rows: &[RenderedRow],
        block_width: usize,
        tone: SnippetTone,
    ) -> io::Result<()> {
        // Render each logical row as:
        // "\r" + indent + styled/clipped payload + "\r\n".
        let bg = tone.bg();
        let default_fg = tone.fg();

        for row in rows {
            out.queue(Print("\r"))?;
            out.queue(Print(settings::INDENT_1))?;
            if self.color {
                let mut used = 0usize;
                match &row.content {
                    RowContent::Plain(text) => {
                        let clipped = clip_to_width(text, block_width);
                        used = visible_width(&clipped);
                        let fg = if row.muted {
                            settings::color_snippet_truncated()
                        } else {
                            default_fg
                        };
                        out.queue(PrintStyledContent(clipped.with(fg).on(bg)))?;
                    }
                    RowContent::Highlighted(tokens) => {
                        for token in tokens {
                            if used >= block_width {
                                break;
                            }
                            let clipped = clip_to_width(&token.text, block_width - used);
                            if clipped.is_empty() {
                                continue;
                            }
                            used += visible_width(&clipped);
                            let mut styled = clipped.as_str().with(Color::Rgb {
                                r: token.rgb.0,
                                g: token.rgb.1,
                                b: token.rgb.2,
                            });
                            styled = styled.on(bg);
                            if token.bold {
                                styled = styled.bold();
                            }
                            if token.italic {
                                styled = styled.italic();
                            }
                            if token.underline {
                                styled = styled.underlined();
                            }
                            out.queue(PrintStyledContent(styled))?;
                        }
                    }
                }

                let pad = block_width.saturating_sub(used);
                if pad > 0 {
                    out.queue(PrintStyledContent(" ".repeat(pad).with(default_fg).on(bg)))?;
                }
            } else {
                out.queue(Print(clip_to_width(&row.as_plain_text(), block_width)))?;
            }
            out.queue(Print("\r\n"))?;
        }
        Ok(())
    }

    fn prepare_block_spacing<W: Write + QueueableCommand>(
        &self,
        out: &mut W,
        target: BlockTarget,
    ) -> io::Result<()> {
        // Ensure a blank separator exists between consecutive non-blank sections.
        out.queue(Print("\r"))?;
        if !stream_is_blank(target) {
            out.queue(Print("\r\n"))?;
        }
        mark_stream_blank(target);
        Ok(())
    }

    fn finish_block_spacing<W: Write + QueueableCommand>(
        &self,
        out: &mut W,
        target: BlockTarget,
    ) -> io::Result<()> {
        // Leave stream in a blank-separated state after each rendered block.
        out.queue(Print("\r\n"))?;
        out.flush()?;
        mark_stream_blank(target);
        Ok(())
    }
}

fn block_content_width() -> usize {
    let cols = terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(settings::BLOCK_FALLBACK_COLUMNS);
    let indent = settings::INDENT_1.chars().count();
    cols.saturating_sub(indent + settings::BLOCK_RIGHT_MARGIN)
        .max(1)
}

/// Split one plain line into one-or-more rendered rows.
fn split_plain_line(line: &str, width: usize, wrap_mode: BlockWrapMode) -> Vec<RenderedRow> {
    match wrap_mode {
        BlockWrapMode::Wrap => wrap_for_block(line, width)
            .into_iter()
            .map(RenderedRow::plain)
            .collect(),
        BlockWrapMode::Clip => vec![RenderedRow::plain(clip_to_width(line, width))],
    }
}

/// Apply lightweight markdown-oriented styling for assistant plain-text output.
fn style_assistant_markdown_line(line: &str) -> Option<Vec<StyledToken>> {
    if line.trim().is_empty() {
        return None;
    }

    let indent_chars = line.chars().take_while(|ch| ch.is_whitespace()).count();
    let indent_end = line
        .char_indices()
        .nth(indent_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(line.len());
    let indent = &line[..indent_end];
    let trimmed = &line[indent_end..];

    let mut tokens = Vec::new();
    if !indent.is_empty() {
        tokens.push(styled_token(
            indent,
            settings::rgb_snippet_assistant_text(),
            false,
            false,
            false,
        ));
    }

    if trimmed.starts_with("```") {
        tokens.push(styled_token(
            trimmed,
            settings::rgb_snippet_assistant_md_code(),
            false,
            false,
            false,
        ));
        return Some(tokens);
    }

    if is_markdown_heading(trimmed) {
        tokens.push(styled_token(
            trimmed,
            settings::rgb_snippet_assistant_md_heading(),
            true,
            false,
            false,
        ));
        return Some(tokens);
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        tokens.push(styled_token(
            "> ",
            settings::rgb_snippet_assistant_md_marker(),
            true,
            false,
            false,
        ));
        tokens.extend(style_inline_code(
            rest,
            settings::rgb_snippet_assistant_md_quote(),
        ));
        return Some(tokens);
    }

    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        tokens.push(styled_token(
            &trimmed[..2],
            settings::rgb_snippet_assistant_md_marker(),
            true,
            false,
            false,
        ));
        tokens.extend(style_inline_code(
            rest,
            settings::rgb_snippet_assistant_text(),
        ));
        return Some(tokens);
    }

    if let Some((prefix, rest)) = split_ordered_list_prefix(trimmed) {
        tokens.push(styled_token(
            prefix,
            settings::rgb_snippet_assistant_md_marker(),
            true,
            false,
            false,
        ));
        tokens.extend(style_inline_code(
            rest,
            settings::rgb_snippet_assistant_text(),
        ));
        return Some(tokens);
    }

    if trimmed.contains('`') {
        tokens.extend(style_inline_code(
            trimmed,
            settings::rgb_snippet_assistant_text(),
        ));
        return Some(tokens);
    }

    None
}

/// Parse ordered-list prefixes like `1. ` and return `(prefix, rest)`.
fn split_ordered_list_prefix(line: &str) -> Option<(&str, &str)> {
    let digit_chars = line.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_chars == 0 {
        return None;
    }

    let digit_end = line
        .char_indices()
        .nth(digit_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(line.len());
    let after_digits = &line[digit_end..];
    let rest = after_digits.strip_prefix(". ")?;
    let prefix = &line[..digit_end + 2];
    Some((prefix, rest))
}

/// Return true when a line starts with a markdown heading marker.
fn is_markdown_heading(line: &str) -> bool {
    let hash_chars = line.chars().take_while(|ch| *ch == '#').count();
    if hash_chars == 0 {
        return false;
    }
    line.chars().nth(hash_chars) == Some(' ')
}

/// Style backtick-delimited inline code segments inside one line.
fn style_inline_code(line: &str, base_rgb: (u8, u8, u8)) -> Vec<StyledToken> {
    let mut tokens = Vec::new();
    let mut in_code = false;
    let mut current = String::new();

    let flush_current = |tokens: &mut Vec<StyledToken>, current: &mut String, in_code: bool| {
        if current.is_empty() {
            return;
        }
        let rgb = if in_code {
            settings::rgb_snippet_assistant_md_code()
        } else {
            base_rgb
        };
        tokens.push(styled_token(current.as_str(), rgb, false, false, false));
        current.clear();
    };

    for ch in line.chars() {
        if ch == '`' {
            flush_current(&mut tokens, &mut current, in_code);
            tokens.push(styled_token(
                "`",
                settings::rgb_snippet_assistant_md_code(),
                false,
                false,
                false,
            ));
            in_code = !in_code;
            continue;
        }
        current.push(ch);
    }
    flush_current(&mut tokens, &mut current, in_code);

    if tokens.is_empty() {
        tokens.push(styled_token(line, base_rgb, false, false, false));
    }
    tokens
}

/// Convenience constructor for one `StyledToken`.
fn styled_token(
    text: impl Into<String>,
    rgb: (u8, u8, u8),
    bold: bool,
    italic: bool,
    underline: bool,
) -> StyledToken {
    StyledToken {
        text: text.into(),
        rgb,
        bold,
        italic,
        underline,
    }
}

/// Split highlighted tokens into wrapped/clipped rows while preserving style spans.
fn split_highlighted_line(
    tokens: &[StyledToken],
    width: usize,
    wrap_mode: BlockWrapMode,
) -> Vec<RenderedRow> {
    if tokens.is_empty() {
        return vec![RenderedRow::plain(String::new())];
    }

    let mut rows = Vec::<Vec<StyledToken>>::new();
    let mut current = Vec::<StyledToken>::new();
    let mut used = 0usize;

    'token_loop: for token in tokens {
        for ch in token.text.chars() {
            if used >= width {
                match wrap_mode {
                    BlockWrapMode::Wrap => {
                        rows.push(current);
                        current = Vec::new();
                        used = 0;
                    }
                    BlockWrapMode::Clip => break 'token_loop,
                }
            }
            push_highlighted_char(&mut current, token, ch);
            used += 1;
        }
    }
    rows.push(current);

    rows.into_iter().map(RenderedRow::highlighted).collect()
}

/// Append one char to the current row, merging with previous token when style matches.
fn push_highlighted_char(current: &mut Vec<StyledToken>, style: &StyledToken, ch: char) {
    if let Some(last) = current.last_mut() {
        if same_style(last, style) {
            last.text.push(ch);
            return;
        }
    }

    current.push(StyledToken {
        text: ch.to_string(),
        rgb: style.rgb,
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
    });
}

/// Compare style attributes while ignoring token text content.
fn same_style(a: &StyledToken, b: &StyledToken) -> bool {
    a.rgb == b.rgb && a.bold == b.bold && a.italic == b.italic && a.underline == b.underline
}

/// Read current stream-spacing state.
fn stream_is_blank(target: BlockTarget) -> bool {
    spacing_state()
        .lock()
        .map(|state| state.is_blank(target))
        .unwrap_or(false)
}

/// Mark stream as containing non-blank content.
fn mark_stream_nonblank(target: BlockTarget) {
    if let Ok(mut state) = spacing_state().lock() {
        state.set_blank(target, false);
    }
}

/// Mark stream as ending on a blank separator row.
fn mark_stream_blank(target: BlockTarget) {
    if let Ok(mut state) = spacing_state().lock() {
        state.set_blank(target, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_heading_line_gets_heading_style() {
        // Headings should be promoted to heading color + bold style.
        let tokens = style_assistant_markdown_line("## Heading").expect("styled");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, "## Heading");
        assert!(tokens[0].bold);
        assert_eq!(tokens[0].rgb, settings::rgb_snippet_assistant_md_heading());
    }

    #[test]
    fn assistant_plain_line_has_no_extra_style() {
        // Plain prose should render with default assistant tone only.
        assert!(style_assistant_markdown_line("plain sentence").is_none());
    }

    #[test]
    fn assistant_inline_code_preserves_text_and_styles_code_segments() {
        // Inline code styling must preserve exact source text while tagging code spans.
        let tokens = style_assistant_markdown_line("Use `ls -la` now").expect("styled");
        let rendered = tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>();
        assert_eq!(rendered, "Use `ls -la` now");
        assert!(tokens
            .iter()
            .any(|token| token.rgb == settings::rgb_snippet_assistant_md_code()));
    }
}
