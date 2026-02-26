//! CLI argument parsing via clap.

use clap::Parser;

/// An AI agent for the terminal. Works with any OpenAI-compatible API.
#[derive(Debug, Parser)]
#[command(name = "buddy", version)]
pub struct Args {
    /// Prompt to send. If provided, runs in one-shot mode and exits.
    pub prompt: Option<String>,

    /// Path to config file (default: ./buddy.toml or ~/.config/buddy/buddy.toml).
    #[arg(short = 'c', long = "config")]
    pub config: Option<String>,

    /// Override model name.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// Override API base URL.
    #[arg(long = "base-url")]
    pub base_url: Option<String>,

    /// Run shell/files tools inside a running container.
    #[arg(long = "container", conflicts_with = "ssh")]
    pub container: Option<String>,

    /// Run shell/files tools on a remote host over SSH.
    #[arg(long = "ssh", conflicts_with = "container")]
    pub ssh: Option<String>,

    /// Optional tmux session name. Without a value, uses the default auto
    /// `buddy-xxxx` name for the active target (local, --ssh, or --container).
    #[arg(long = "tmux", num_args = 0..=1, value_name = "SESSION")]
    pub tmux: Option<Option<String>>,

    /// Disable color output.
    #[arg(long = "no-color")]
    pub no_color: bool,
}

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::Parser;

    #[test]
    fn tmux_parses_without_ssh() {
        let args = Args::parse_from(["buddy", "--tmux", "buddy-dev"]);
        assert_eq!(args.tmux, Some(Some("buddy-dev".to_string())));
        assert!(args.ssh.is_none());
    }

    #[test]
    fn tmux_without_value_uses_auto_session_name() {
        let args = Args::parse_from(["buddy", "--tmux"]);
        assert_eq!(args.tmux, Some(None));
    }

    #[test]
    fn tmux_parses_with_container() {
        let args = Args::parse_from(["buddy", "--container", "dev", "--tmux", "buddy-dev"]);
        assert_eq!(args.container.as_deref(), Some("dev"));
        assert_eq!(args.tmux, Some(Some("buddy-dev".to_string())));
    }
}
