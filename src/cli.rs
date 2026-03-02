//! CLI argument parsing via clap.
//!
//! This module defines the user-facing command surface and leaves all runtime
//! behavior to higher-level orchestration code (`main.rs` / `agent.rs`).

use clap::{ArgAction, Parser, Subcommand};

/// An AI agent for the terminal. Works with OpenAI-compatible APIs.
#[derive(Debug, Parser)]
#[command(
    name = "buddy",
    version = buddy::build_info::VERSION,
    long_version = buddy::build_info::VERSION,
    after_help = buddy::build_info::HELP_BUILD_METADATA,
    disable_version_flag = true,
    subcommand_required = false
)]
pub struct Args {
    /// Print version/commit/build metadata.
    #[arg(short = 'V', long = "version", global = true, action = ArgAction::SetTrue)]
    pub version: bool,

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

    /// Increase runtime diagnostics (`-v`, `-vv`, `-vvv`).
    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Write runtime events to a JSONL trace file.
    #[arg(long = "trace", global = true, value_name = "PATH")]
    pub trace: Option<String>,

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
    /// Login to a provider.
    Login {
        /// Provider (e.g., openai, openrouter, moonshot/kimi, anthropic/claude).
        /// For compatibility, model-profile selectors are also accepted.
        provider: Option<String>,
        /// Remove stored credentials for this provider before starting login.
        #[arg(long = "reset", default_value_t = false)]
        reset: bool,
        /// Print credential health for this provider and exit.
        #[arg(long = "check", default_value_t = false)]
        check: bool,
    },
    /// Remove saved login credentials for a provider.
    Logout {
        /// Provider (e.g., openai). Uses active profile provider when omitted.
        provider: Option<String>,
    },
    /// Analyze runtime trace JSONL files.
    Trace {
        /// Trace analysis command.
        #[command(subcommand)]
        command: TraceCommand,
    },
}

/// Trace analysis subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum TraceCommand {
    /// Show a high-level summary for a trace file.
    Summary {
        /// Path to JSONL trace file.
        file: String,
    },
    /// Replay one prompt turn from a trace file.
    Replay {
        /// Path to JSONL trace file.
        file: String,
        /// 1-based prompt turn index to replay.
        #[arg(long = "turn")]
        turn: usize,
    },
    /// Show context/token/cost timeline evolution for a trace file.
    ContextEvolution {
        /// Path to JSONL trace file.
        file: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{Args, Command, TraceCommand};
    use clap::{CommandFactory, Parser};

    // Verifies the baseline UX contract: no subcommand means "start REPL".
    #[test]
    fn no_args_defaults_to_repl_mode() {
        let args = Args::parse_from(["buddy"]);
        assert!(args.command.is_none());
        assert!(!args.version);
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

    // Ensures `login` accepts an explicit provider without mutating flags.
    #[test]
    fn login_subcommand_accepts_optional_provider() {
        let args = Args::parse_from(["buddy", "login", "openai"]);
        assert!(matches!(
            args.command,
            Some(Command::Login { provider, reset, check })
                if provider.as_deref() == Some("openai") && !reset && !check
        ));
    }

    // Ensures optional login control flags can be combined.
    #[test]
    fn login_subcommand_accepts_reset_and_check_flags() {
        let args = Args::parse_from(["buddy", "login", "openai", "--reset", "--check"]);
        assert!(matches!(
            args.command,
            Some(Command::Login { provider, reset, check })
                if provider.as_deref() == Some("openai") && reset && check
        ));
    }

    // Ensures `logout` supports optional provider selector.
    #[test]
    fn logout_subcommand_accepts_optional_provider() {
        let args = Args::parse_from(["buddy", "logout", "openai"]);
        assert!(matches!(
            args.command,
            Some(Command::Logout { provider }) if provider.as_deref() == Some("openai")
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

    // Verifies optional runtime trace path parsing.
    #[test]
    fn trace_flag_parses() {
        let args = Args::parse_from(["buddy", "--trace", "/tmp/buddy-trace.jsonl"]);
        assert_eq!(args.trace.as_deref(), Some("/tmp/buddy-trace.jsonl"));
    }

    // Verifies verbosity counters map to the expected numeric levels.
    #[test]
    fn verbose_flag_counts_occurrences() {
        let once = Args::parse_from(["buddy", "-v"]);
        assert_eq!(once.verbose, 1);

        let thrice = Args::parse_from(["buddy", "-vvv"]);
        assert_eq!(thrice.verbose, 3);
    }

    #[test]
    fn command_exposes_build_metadata_in_version_output() {
        // The clap command should include the compile-time extended version block.
        let cmd = Args::command();
        assert_eq!(cmd.get_version(), Some(buddy::build_info::VERSION));
        assert_eq!(cmd.get_long_version(), Some(buddy::build_info::VERSION));
        let mut help = Vec::<u8>::new();
        cmd.clone().write_long_help(&mut help).unwrap();
        let help_text = String::from_utf8(help).unwrap();
        assert!(help_text.contains("Build metadata:"));
    }

    #[test]
    fn version_flag_parses_with_no_subcommand() {
        // `buddy --version` should route through the manual version path.
        let args = Args::parse_from(["buddy", "--version"]);
        assert!(args.version);
        assert!(args.command.is_none());
    }

    // Verifies trace summary command parses with file path positional argument.
    #[test]
    fn trace_summary_subcommand_parses() {
        let args = Args::parse_from(["buddy", "trace", "summary", "/tmp/buddy.trace.jsonl"]);
        assert!(matches!(
            args.command,
            Some(Command::Trace {
                command: TraceCommand::Summary { file }
            }) if file == "/tmp/buddy.trace.jsonl"
        ));
    }

    // Verifies trace replay command requires a 1-based turn selector.
    #[test]
    fn trace_replay_subcommand_parses() {
        let args = Args::parse_from([
            "buddy",
            "trace",
            "replay",
            "/tmp/buddy.trace.jsonl",
            "--turn",
            "3",
        ]);
        assert!(matches!(
            args.command,
            Some(Command::Trace {
                command: TraceCommand::Replay { file, turn }
            }) if file == "/tmp/buddy.trace.jsonl" && turn == 3
        ));
    }

    // Verifies trace context-evolution command parsing.
    #[test]
    fn trace_context_evolution_subcommand_parses() {
        let args = Args::parse_from([
            "buddy",
            "trace",
            "context-evolution",
            "/tmp/buddy.trace.jsonl",
        ]);
        assert!(matches!(
            args.command,
            Some(Command::Trace {
                command: TraceCommand::ContextEvolution { file }
            }) if file == "/tmp/buddy.trace.jsonl"
        ));
    }
}
