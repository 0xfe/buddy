//! Config-path helpers and default config initialization routines.
//!
//! All writes use race-safe create semantics where possible to avoid
//! clobbering user files when multiple processes bootstrap simultaneously.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ConfigError;

use super::defaults::DEFAULT_BUDDY_CONFIG_TEMPLATE;
use super::GlobalConfigInitResult;

/// Return the default per-user config path (`~/.config/buddy/buddy.toml`).
pub fn default_global_config_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("buddy.toml"))
}

/// Return the default REPL history path (`~/.config/buddy/history`).
pub fn default_history_path() -> Option<PathBuf> {
    config_root_dir().map(|dir| dir.join("buddy").join("history"))
}

/// Initialize `~/.config/buddy/buddy.toml`.
///
/// - Without `force`, returns `AlreadyInitialized` if the file exists.
/// - With `force`, backs up the existing file in the same directory using a
///   timestamped name, then rewrites it from the compiled template.
pub fn initialize_default_global_config(
    force: bool,
) -> Result<GlobalConfigInitResult, ConfigError> {
    let path = default_global_config_path().ok_or_else(|| {
        ConfigError::Invalid(
            "unable to resolve default config path for ~/.config/buddy/buddy.toml".to_string(),
        )
    })?;
    initialize_default_global_config_at_path(&path, force)
}

/// Ensure the default global config file exists.
///
/// Returns the global config path when available on this platform.
pub fn ensure_default_global_config() -> Result<Option<PathBuf>, ConfigError> {
    let Some(path) = default_global_config_path() else {
        return Ok(None);
    };
    ensure_default_global_config_at_path(&path)?;
    Ok(Some(path))
}

pub(super) fn ensure_default_global_config_at_path(path: &Path) -> Result<(), ConfigError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // create_new avoids clobbering an existing file if another process won the race.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(DEFAULT_BUDDY_CONFIG_TEMPLATE.as_bytes())?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

/// Initialize a config file at an explicit path, with optional force overwrite.
pub(super) fn initialize_default_global_config_at_path(
    path: &Path,
    force: bool,
) -> Result<GlobalConfigInitResult, ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if path.exists() {
        if !force {
            return Ok(GlobalConfigInitResult::AlreadyInitialized {
                path: path.to_path_buf(),
            });
        }
        // Preserve existing file before replacing it with the latest template.
        let backup_path = timestamped_backup_path(path);
        std::fs::copy(path, &backup_path)?;
        std::fs::write(path, DEFAULT_BUDDY_CONFIG_TEMPLATE)?;
        return Ok(GlobalConfigInitResult::Overwritten {
            path: path.to_path_buf(),
            backup_path,
        });
    }

    // create_new avoids clobbering if another process wins a race to create.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(DEFAULT_BUDDY_CONFIG_TEMPLATE.as_bytes())?;
            Ok(GlobalConfigInitResult::Created {
                path: path.to_path_buf(),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(GlobalConfigInitResult::AlreadyInitialized {
                path: path.to_path_buf(),
            })
        }
        Err(e) => Err(ConfigError::Io(e)),
    }
}

/// Build a non-colliding backup path in the same directory as `path`.
fn timestamped_backup_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "buddy.toml".to_string());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Prefer deterministic timestamped names and add numeric suffixes on collision.
    for suffix in 0..1000usize {
        let candidate_name = if suffix == 0 {
            format!("{file_name}.{timestamp}.bak")
        } else {
            format!("{file_name}.{timestamp}.{suffix}.bak")
        };
        let candidate = path.with_file_name(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }

    // Very unlikely fallback if all deterministic names already exist.
    path.with_file_name(format!(
        "{file_name}.{timestamp}.{}.bak",
        std::process::id()
    ))
}

/// Resolve the base config directory from env/home conventions.
pub fn config_root_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir()
        .map(|home| home.join(".config"))
        .or_else(dirs::config_dir)
}
