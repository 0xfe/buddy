//! Tmux session/pane orchestration and command transport helpers.
//!
//! These helpers keep tmux interactions centralized so tool/backend code can
//! operate on higher-level capture/send/run primitives.

pub(crate) mod capture;
pub(crate) mod management;
pub(crate) mod pane;
pub(crate) mod prompt;
pub(crate) mod run;
pub(crate) mod send_keys;
