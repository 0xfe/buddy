//! Shared execution data structures and backend-local context types.

use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::time::Duration;

/// Structured process output for shell-style commands.
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
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
    pub target: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub join_wrapped_lines: bool,
    pub preserve_trailing_spaces: bool,
    pub include_escape_sequences: bool,
    pub escape_non_printable: bool,
    pub include_alternate_screen: bool,
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
    pub target: Option<String>,
    pub keys: Vec<String>,
    pub literal_text: Option<String>,
    pub press_enter: bool,
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
    pub session: String,
    pub window: &'static str,
    pub target: TmuxAttachTarget,
}

/// Concrete execution target for tmux attach instructions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TmuxAttachTarget {
    Local,
    Ssh { target: String },
    Container { engine: String, container: String },
}

pub(super) struct ContainerContext {
    pub(super) engine: ContainerEngine,
    pub(super) container: String,
}

pub(super) struct LocalBackend;

pub(super) struct ContainerTmuxContext {
    pub(super) engine: ContainerEngine,
    pub(super) container: String,
    pub(super) tmux_session: String,
    pub(super) configured_tmux_pane: Mutex<Option<String>>,
    pub(super) startup_existing_tmux_pane: Option<String>,
}

pub(super) struct ContainerEngine {
    pub(super) command: &'static str,
    pub(super) kind: ContainerEngineKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContainerEngineKind {
    Docker,
    Podman,
}

pub(super) const TMUX_WINDOW_NAME: &str = "shared";
pub(super) const LEGACY_TMUX_WINDOW_NAME: &str = "buddy-shared";
pub(super) const TMUX_PANE_TITLE: &str = "shared";

pub(super) struct LocalTmuxContext {
    pub(super) tmux_session: String,
    pub(super) configured_tmux_pane: Mutex<Option<String>>,
    pub(super) startup_existing_tmux_pane: Option<String>,
}

pub(super) struct SshContext {
    pub(super) target: String,
    pub(super) control_path: PathBuf,
    pub(super) tmux_session: Option<String>,
    pub(super) configured_tmux_pane: Mutex<Option<String>>,
    pub(super) startup_existing_tmux_pane: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EnsuredTmuxPane {
    pub(super) pane_id: String,
    pub(super) created: bool,
}
