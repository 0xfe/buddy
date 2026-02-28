//! Auth subsystem error definitions.

use std::fmt;

/// Errors surfaced by the login/auth subsystem.
#[derive(Debug)]
pub enum AuthError {
    /// Local filesystem read/write failures.
    Io(std::io::Error),
    /// HTTP transport or decode failures.
    Http(reqwest::Error),
    /// Non-success HTTP status and raw response payload.
    Status(u16, String),
    /// Invalid auth payload or local auth state.
    Invalid(String),
    /// Operation is unsupported for the selected provider/runtime.
    Unsupported(String),
    /// Refresh/login failed because credentials are expired or revoked.
    LoginExpired,
}

impl fmt::Display for AuthError {
    /// Render concise human-readable auth error text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io: {err}"),
            Self::Http(err) => write!(f, "http: {err}"),
            Self::Status(code, body) => write!(f, "status {code}: {body}"),
            Self::Invalid(msg) => write!(f, "{msg}"),
            Self::Unsupported(msg) => write!(f, "{msg}"),
            Self::LoginExpired => {
                write!(
                    f,
                    "saved login has expired or was revoked; run `buddy login` again"
                )
            }
        }
    }
}

impl std::error::Error for AuthError {}

impl From<std::io::Error> for AuthError {
    /// Convert IO errors into auth-layer errors.
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<reqwest::Error> for AuthError {
    /// Convert reqwest errors into auth-layer errors.
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}
