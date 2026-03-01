//! Interactive `buddy init` orchestration and first-run bootstrap helpers.
//!
//! The flow is intentionally state-oriented:
//! - first-run auto bootstrap only triggers when no config source exists,
//! - explicit `buddy init` supports update/overwrite/cancel for existing files,
//! - interactive questions stay narrow (model selection + login guidance).

use crate::app::commands::model::{configured_model_profile_names, model_picker_options};
use crate::cli;
use buddy::config::{
    default_global_config_path, initialize_default_global_config, load_config, persist_agent_model,
    AuthMode, Config, GlobalConfigInitResult,
};
use buddy::ui::render::RenderSink;
use buddy::ui::terminal as term_ui;
use std::io::IsTerminal;
use std::path::Path;

/// Existing-config action requested by the user during `buddy init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingConfigAction {
    /// Keep file content and update selected user-facing values.
    Update,
    /// Rewrite from template after creating a backup.
    Overwrite,
    /// Abort init without mutating anything.
    Cancel,
}

/// Init invocation context (manual subcommand vs startup auto-bootstrap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitInvocation {
    /// `buddy init` command with optional force overwrite.
    Manual { force: bool },
    /// First-run startup bootstrap when no config exists anywhere.
    AutoBootstrap,
}

/// Run first-run bootstrap only when no config source is currently present.
pub(crate) fn maybe_run_auto_init(
    renderer: &dyn RenderSink,
    args: &cli::Args,
) -> Result<(), String> {
    let Some(global_path) = default_global_config_path() else {
        return Ok(());
    };
    let should_bootstrap = should_auto_init(
        args.config.as_deref(),
        Path::new("buddy.toml").exists(),
        Path::new("agent.toml").exists(),
        global_path.exists(),
    );
    if !should_bootstrap {
        return Ok(());
    }

    renderer.section("first-run setup");
    renderer.detail("No buddy config was found. Starting guided setup.");
    eprintln!();
    run_init_flow(renderer, InitInvocation::AutoBootstrap)
}

/// Run the user-facing init flow.
pub(crate) fn run_init_flow(
    renderer: &dyn RenderSink,
    invocation: InitInvocation,
) -> Result<(), String> {
    let path = default_global_config_path().ok_or_else(|| {
        "unable to resolve default config path for ~/.config/buddy/buddy.toml".to_string()
    })?;
    let path_display = path.display().to_string();
    let interactive = is_interactive_terminal();

    match invocation {
        InitInvocation::AutoBootstrap => {
            apply_init_result(
                renderer,
                initialize_default_global_config(false)
                    .map_err(|e| format!("failed to initialize ~/.config/buddy: {e}"))?,
            );
            if interactive {
                run_interactive_update(renderer, &path_display)?;
            } else {
                renderer
                    .detail("Non-interactive terminal detected; keeping default config values.");
                eprintln!();
            }
            Ok(())
        }
        InitInvocation::Manual { force } => {
            if force {
                apply_init_result(
                    renderer,
                    initialize_default_global_config(true)
                        .map_err(|e| format!("failed to initialize ~/.config/buddy: {e}"))?,
                );
                if interactive {
                    run_interactive_update(renderer, &path_display)?;
                }
                return Ok(());
            }

            if !path.exists() {
                apply_init_result(
                    renderer,
                    initialize_default_global_config(false)
                        .map_err(|e| format!("failed to initialize ~/.config/buddy: {e}"))?,
                );
                if interactive {
                    run_interactive_update(renderer, &path_display)?;
                }
                return Ok(());
            }

            if !interactive {
                return Err(format!(
                    "buddy is already initialized at {}. Re-run with an interactive terminal, or use `buddy init --force` to overwrite.",
                    path_display
                ));
            }

            let action = prompt_existing_config_action()?;
            match action {
                ExistingConfigAction::Update => {
                    renderer.section("updating buddy config");
                    renderer.field("path", &path_display);
                    eprintln!();
                    run_interactive_update(renderer, &path_display)
                }
                ExistingConfigAction::Overwrite => {
                    apply_init_result(
                        renderer,
                        initialize_default_global_config(true)
                            .map_err(|e| format!("failed to initialize ~/.config/buddy: {e}"))?,
                    );
                    run_interactive_update(renderer, &path_display)
                }
                ExistingConfigAction::Cancel => {
                    renderer.warn("init cancelled. No changes were made.");
                    Ok(())
                }
            }
        }
    }
}

/// True when startup should trigger first-run bootstrap.
fn should_auto_init(
    config_override: Option<&str>,
    local_buddy_exists: bool,
    local_legacy_exists: bool,
    global_buddy_exists: bool,
) -> bool {
    if config_override.is_some() {
        return false;
    }
    if local_buddy_exists || local_legacy_exists {
        return false;
    }
    !global_buddy_exists
}

