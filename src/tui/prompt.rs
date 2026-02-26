//! Prompt rendering helpers for the REPL editor.

use crate::tui::settings;
use crossterm::style::{Print, PrintStyledContent, Stylize};
use crossterm::QueueableCommand;
use std::io::{self, Write};

/// Prompt mode for the REPL input renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    Normal,
    Approval,
}

/// Dynamic approval prompt content rendered inline on one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalPrompt<'a> {
    pub actor: &'a str,
    pub command: &'a str,
}

/// Build the visible primary prompt string.
pub fn primary_prompt_text(
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
) -> String {
    match prompt_mode {
        PromptMode::Normal => settings::normal_prompt_text(ssh_target, context_used_percent),
        PromptMode::Approval => approval_prompt
            .map(approval_prompt_text)
            .unwrap_or_else(|| settings::approval_prompt_text().to_string()),
    }
}

/// Build the fallback/plain two-line approval prompt.
pub fn approval_prompt_text(prompt: &ApprovalPrompt<'_>) -> String {
    format!(
        "{}$ {}\n{}{} run on {}? ",
        settings::INDENT_1,
        prompt.command,
        settings::INDENT_1,
        settings::GLYPH_SECTION_BULLET,
        prompt.actor
    )
}

/// Queue a one-line status indicator above the active prompt.
pub(crate) fn write_status_line<W>(
    stderr: &mut W,
    color: bool,
    status_line: &str,
    newline: &str,
) -> io::Result<()>
where
    W: Write + QueueableCommand,
{
    if color {
        stderr.queue(PrintStyledContent(
            status_line.with(settings::COLOR_STATUS_LINE),
        ))?;
    } else {
        stderr.queue(Print(status_line))?;
    }
    stderr.queue(Print(newline))?;
    Ok(())
}

/// Queue the primary prompt with color/styling.
pub(crate) fn write_primary_prompt<W>(
    stderr: &mut W,
    color: bool,
    ssh_target: Option<&str>,
    context_used_percent: Option<u16>,
    prompt_mode: PromptMode,
    approval_prompt: Option<&ApprovalPrompt<'_>>,
) -> io::Result<()>
where
    W: Write + QueueableCommand,
{
    if color {
        match prompt_mode {
            PromptMode::Normal => {
                if let Some(target) = ssh_target {
                    stderr.queue(PrintStyledContent(
                        format!("{}{}{}", settings::SSH_PREFIX, target, settings::SSH_SUFFIX)
                            .with(settings::COLOR_PROMPT_HOST),
                    ))?;
                }
                if let Some(used) = context_used_percent {
                    if ssh_target.is_some() {
                        stderr.queue(Print(settings::PROMPT_SPACER))?;
                    }
                    stderr.queue(PrintStyledContent(
                        format!("({used}% used)").with(settings::COLOR_PROMPT_HOST),
                    ))?;
                }
                stderr.queue(PrintStyledContent(
                    settings::PROMPT_SYMBOL
                        .with(settings::COLOR_PROMPT_SYMBOL)
                        .bold(),
                ))?;
                stderr.queue(Print(settings::PROMPT_SPACER))?;
            }
            PromptMode::Approval => {
                if let Some(prompt) = approval_prompt {
                    stderr.queue(Print(settings::INDENT_1))?;
                    stderr.queue(PrintStyledContent(
                        "$".with(settings::COLOR_PROMPT_APPROVAL_COMMAND)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                    stderr.queue(PrintStyledContent(
                        " ".with(settings::COLOR_PROMPT_APPROVAL_COMMAND)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                    stderr.queue(PrintStyledContent(
                        prompt
                            .command
                            .with(settings::COLOR_PROMPT_APPROVAL_COMMAND)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                    stderr.queue(Print("\r\n"))?;
                    stderr.queue(Print(settings::INDENT_1))?;
                    stderr.queue(PrintStyledContent(
                        settings::GLYPH_SECTION_BULLET
                            .with(settings::COLOR_SECTION_BULLET)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                    stderr.queue(PrintStyledContent(
                        " run on "
                            .with(settings::COLOR_PROMPT_APPROVAL_COMMAND)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                    stderr.queue(PrintStyledContent(
                        prompt
                            .actor
                            .with(settings::COLOR_PROMPT_HOST)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                    stderr.queue(PrintStyledContent(
                        "?".with(settings::COLOR_PROMPT_APPROVAL_QUERY)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG)
                            .bold(),
                    ))?;
                    stderr.queue(Print(settings::PROMPT_SPACER))?;
                } else {
                    stderr.queue(PrintStyledContent(
                        settings::PROMPT_LOCAL_APPROVAL
                            .with(settings::COLOR_PROMPT_APPROVAL_COMMAND)
                            .on(settings::COLOR_PROMPT_APPROVAL_BG),
                    ))?;
                }
            }
        }
    } else {
        stderr.queue(Print(primary_prompt_text(
            ssh_target,
            context_used_percent,
            prompt_mode,
            approval_prompt,
        )))?;
    }
    Ok(())
}

/// Queue the continuation prompt used for multiline entry.
pub(crate) fn write_continuation_prompt<W>(stderr: &mut W, color: bool) -> io::Result<()>
where
    W: Write + QueueableCommand,
{
    if color {
        stderr.queue(PrintStyledContent(
            settings::PROMPT_CONTINUATION_LABEL.with(settings::COLOR_CONTINUATION_PROMPT),
        ))?;
        stderr.queue(Print(settings::PROMPT_SPACER))?;
    } else {
        stderr.queue(Print(settings::PROMPT_CONTINUATION))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_prompt_includes_ssh_target_when_present() {
        assert_eq!(
            primary_prompt_text(None, None, PromptMode::Normal, None),
            settings::PROMPT_LOCAL_PRIMARY
        );
        assert_eq!(
            primary_prompt_text(Some("user@host"), None, PromptMode::Normal, None),
            "(ssh user@host)> "
        );
        assert_eq!(
            primary_prompt_text(None, None, PromptMode::Approval, None),
            settings::PROMPT_LOCAL_APPROVAL
        );
        assert_eq!(
            primary_prompt_text(Some("user@host"), None, PromptMode::Approval, None),
            settings::PROMPT_LOCAL_APPROVAL
        );
        assert_eq!(
            primary_prompt_text(Some("user@host"), Some(4), PromptMode::Normal, None),
            "(ssh user@host) (4% used)> "
        );
    }

    #[test]
    fn approval_prompt_formats_on_two_lines() {
        let prompt = ApprovalPrompt {
            actor: "mo@bee",
            command: "top",
        };
        assert_eq!(
            approval_prompt_text(&prompt),
            "  $ top\n  • run on mo@bee? "
        );
    }
}
