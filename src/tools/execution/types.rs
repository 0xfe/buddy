//! Shared execution data structures and backend-local context types.

use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::time::Duration;

/// Structured process output for shell-style commands.
pub struct ExecOutput {
    /// Numeric process exit code (`-1` when unavailable).
    pub exit_code: i32,
    /// Captured standard output text.
    pub stdout: String,
    /// Captured standard error text.
    pub stderr: String,
}

/// Waiting behavior for `run_shell` execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellWait {
    /// Wait for command completion with no explicit timeout.
    Wait,
    /// Wait for command completion, but fail if it exceeds this timeout.
    WaitWithTimeout(Duration),
    /// Do not wait for completion; fire command and return immediately.
    NoWait,
}

/// Options for tmux `capture-pane` operations.
///
/// These options are intentionally close to tmux's native flags so tool-level
/// callers can expose common capture behaviors without coupling to shell text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturePaneOptions {
    /// Optional explicit pane/session target (`tmux -t` syntax).
    pub target: Option<String>,
    /// Optional `tmux capture-pane -S` start bound.
    pub start: Option<String>,
    /// Optional `tmux capture-pane -E` end bound.
    pub end: Option<String>,
    /// Include `-J` to join wrapped lines.
    pub join_wrapped_lines: bool,
    /// Include `-N` to preserve trailing spaces.
    pub preserve_trailing_spaces: bool,
    /// Include `-e` to keep escape sequences.
    pub include_escape_sequences: bool,
    /// Include `-C` to escape non-printables.
    pub escape_non_printable: bool,
    /// Include `-a` to read alternate screen when active.
    pub include_alternate_screen: bool,
    /// Optional pre-capture delay handled by execution context.
    pub delay: Duration,
}

impl Default for CapturePaneOptions {
    fn default() -> Self {
        Self {
            target: None,
            start: None,
            end: None,
            join_wrapped_lines: true,
            preserve_trailing_spaces: false,
            include_escape_sequences: false,
            escape_non_printable: false,
            include_alternate_screen: false,
            delay: Duration::ZERO,
        }
    }
}

/// Options for tmux key injection against a pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendKeysOptions {
    /// Optional explicit pane/session target (`tmux -t` syntax).
    pub target: Option<String>,
    /// Named tmux keys to send.
    pub keys: Vec<String>,
    /// Literal text payload (`tmux send-keys -l`).
    pub literal_text: Option<String>,
    /// Whether to send Enter after other inputs.
    pub press_enter: bool,
    /// Optional pre-send delay handled by execution context.
    pub delay: Duration,
}

impl Default for SendKeysOptions {
    fn default() -> Self {
        Self {
            target: None,
            keys: Vec::new(),
            literal_text: None,
            press_enter: false,
            delay: Duration::ZERO,
        }
    }
}

/// Attach metadata for tmux-backed execution targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TmuxAttachInfo {
    /// Tmux session name.
    pub session: String,
    /// Shared window name inside the session.
    pub window: &'static str,
    /// Concrete attach target details.
    pub target: TmuxAttachTarget,
}

/// Concrete execution target for tmux attach instructions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TmuxAttachTarget {
    /// Local machine tmux attach target.
    Local,
    /// SSH host tmux attach target.
    Ssh { target: String },
    /// Container tmux attach target.
    Container { engine: String, container: String },
}

/// Container execution backend without tmux mediation.
pub(crate) struct ContainerContext {
    pub(crate) engine: ContainerEngine,
    pub(crate) container: String,
}

/// Local execution backend without tmux mediation.
pub(crate) struct LocalBackend;

/// Container execution backend backed by a managed tmux pane.
pub(crate) struct ContainerTmuxContext {
    pub(crate) engine: ContainerEngine,
    pub(crate) container: String,
    pub(crate) tmux_session: String,
    pub(crate) configured_tmux_pane: Mutex<Option<String>>,
    pub(crate) startup_existing_tmux_pane: Option<String>,
}

/// Detected container CLI frontend and compatibility mode.
pub(crate) struct ContainerEngine {
    pub(crate) command: &'static str,
    pub(crate) kind: ContainerEngineKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerEngineKind {
    /// Native Docker CLI semantics.
    Docker,
    /// Podman CLI semantics (including podman-docker compatibility).
    Podman,
}

/// Shared window name used for managed tmux sessions.
pub(crate) const TMUX_WINDOW_NAME: &str = "shared";
/// Backward-compatible window name from older releases.
pub(crate) const LEGACY_TMUX_WINDOW_NAME: &str = "buddy-shared";
/// Pane title used to identify managed panes.
pub(crate) const TMUX_PANE_TITLE: &str = "shared";

/// Local execution backend backed by a managed tmux pane.
pub(crate) struct LocalTmuxContext {
    pub(crate) tmux_session: String,
    pub(crate) configured_tmux_pane: Mutex<Option<String>>,
    pub(crate) startup_existing_tmux_pane: Option<String>,
}

/// SSH execution backend with optional managed tmux pane.
pub(crate) struct SshContext {
    pub(crate) target: String,
    pub(crate) control_path: PathBuf,
    pub(crate) tmux_session: Option<String>,
    pub(crate) configured_tmux_pane: Mutex<Option<String>>,
    pub(crate) startup_existing_tmux_pane: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EnsuredTmuxPane {
    /// Resolved pane identifier (for example `%7`).
    pub(crate) pane_id: String,
    /// Whether pane was created during this ensure call.
    pub(crate) created: bool,
}
