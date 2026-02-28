//! Slash-command metadata and parsing.

/// Maximum number of autocomplete candidates rendered under the prompt.
const MAX_SUGGESTIONS: usize = 6;

/// Static slash command metadata used by both parsing and autocomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommand {
    /// Slash command token (for example `/status`).
    pub name: &'static str,
    /// Human-readable one-line summary shown in autocomplete.
    pub description: &'static str,
}

/// Built-in slash commands for interactive mode.
pub const SLASH_COMMANDS: [SlashCommand; 14] = [
    SlashCommand {
        name: "/status",
        description: "Show model, endpoint, tools, and session details.",
    },
    SlashCommand {
        name: "/context",
        description: "Show estimated context window usage.",
    },
    SlashCommand {
        name: "/ps",
        description: "List background tasks currently running.",
    },
    SlashCommand {
        name: "/kill",
        description: "Cancel a background task: /kill <id>.",
    },
    SlashCommand {
        name: "/timeout",
        description: "Set a task timeout: /timeout <dur> [id].",
    },
    SlashCommand {
        name: "/approve",
        description: "Approval policy: /approve all|ask|none|<dur>.",
    },
    SlashCommand {
        name: "/session",
        description: "Session ops: list, resume, create.",
    },
    SlashCommand {
        name: "/compact",
        description: "Compact older turns to reclaim context space.",
    },
    SlashCommand {
        name: "/model",
        description: "Switch active model profile: /model [name|index].",
    },
    SlashCommand {
        name: "/login",
        description: "Login for a model profile: /login [name|index].",
    },
    SlashCommand {
        name: "/help",
        description: "List available slash commands.",
    },
    SlashCommand {
        name: "/quit",
        description: "Exit interactive mode.",
    },
    SlashCommand {
        name: "/exit",
        description: "Exit interactive mode.",
    },
    SlashCommand {
        name: "/q",
        description: "Short alias for exit.",
    },
];

/// Parsed slash command actions consumed by the main loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandAction {
    /// Exit interactive mode.
    Quit,
    /// Print current runtime/model status.
    Status,
    /// Print current context-window usage.
    Context,
    /// List active background tasks.
    Ps,
    /// Cancel a task by optional id.
    Kill(Option<String>),
    /// Configure timeout duration, optionally for one task.
    Timeout {
        /// Timeout duration string (for example `10m`).
        duration: Option<String>,
        /// Optional task id override.
        task_id: Option<String>,
    },
    /// Configure approval policy.
    Approve(Option<String>),
    /// Session management operation.
    Session {
        /// Session command verb (for example `list`, `resume`, `create`).
        verb: Option<String>,
        /// Optional session name/id argument.
        name: Option<String>,
    },
    /// Compact session history.
    Compact,
    /// Switch the active model profile.
    Model(Option<String>),
    /// Start login flow for a model profile.
    Login(Option<String>),
    /// Show slash-command help.
    Help,
    /// Preserve unknown command token for higher-level UX handling.
    Unknown(String),
}

/// Parse a slash command from user input.
///
/// Returns `None` if the input is not a slash command.
pub fn parse_slash_command(input: &str) -> Option<SlashCommandAction> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let token = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    let action = match token.as_str() {
        "/" | "/help" => SlashCommandAction::Help,
        "/quit" | "/exit" | "/q" => SlashCommandAction::Quit,
        "/status" => SlashCommandAction::Status,
        "/context" => SlashCommandAction::Context,
        "/ps" => SlashCommandAction::Ps,
        "/kill" => SlashCommandAction::Kill(trimmed.split_whitespace().nth(1).map(str::to_string)),
        "/timeout" => SlashCommandAction::Timeout {
            duration: trimmed.split_whitespace().nth(1).map(str::to_string),
            task_id: trimmed.split_whitespace().nth(2).map(str::to_string),
        },
        "/approve" => {
            SlashCommandAction::Approve(trimmed.split_whitespace().nth(1).map(str::to_string))
        }
        "/session" => SlashCommandAction::Session {
            verb: trimmed.split_whitespace().nth(1).map(str::to_string),
            name: trimmed.split_whitespace().nth(2).map(str::to_string),
        },
        "/compact" => SlashCommandAction::Compact,
        "/model" => {
            SlashCommandAction::Model(trimmed.split_whitespace().nth(1).map(str::to_string))
        }
        "/login" => {
            SlashCommandAction::Login(trimmed.split_whitespace().nth(1).map(str::to_string))
        }
        other => SlashCommandAction::Unknown(other.to_string()),
    };

    Some(action)
}

/// Return matching slash commands for autocomplete.
pub fn matching_slash_commands(input: &str) -> Vec<SlashCommand> {
    if !input.starts_with('/') {
        return Vec::new();
    }

    let prefix = input
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|cmd| cmd.name.starts_with(prefix.as_str()))
        .take(MAX_SUGGESTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_slash_commands() {
        // Covers all built-ins and ensures payload extraction stays stable.
        assert_eq!(
            parse_slash_command("/status"),
            Some(SlashCommandAction::Status)
        );
        assert_eq!(
            parse_slash_command("/context extra"),
            Some(SlashCommandAction::Context)
        );
        assert_eq!(parse_slash_command("/ps"), Some(SlashCommandAction::Ps));
        assert_eq!(
            parse_slash_command("/kill 7"),
            Some(SlashCommandAction::Kill(Some("7".to_string())))
        );
        assert_eq!(
            parse_slash_command("/kill"),
            Some(SlashCommandAction::Kill(None))
        );
        assert_eq!(
            parse_slash_command("/timeout 10m 7"),
            Some(SlashCommandAction::Timeout {
                duration: Some("10m".to_string()),
                task_id: Some("7".to_string())
            })
        );
        assert_eq!(
            parse_slash_command("/approve ask"),
            Some(SlashCommandAction::Approve(Some("ask".to_string())))
        );
        assert_eq!(
            parse_slash_command("/session"),
            Some(SlashCommandAction::Session {
                verb: None,
                name: None
            })
        );
        assert_eq!(
            parse_slash_command("/session resume last"),
            Some(SlashCommandAction::Session {
                verb: Some("resume".to_string()),
                name: Some("last".to_string())
            })
        );
        assert_eq!(
            parse_slash_command("/compact"),
            Some(SlashCommandAction::Compact)
        );
        assert_eq!(
            parse_slash_command("/model kimi"),
            Some(SlashCommandAction::Model(Some("kimi".to_string())))
        );
        assert_eq!(
            parse_slash_command("/login"),
            Some(SlashCommandAction::Login(None))
        );
        assert_eq!(
            parse_slash_command("/login kimi"),
            Some(SlashCommandAction::Login(Some("kimi".to_string())))
        );
        assert_eq!(parse_slash_command("/q"), Some(SlashCommandAction::Quit));
        assert_eq!(parse_slash_command("hello"), None);
    }

    #[test]
    fn matching_filters_by_prefix() {
        // Ensures autocomplete filtering is prefix-based and empty-safe.
        let all = matching_slash_commands("/");
        assert!(!all.is_empty());
        let status = matching_slash_commands("/st");
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].name, "/status");
        let none = matching_slash_commands("/does-not-exist");
        assert!(none.is_empty());
    }
}
