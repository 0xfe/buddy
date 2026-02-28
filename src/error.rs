//! Unified error types for the agent.
//!
//! The codebase intentionally uses hand-written enums and `From` conversions
//! instead of a macro-based error crate to keep dependency surface minimal.

use std::fmt;

// ---------------------------------------------------------------------------
// ToolError
// ---------------------------------------------------------------------------

/// Errors arising from tool execution.
#[derive(Debug)]
pub enum ToolError {
    /// The model supplied arguments the tool couldn't parse.
    InvalidArguments(String),
    /// The tool ran but encountered a failure.
    ExecutionFailed(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments(msg) => write!(f, "invalid arguments: {msg}"),
            Self::ExecutionFailed(msg) => write!(f, "execution failed: {msg}"),
        }
    }
}

impl std::error::Error for ToolError {}

// ---------------------------------------------------------------------------
// ConfigError
// ---------------------------------------------------------------------------

/// Errors when loading or parsing configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// Filesystem I/O failure while reading/writing config.
    Io(std::io::Error),
    /// TOML deserialization failure for config text.
    Toml(toml::de::Error),
    /// Semantically invalid config value or conflict.
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Toml(e) => write!(f, "toml: {e}"),
            Self::Invalid(msg) => write!(f, "invalid config: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        Self::Toml(e)
    }
}

// ---------------------------------------------------------------------------
// ApiError
// ---------------------------------------------------------------------------

/// Errors from the HTTP API layer.
#[derive(Debug)]
pub enum ApiError {
    /// Network / reqwest-level error.
    Http(reqwest::Error),
    /// Non-2xx status from the API.
    Status {
        /// HTTP status code returned by the upstream provider.
        code: u16,
        /// Response body (often provider error payload).
        body: String,
        /// Parsed Retry-After hint (seconds), if provided.
        retry_after_secs: Option<u64>,
    },
    /// Login-based auth is configured but no usable login exists.
    LoginRequired(String),
    /// Response body did not match the expected API shape.
    InvalidResponse(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "http: {e}"),
            Self::Status { code, body, .. } => write!(f, "status {code}: {body}"),
            Self::LoginRequired(msg) => write!(f, "{msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid response: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl ApiError {
    /// Build a status error with optional Retry-After metadata.
    pub fn status(code: u16, body: String, retry_after_secs: Option<u64>) -> Self {
        Self::Status {
            code,
            body,
            retry_after_secs,
        }
    }

    /// Return the HTTP status code when this error came from a non-2xx response.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Status { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// Return parsed Retry-After seconds for status errors when available.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::Status {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// AgentError — top-level
// ---------------------------------------------------------------------------

/// Top-level error type for the agent.
#[derive(Debug)]
pub enum AgentError {
    /// Configuration load/validation failure.
    Config(ConfigError),
    /// HTTP/API layer failure.
    Api(ApiError),
    /// Tool execution/parsing failure.
    Tool(ToolError),
    /// Model returned no choices in the response.
    EmptyResponse,
    /// The agentic loop exceeded the configured iteration cap.
    MaxIterationsReached,
    /// Estimated context usage exceeded the hard limit even after compaction.
    ContextLimitExceeded {
        estimated_tokens: u64,
        context_limit: u64,
    },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "config: {e}"),
            Self::Api(e) => write!(f, "api: {e}"),
            Self::Tool(e) => write!(f, "tool: {e}"),
            Self::EmptyResponse => write!(f, "model returned empty response"),
            Self::MaxIterationsReached => write!(f, "max agentic loop iterations reached"),
            Self::ContextLimitExceeded {
                estimated_tokens,
                context_limit,
            } => write!(
                f,
                "context limit exceeded ({estimated_tokens}/{context_limit} estimated tokens). Run `/compact` or `/session new` and retry"
            ),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<ConfigError> for AgentError {
    fn from(e: ConfigError) -> Self {
        Self::Config(e)
    }
}

impl From<ApiError> for AgentError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<ToolError> for AgentError {
    fn from(e: ToolError) -> Self {
        Self::Tool(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ensures user-facing tool parse/execution errors are clearly formatted.
    #[test]
    fn tool_error_display() {
        assert_eq!(
            ToolError::InvalidArguments("bad json".into()).to_string(),
            "invalid arguments: bad json"
        );
        assert_eq!(
            ToolError::ExecutionFailed("timeout".into()).to_string(),
            "execution failed: timeout"
        );
    }

    // Ensures std::io::Error converts into a prefixed config error message.
    #[test]
    fn config_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = ConfigError::from(io_err);
        let s = e.to_string();
        assert!(s.starts_with("io:"), "got: {s}");
        assert!(s.contains("file not found"));
    }

    // Ensures TOML parse failures map to the dedicated config variant.
    #[test]
    fn config_error_from_toml() {
        let toml_err: toml::de::Error = toml::from_str::<toml::Value>("x = [unclosed").unwrap_err();
        let e = ConfigError::from(toml_err);
        assert!(e.to_string().starts_with("toml:"));
    }

    // Ensures custom validation errors preserve their payload text.
    #[test]
    fn config_error_invalid_message() {
        let e = ConfigError::Invalid("api key source conflict".into());
        assert_eq!(e.to_string(), "invalid config: api key source conflict");
    }

    // Ensures high-level agent errors have stable, actionable display strings.
    #[test]
    fn agent_error_display_variants() {
        assert_eq!(
            AgentError::EmptyResponse.to_string(),
            "model returned empty response"
        );
        assert_eq!(
            AgentError::MaxIterationsReached.to_string(),
            "max agentic loop iterations reached"
        );
        assert_eq!(
            AgentError::ContextLimitExceeded {
                estimated_tokens: 970,
                context_limit: 1000
            }
            .to_string(),
            "context limit exceeded (970/1000 estimated tokens). Run `/compact` or `/session new` and retry"
        );
    }

    // Ensures tool errors upcast into `AgentError` without losing detail.
    #[test]
    fn agent_error_from_tool_error() {
        let ae = AgentError::from(ToolError::ExecutionFailed("oops".into()));
        assert!(ae.to_string().contains("oops"), "got: {ae}");
    }

    // Ensures config errors upcast into `AgentError` with the expected prefix.
    #[test]
    fn agent_error_from_config_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let ae = AgentError::from(ConfigError::from(io_err));
        assert!(ae.to_string().starts_with("config:"), "got: {ae}");
    }
}
