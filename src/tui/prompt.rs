//! Prompt rendering helpers for the REPL editor.

use crate::tui::settings;
use crossterm::style::{Print, PrintStyledContent, Stylize};
use crossterm::QueueableCommand;
use std::io::{self, Write};

/// Prompt mode for the REPL input renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// Standard command-entry prompt.
    Normal,
    /// Approval response prompt (y/n style).
    Approval,
}

/// Dynamic approval prompt content rendered inline on one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalPrompt<'a> {
    /// Actor label associated with the approval request.
    pub actor: &'a str,
    /// Command text under approval.
    pub command: &'a str,
    /// Whether the requested action requires elevated privileges.
    pub privileged: bool,
    /// Whether the requested action may mutate state.
    pub mutation: bool,
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
    // Plain-text variant used in non-color/non-interactive rendering paths.
    let mut line = String::from("• approve");
    if prompt.privileged {
        line.push_str(" (privileged)");
    }
    if prompt.mutation {
        line.push_str(" (mutation)");
    }
    line.push_str(" command ? [y/n] ");
    line
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
    // Color mode composes prompt fragments so risk tags can be emphasized.
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
                let prompt = approval_prompt.copied().unwrap_or(ApprovalPrompt {
                    actor: "",
                    command: "",
                    privileged: false,
                    mutation: false,
                });
                stderr.queue(PrintStyledContent(
                    settings::GLYPH_SECTION_BULLET
                        .with(settings::COLOR_SECTION_BULLET)
                        .bold(),
                ))?;
                stderr.queue(Print(settings::PROMPT_SPACER))?;
                stderr.queue(PrintStyledContent(
                    "approve".with(settings::COLOR_PROMPT_APPROVAL_QUERY).bold(),
                ))?;
                if prompt.privileged {
                    stderr.queue(Print(settings::PROMPT_SPACER))?;
                    stderr.queue(PrintStyledContent(
                        "(privileged)"
                            .with(settings::COLOR_PROMPT_APPROVAL_PRIVILEGED)
                            .bold(),
                    ))?;
                }
                if prompt.mutation {
                    stderr.queue(Print(settings::PROMPT_SPACER))?;
                    stderr.queue(PrintStyledContent(
                        "(mutation)"
                            .with(settings::COLOR_PROMPT_APPROVAL_MUTATION)
                            .bold(),
                    ))?;
                }
                stderr.queue(Print(settings::PROMPT_SPACER))?;
                stderr.queue(PrintStyledContent(
                    "command ?".with(settings::COLOR_PROMPT_APPROVAL_COMMAND),
                ))?;
                stderr.queue(PrintStyledContent(
                    " [y/n]".with(settings::COLOR_PROMPT_APPROVAL_COMMAND),
                ))?;
                stderr.queue(Print(settings::PROMPT_SPACER))?;
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
        // Normal prompts should include host and context metadata when provided.
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
        // Approval prompt string should include both risk qualifiers.
        let prompt = ApprovalPrompt {
            actor: "mo@bee",
            command: "top",
            privileged: true,
            mutation: true,
        };
        assert_eq!(
            approval_prompt_text(&prompt),
            "• approve (privileged) (mutation) command ? [y/n] "
        );
    }
}
