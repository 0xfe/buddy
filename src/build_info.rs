//! Compile-time build metadata exposed to CLI/runtime surfaces.

/// Semver package version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// VCS commit hash captured at build time.
pub const GIT_COMMIT: &str = env!("BUDDY_BUILD_GIT_HASH");

/// Build timestamp captured at compile time.
pub const BUILD_TIMESTAMP: &str = env!("BUDDY_BUILD_TIMESTAMP");

/// Help trailer block that surfaces build metadata in `buddy --help`.
pub const HELP_BUILD_METADATA: &str = concat!(
    "Build metadata:\n  commit: ",
    env!("BUDDY_BUILD_GIT_HASH"),
    "\n  built: ",
    env!("BUDDY_BUILD_TIMESTAMP")
);

/// Render concise startup metadata shown in the interactive banner.
pub fn startup_metadata_line() -> String {
    format!("v{VERSION} ({GIT_COMMIT}, built {BUILD_TIMESTAMP})")
}

/// Render CLI version block used by `buddy --version`.
pub fn cli_version_text() -> String {
    format!("buddy {VERSION}\ncommit: {GIT_COMMIT}\nbuilt: {BUILD_TIMESTAMP}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_metadata_line_contains_all_fields() {
        // Startup metadata should always include version, commit, and build-time.
        let text = startup_metadata_line();
        assert!(text.starts_with('v'));
        assert!(text.contains(GIT_COMMIT));
        assert!(text.contains(BUILD_TIMESTAMP));
    }

    #[test]
    fn cli_version_text_includes_expected_lines() {
        // Version output must include all embedded metadata fields.
        let text = cli_version_text();
        assert!(text.starts_with("buddy "));
        assert!(text.contains("commit:"));
        assert!(text.contains("built:"));
    }
}
