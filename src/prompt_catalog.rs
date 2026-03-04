//! Parameterized prompt snippet catalog.
//!
//! Prompt-heavy modules load reusable strings from `templates/prompts.toml`
//! so wording stays centralized and easier to audit without scattering large
//! literals across runtime code.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Embedded prompt-catalog template source.
const PROMPTS_TOML: &str = include_str!("templates/prompts.toml");

/// Parsed catalog schema for prompt snippets.
#[derive(Debug, Deserialize)]
struct PromptCatalog {
    /// Indexed prompt templates keyed by stable ids.
    templates: BTreeMap<String, String>,
}

/// Lazily parsed prompt catalog singleton.
static PROMPT_CATALOG: OnceLock<PromptCatalog> = OnceLock::new();

/// Fetch one template by id and render `{{KEY}}` placeholders.
pub(crate) fn render_prompt_template(id: &str, vars: &[(&str, &str)]) -> String {
    let template = prompt_catalog()
        .templates
        .get(id)
        .unwrap_or_else(|| panic!("missing prompt template `{id}`"));
    render_template(template, vars)
}

fn prompt_catalog() -> &'static PromptCatalog {
    PROMPT_CATALOG.get_or_init(|| {
        toml::from_str(PROMPTS_TOML).expect("failed to parse src/templates/prompts.toml")
    })
}

fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{{{key}}}}}");
        rendered = rendered.replace(&placeholder, value);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::render_prompt_template;

    #[test]
    fn renders_prompt_templates_with_placeholders() {
        let rendered = render_prompt_template(
            "dynamic_non_default_tmux_target_context",
            &[
                ("TOOL_NAME", "tmux_capture_pane"),
                ("TARGET_LABEL", "pane=build"),
            ],
        );
        assert!(rendered.contains("last_tool: tmux_capture_pane"));
        assert!(rendered.contains("last_target: pane=build"));
    }
}
