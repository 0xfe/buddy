//! `/theme` command helpers.
//!
//! This module owns theme selection, persistence, and preview rendering so
//! REPL command dispatch stays focused on orchestration.

use buddy::config::{persist_display_theme, Config};
use buddy::ui::render::RenderSink;
use buddy::ui::terminal as term_ui;
use buddy::ui::theme;

/// Handle `/theme` selection, activation, persistence, and preview rendering.
pub(crate) fn handle_theme_command(
    renderer: &dyn RenderSink,
    config: &mut Config,
    config_path_override: Option<&str>,
    selector: Option<&str>,
) {
    let names = configured_theme_names();
    if names.is_empty() {
        renderer.warn("No themes are available.");
        return;
    }

    let selected_name = if let Some(selector) = selector {
        let selected_input = selector.trim();
        if selected_input.is_empty() {
            return;
        }
        match resolve_theme_selector(&names, selected_input) {
            Ok(name) => name,
            Err(msg) => {
                renderer.warn(&msg);
                return;
            }
        }
    } else {
        let options = theme_picker_options(&names, &theme::active_theme_name());
        let initial = names
            .iter()
            .position(|name| name == &theme::active_theme_name())
            .unwrap_or(0);
        match term_ui::pick_from_list(
            config.display.color,
            "themes",
            "Use ↑/↓ to pick, Enter to confirm, Esc to cancel.",
            &options,
            initial,
        ) {
            Ok(Some(index)) => names[index].clone(),
            Ok(None) => return,
            Err(err) => {
                renderer.warn(&format!("failed to read theme selection: {err}"));
                return;
            }
        }
    };

    if let Err(err) = theme::set_active_theme(&selected_name) {
        renderer.warn(&err);
        return;
    }
    config.display.theme = selected_name.clone();

    let persisted_path = match persist_display_theme(config_path_override, &selected_name) {
        Ok(path) => Some(path),
        Err(err) => {
            renderer.warn(&format!(
                "theme switched to `{selected_name}`, but persistence failed: {err}"
            ));
            None
        }
    };

    renderer.section(&format!("switched theme: {selected_name}"));
    if let Some(path) = persisted_path {
        renderer.field("saved_to", &path.display().to_string());
    }
    eprintln!();
    render_theme_preview(renderer);
}

/// Return available theme names from the active registry.
pub(crate) fn configured_theme_names() -> Vec<String> {
    theme::available_theme_names()
}

/// Build picker labels with active-theme marker.
pub(crate) fn theme_picker_options(names: &[String], active: &str) -> Vec<String> {
    names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let marker = if name == active { "*" } else { " " };
            format!("{}.{} {}", idx + 1, marker, name)
        })
        .collect()
}

/// Resolve `/theme` selector as either index or exact name.
pub(crate) fn resolve_theme_selector(names: &[String], selector: &str) -> Result<String, String> {
    let trimmed = selector.trim();
    if trimmed.is_empty() {
        return Err("Usage: /theme <name|index>".to_string());
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 || index > names.len() {
            return Err(format!(
                "Theme index out of range: {index}. Choose 1-{}.",
                names.len()
            ));
        }
        return Ok(names[index - 1].clone());
    }

    let normalized = trimmed.to_ascii_lowercase();
    names
        .iter()
        .find(|name| name.to_ascii_lowercase() == normalized)
        .cloned()
        .ok_or_else(|| format!("Unknown theme `{trimmed}`. Use /theme to pick from themes."))
}

/// Render a sample block set so users can preview current theme output.
pub(crate) fn render_theme_preview(renderer: &dyn RenderSink) {
    renderer.section("theme preview");
    renderer.activity("task #preview running");
    renderer.reasoning_trace(
        "preview reasoning",
        "Inspecting workspace state before executing shell command.",
    );
    renderer.approval_block("$ ls -la\n  # sample approval command");
    renderer.tool_output_block("total 12\n-rw-r--r-- 1 user staff 120 README.md", None);
    renderer.assistant_message(
        "## Preview Title\n- theme token sample\n- inline code: `cargo test`\n> quoted helper text",
    );
    renderer.warn("sample warning");
    renderer.error("sample error");
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_theme_selector_accepts_index_and_name() {
        let names = vec!["dark".to_string(), "light".to_string()];
        assert_eq!(
            resolve_theme_selector(&names, "2").expect("index"),
            "light".to_string()
        );
        assert_eq!(
            resolve_theme_selector(&names, "dark").expect("name"),
            "dark".to_string()
        );
    }

    #[test]
    fn resolve_theme_selector_rejects_unknown_values() {
        let names = vec!["dark".to_string(), "light".to_string()];
        let err = resolve_theme_selector(&names, "nope").expect_err("must reject");
        assert!(err.contains("Unknown theme"));
    }
}
