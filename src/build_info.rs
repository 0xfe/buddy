//! Compile-time build metadata exposed to CLI/runtime surfaces.

/// Semver package version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// VCS commit hash captured at build time.
pub const GIT_COMMIT: &str = env!("BUDDY_BUILD_GIT_HASH");

/// Build timestamp captured at compile time.
pub const BUILD_TIMESTAMP: &str = env!("BUDDY_BUILD_TIMESTAMP");

/// Extended clap version text shown via `--version`.
pub const LONG_VERSION: &str = env!("BUDDY_LONG_VERSION");

/// Render concise startup metadata shown in the interactive banner.
pub fn startup_metadata_line() -> String {
    format!("v{VERSION} ({GIT_COMMIT}, built {BUILD_TIMESTAMP})")
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
}
