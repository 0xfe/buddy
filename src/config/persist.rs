//! Mutable config persistence helpers.
//!
//! These helpers are intentionally narrow and only mutate specific user-facing
//! values to avoid broad config rewrites.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

use super::init::{default_global_config_path, ensure_default_global_config_at_path};

/// Persist `[display].theme` to the effective config file and return that path.
pub(super) fn persist_display_theme(
    path_override: Option<&str>,
    theme: &str,
) -> Result<PathBuf, ConfigError> {
    let normalized_theme = theme.trim().to_ascii_lowercase();
    if normalized_theme.is_empty() {
        return Err(ConfigError::Invalid(
            "display.theme cannot be empty".to_string(),
        ));
    }

    let path = resolve_persist_path(path_override)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        ensure_default_global_config_at_path(&path)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = upsert_display_theme(&existing, &normalized_theme);
    std::fs::write(&path, updated)?;
    Ok(path)
}

/// Persist `[agent].model` to the effective config file and return that path.
pub(super) fn persist_agent_model(
    path_override: Option<&str>,
    model: &str,
) -> Result<PathBuf, ConfigError> {
    let normalized_model = model.trim();
    if normalized_model.is_empty() {
        return Err(ConfigError::Invalid(
            "agent.model cannot be empty".to_string(),
        ));
    }

    let path = resolve_persist_path(path_override)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        ensure_default_global_config_at_path(&path)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = upsert_agent_model(&existing, normalized_model);
    std::fs::write(&path, updated)?;
    Ok(path)
}

/// Resolve the config file path that should receive persisted theme updates.
fn resolve_persist_path(path_override: Option<&str>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = path_override {
        return Ok(PathBuf::from(path));
    }
    if Path::new("buddy.toml").exists() {
        return Ok(PathBuf::from("buddy.toml"));
    }
    if Path::new("agent.toml").exists() {
        return Ok(PathBuf::from("agent.toml"));
    }
    default_global_config_path().ok_or_else(|| {
        ConfigError::Invalid(
            "unable to resolve default config path for theme persistence".to_string(),
        )
    })
}

/// Upsert `display.theme` while preserving unrelated file contents.
fn upsert_display_theme(input: &str, theme: &str) -> String {
    let mut lines = if input.is_empty() {
        Vec::new()
    } else {
        input.lines().map(str::to_string).collect::<Vec<_>>()
    };

    let mut display_idx: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().eq_ignore_ascii_case("[display]") {
            display_idx = Some(idx);
            break;
        }
    }

    if let Some(start) = display_idx {
        let mut end = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                end = idx;
                break;
            }
        }

        for idx in (start + 1)..end {
            if lines[idx].trim_start().starts_with("theme") {
                lines[idx] = format!("theme = \"{theme}\"");
                return ensure_trailing_newline(lines.join("\n"));
            }
        }

        lines.insert(start + 1, format!("theme = \"{theme}\""));
        return ensure_trailing_newline(lines.join("\n"));
    }

    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push("[display]".to_string());
    lines.push(format!("theme = \"{theme}\""));
    ensure_trailing_newline(lines.join("\n"))
}

/// Upsert `agent.model` while preserving unrelated file contents.
fn upsert_agent_model(input: &str, model: &str) -> String {
    let mut lines = if input.is_empty() {
        Vec::new()
    } else {
        input.lines().map(str::to_string).collect::<Vec<_>>()
    };

    let mut agent_idx: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().eq_ignore_ascii_case("[agent]") {
            agent_idx = Some(idx);
            break;
        }
    }

    if let Some(start) = agent_idx {
        let mut end = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                end = idx;
                break;
            }
        }

        for idx in (start + 1)..end {
            if is_assignment_key(&lines[idx], "model") {
                lines[idx] = format!("model = \"{model}\"");
                return ensure_trailing_newline(lines.join("\n"));
            }
        }

        lines.insert(start + 1, format!("model = \"{model}\""));
        return ensure_trailing_newline(lines.join("\n"));
    }

    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push("[agent]".to_string());
    lines.push(format!("model = \"{model}\""));
    ensure_trailing_newline(lines.join("\n"))
}

/// Return true when `line` assigns a value to `key` (e.g., `key = ...`).
fn is_assignment_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix(key) else {
        return false;
    };
    rest.trim_start().starts_with('=')
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{upsert_agent_model, upsert_display_theme};

    #[test]
    fn inserts_display_section_when_missing() {
        let out = upsert_display_theme("", "light");
        assert_eq!(out, "[display]\ntheme = \"light\"\n");
    }

    #[test]
    fn inserts_theme_into_existing_display_section() {
        let input = "[display]\ncolor = true\n";
        let out = upsert_display_theme(input, "light");
        assert_eq!(out, "[display]\ntheme = \"light\"\ncolor = true\n");
    }

    #[test]
    fn replaces_existing_theme_setting() {
        let input = "[display]\ncolor = true\ntheme = \"dark\"\n";
        let out = upsert_display_theme(input, "light");
        assert_eq!(out, "[display]\ncolor = true\ntheme = \"light\"\n");
    }

    #[test]
    fn inserts_agent_section_when_missing() {
        let out = upsert_agent_model("", "kimi");
        assert_eq!(out, "[agent]\nmodel = \"kimi\"\n");
    }

    #[test]
    fn inserts_model_into_existing_agent_section() {
        let input = "[agent]\nname = \"agent-mo\"\n";
        let out = upsert_agent_model(input, "kimi");
        assert_eq!(out, "[agent]\nmodel = \"kimi\"\nname = \"agent-mo\"\n");
    }

    #[test]
    fn replaces_existing_agent_model_setting() {
        let input = "[agent]\nname = \"agent-mo\"\nmodel = \"gpt-codex\"\n";
        let out = upsert_agent_model(input, "kimi");
        assert_eq!(out, "[agent]\nname = \"agent-mo\"\nmodel = \"kimi\"\n");
    }
}
