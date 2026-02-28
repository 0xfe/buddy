//! Auth subsystem error definitions.

use std::fmt;

/// Errors surfaced by the login/auth subsystem.
#[derive(Debug)]
pub enum AuthError {
    Io(std::io::Error),
    Http(reqwest::Error),
    Status(u16, String),
    Invalid(String),
    Unsupported(String),
    LoginExpired,
}

impl fmt::Display for AuthError {
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
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<reqwest::Error> for AuthError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}