/// Render init-result status consistently for create/overwrite/no-op outcomes.
fn apply_init_result(renderer: &dyn RenderSink, result: GlobalConfigInitResult) {
    match result {
        GlobalConfigInitResult::Created { path } => {
            renderer.section("initialized buddy config");
            renderer.field("path", &path.display().to_string());
            eprintln!();
        }
        GlobalConfigInitResult::Overwritten { path, backup_path } => {
            renderer.section("reinitialized buddy config");
            renderer.field("path", &path.display().to_string());
            renderer.field("backup", &backup_path.display().to_string());
            eprintln!();
        }
        GlobalConfigInitResult::AlreadyInitialized { path } => {
            renderer.section("buddy config already initialized");
            renderer.field("path", &path.display().to_string());
            eprintln!();
        }
    }
}

/// Run interactive model-selection + login-guidance update for one config path.
fn run_interactive_update(renderer: &dyn RenderSink, config_path: &str) -> Result<(), String> {
    let mut config = load_config(Some(config_path))
        .map_err(|err| format!("failed to load config for init update: {err}"))?;
    let names = configured_model_profile_names(&config);
    if names.is_empty() {
        return Err("no model profiles are configured in buddy.toml".to_string());
    }

    let options = model_picker_options(&config, &names);
    let initial = names
        .iter()
        .position(|name| name == &config.agent.model)
        .unwrap_or(0);
    let selected_idx = term_ui::pick_from_list(
        config.display.color,
        "default model profile",
        "Use ↑/↓ to pick the default profile, Enter to confirm, Esc to keep current.",
        &options,
        initial,
    )
    .map_err(|err| format!("failed to read model selection: {err}"))?;

    if let Some(index) = selected_idx {
        let selected = &names[index];
        if selected != &config.agent.model {
            persist_agent_model(Some(config_path), selected)
                .map_err(|err| format!("failed to persist agent.model: {err}"))?;
            config.agent.model = selected.clone();
            renderer.section("updated default model");
            renderer.field("model", selected);
            renderer.field("saved_to", config_path);
            eprintln!();
        } else {
            renderer.section("default model unchanged");
            renderer.field("model", selected);
            eprintln!();
        }
    }

    render_login_guidance(renderer, &config);
    Ok(())
}

/// Ask which path to take when config already exists.
fn prompt_existing_config_action() -> Result<ExistingConfigAction, String> {
    let options = vec![
        "1. Update existing config (recommended)".to_string(),
        "2. Overwrite with fresh template (creates backup)".to_string(),
        "3. Cancel".to_string(),
    ];
    let selected = term_ui::pick_from_list(
        true,
        "buddy init",
        "Choose setup mode for the existing config file.",
        &options,
        0,
    )
    .map_err(|err| format!("failed to read init action: {err}"))?;
    Ok(match selected {
        Some(0) => ExistingConfigAction::Update,
        Some(1) => ExistingConfigAction::Overwrite,
        _ => ExistingConfigAction::Cancel,
    })
}

/// Render model-auth guidance and prompt for optional immediate login action.
fn render_login_guidance(renderer: &dyn RenderSink, config: &Config) {
    let Some(active_profile) = config.models.get(&config.agent.model) else {
        return;
    };
    if active_profile.auth != AuthMode::Login {
        renderer.section("login setup");
        renderer.detail("Selected profile uses API-key auth.");
        renderer.detail("Set api_key, api_key_env, or api_key_file in buddy.toml.");
        eprintln!();
        return;
    }

    let login_now = term_ui::pick_from_list(
        config.display.color,
        "login setup",
        "Selected profile uses auth=login. Do you want login instructions now?",
        &["1. Yes".to_string(), "2. Not now".to_string()],
        0,
    );
    let wants_login_now = matches!(login_now, Ok(Some(0)));
    renderer.section("login setup");
    renderer.field("profile", &config.agent.model);
    if wants_login_now {
        renderer.detail(&format!(
            "run `buddy login {}` now (or `/login {}` inside REPL).",
            config.agent.model, config.agent.model
        ));
    } else {
        renderer.detail(&format!(
            "when ready, run `buddy login {}` (or `/login {}` inside REPL).",
            config.agent.model, config.agent.model
        ));
    }
    eprintln!();
}

/// True when stdin/stderr support interactive terminal UI.
fn is_interactive_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::should_auto_init;

    #[test]
    fn auto_init_triggers_only_when_no_sources_exist() {
        assert!(should_auto_init(None, false, false, false));
        assert!(!should_auto_init(Some("custom.toml"), false, false, false));
        assert!(!should_auto_init(None, true, false, false));
        assert!(!should_auto_init(None, false, true, false));
        assert!(!should_auto_init(None, false, false, true));
    }
}
