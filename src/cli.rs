//! CLI argument parsing via clap.
//!
//! This module defines the user-facing command surface and leaves all runtime
//! behavior to higher-level orchestration code (`main.rs` / `agent.rs`).

use clap::{Parser, Subcommand};

/// An AI agent for the terminal. Works with OpenAI-compatible APIs.
#[derive(Debug, Parser)]
#[command(name = "buddy", version, subcommand_required = false)]
pub struct Args {
    /// Path to config file (default: ./buddy.toml or ~/.config/buddy/buddy.toml).
    #[arg(short = 'c', long = "config", global = true)]
    pub config: Option<String>,

    /// Override model profile key (if configured) or raw API model id.
    #[arg(short = 'm', long = "model", global = true)]
    pub model: Option<String>,

    /// Override API base URL.
    #[arg(long = "base-url", global = true)]
    pub base_url: Option<String>,

    /// Run shell/files tools inside a running container.
    #[arg(long = "container", global = true, conflicts_with = "ssh")]
    pub container: Option<String>,

    /// Run shell/files tools on a remote host over SSH.
    #[arg(long = "ssh", global = true, conflicts_with = "container")]
    pub ssh: Option<String>,

    /// Optional tmux session name. Without a value, uses `buddy-<agent.name>`
    /// for the active target (local, --ssh, or --container).
    #[arg(long = "tmux", global = true, num_args = 0..=1, value_name = "SESSION")]
    pub tmux: Option<Option<String>>,

    /// Disable color output.
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,

    /// In `buddy exec`, bypass shell confirmation prompts and auto-approve
    /// `run_shell` commands. Dangerous: use only in trusted contexts.
    #[arg(
        long = "dangerously-auto-approve",
        global = true,
        default_value_t = false
    )]
    pub dangerously_auto_approve: bool,

    /// Optional subcommand. When omitted, the binary runs in interactive REPL mode.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level CLI subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Initialize ~/.config/buddy with default config files.
    Init {
        /// Overwrite existing files after creating timestamped backups.
        #[arg(long = "force", default_value_t = false)]
        force: bool,
    },
    /// Execute one prompt and exit.
    Exec {
        /// Prompt text to execute.
        prompt: String,
    },
    /// Resume a saved session by ID (or resume the most recent with --last).
    Resume {
        /// Session ID to resume.
        session_id: Option<String>,
        /// Resume the most recently used session in this directory.
        #[arg(long = "last", default_value_t = false)]
        last: bool,
    },
    /// Login to a provider for a model profile.
    Login {
        /// Model profile name. Uses [agent].model when omitted.
        model: Option<String>,
        /// Remove stored credentials for this provider before starting login.
        #[arg(long = "reset", default_value_t = false)]
        reset: bool,
        /// Print credential health for this provider and exit.
        #[arg(long = "check", default_value_t = false)]
        check: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::{Args, Command};
    use clap::Parser;

    // Verifies the baseline UX contract: no subcommand means "start REPL".
    #[test]
    fn no_args_defaults_to_repl_mode() {
        let args = Args::parse_from(["buddy"]);
        assert!(args.command.is_none());
    }

    // Confirms one-shot execution captures prompt text as a positional argument.
    #[test]
    fn exec_subcommand_parses_prompt() {
        let args = Args::parse_from(["buddy", "exec", "hello"]);
        assert!(matches!(
            args.command,
            Some(Command::Exec { prompt }) if prompt == "hello"
        ));
    }

    // Guards the init overwrite flag wiring.
    #[test]
    fn init_subcommand_supports_force_flag() {
        let args = Args::parse_from(["buddy", "init", "--force"]);
        assert!(matches!(
            args.command,
            Some(Command::Init { force }) if force
        ));
    }

    // Ensures `login` accepts an explicit model profile without mutating flags.
    #[test]
    fn login_subcommand_accepts_optional_model() {
        let args = Args::parse_from(["buddy", "login", "gpt-codex"]);
        assert!(matches!(
            args.command,
            Some(Command::Login { model, reset, check })
                if model.as_deref() == Some("gpt-codex") && !reset && !check
        ));
    }

    // Ensures optional login control flags can be combined.
    #[test]
    fn login_subcommand_accepts_reset_and_check_flags() {
        let args = Args::parse_from(["buddy", "login", "gpt-codex", "--reset", "--check"]);
        assert!(matches!(
            args.command,
            Some(Command::Login { model, reset, check })
                if model.as_deref() == Some("gpt-codex") && reset && check
        ));
    }

    // Confirms explicit session IDs parse as the positional resume target.
    #[test]
    fn resume_subcommand_accepts_session_id() {
        let args = Args::parse_from(["buddy", "resume", "a1b2-c3d4"]);
        assert!(matches!(
            args.command,
            Some(Command::Resume { session_id, last }) if session_id.as_deref() == Some("a1b2-c3d4") && !last
        ));
    }

    // Ensures `--last` toggles the expected branch for resume lookup.
    #[test]
    fn resume_subcommand_supports_last_flag() {
        let args = Args::parse_from(["buddy", "resume", "--last"]);
        assert!(matches!(
            args.command,
            Some(Command::Resume { session_id, last }) if session_id.is_none() && last
        ));
    }

    // Ensures optional tmux session naming works in local mode.
    #[test]
    fn tmux_parses_without_remote_flags() {
        let args = Args::parse_from(["buddy", "--tmux", "buddy-dev"]);
        assert_eq!(args.tmux, Some(Some("buddy-dev".to_string())));
        assert!(args.ssh.is_none());
    }

    // Ensures tmux targeting also works when container execution is selected.
    #[test]
    fn tmux_parses_with_container() {
        let args = Args::parse_from(["buddy", "--container", "dev", "--tmux", "buddy-dev"]);
        assert_eq!(args.container.as_deref(), Some("dev"));
        assert_eq!(args.tmux, Some(Some("buddy-dev".to_string())));
    }

    // Guards the high-risk bypass flag parse path for `buddy exec`.
    #[test]
    fn dangerously_auto_approve_flag_parses() {
        let args = Args::parse_from(["buddy", "--dangerously-auto-approve", "exec", "hi"]);
        assert!(args.dangerously_auto_approve);
    }
}
