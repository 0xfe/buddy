//! System prompt templating helpers.
//!
//! The full built-in prompt text lives in one template file and is rendered
//! from a single code path with runtime parameters (tools, target, and
//! optional operator instructions).

use std::collections::BTreeMap;

const SYSTEM_PROMPT_TEMPLATE: &str = include_str!("templates/system_prompt.template");

/// The execution target selected by CLI flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionTarget<'a> {
    Local,
    Container(&'a str),
    Ssh(&'a str),
}

/// Parameters used to compile the system prompt template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemPromptParams<'a> {
    pub execution_target: ExecutionTarget<'a>,
    pub enabled_tools: Vec<&'a str>,
    pub custom_instructions: Option<&'a str>,
}

/// Render the single system prompt template using runtime parameters.
pub fn render_system_prompt(params: SystemPromptParams<'_>) -> String {
    let mut vars = BTreeMap::<&str, String>::new();
    vars.insert(
        "REMOTE_TARGET_NOTE",
        render_remote_target_note(params.execution_target),
    );
    vars.insert(
        "ENABLED_TOOLS_LIST",
        render_enabled_tools(&params.enabled_tools),
    );
    vars.insert(
        "CUSTOM_INSTRUCTIONS_BLOCK",
        render_custom_instructions(params.custom_instructions),
    );

    normalize_blank_lines(&render_template(SYSTEM_PROMPT_TEMPLATE, &vars))
}

fn render_template(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut rendered = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{{{key}}}}}");
        rendered = rendered.replace(&placeholder, value);
    }
    rendered
}

fn render_remote_target_note(target: ExecutionTarget<'_>) -> String {
    match target {
        ExecutionTarget::Local => String::new(),
        ExecutionTarget::Container(name) => format!(
            "You are currently operating against a remote container target (`{name}`).\n\
             The `run_shell`, `read_file`, and `write_file` tools (plus tmux tools like \
             `capture-pane`/`send-keys` when available) act on that remote target, not on \
             the local host running this agent.\n\
             Treat this conversation as targeting the remote environment unless the user \
             explicitly says otherwise."
        ),
        ExecutionTarget::Ssh(name) => format!(
            "You are currently operating against a remote SSH host target (`{name}`).\n\
             The `run_shell`, `read_file`, and `write_file` tools (plus tmux tools like \
             `capture-pane`/`send-keys` when available) act on that remote target, not on \
             the local host running this agent.\n\
             Treat this conversation as targeting the remote environment unless the user \
             explicitly says otherwise."
        ),
    }
}

fn render_enabled_tools(enabled_tools: &[&str]) -> String {
    if enabled_tools.is_empty() {
        return "- none".to_string();
    }

    enabled_tools
        .iter()
        .map(|name| format!("- `{name}`"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_custom_instructions(custom: Option<&str>) -> String {
    let Some(custom) = custom.map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    format!("Additional operator instructions:\n{custom}")
}

fn normalize_blank_lines(text: &str) -> String {
    let mut out = String::new();
    let mut previous_blank = false;

    for line in text.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && previous_blank {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
        previous_blank = is_blank;
    }

    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_static_core_text() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell", "read_file"],
            custom_instructions: None,
        });

        assert!(prompt.contains("friendly systems engineer"));
        assert!(prompt.contains("try `rg` before `grep`"));
        assert!(prompt.contains("`wc`"));
        assert!(prompt.contains("`cat -l`"));
        assert!(prompt.contains("`patch`"));
        assert!(prompt.contains("valid Markdown"));
    }

    #[test]
    fn prompt_renders_remote_note_for_container() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Container("devbox"),
            enabled_tools: vec![],
            custom_instructions: None,
        });
        assert!(prompt.contains("remote container target (`devbox`)"));
    }

    #[test]
    fn prompt_renders_remote_note_for_ssh() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Ssh("user@host"),
            enabled_tools: vec![],
            custom_instructions: None,
        });
        assert!(prompt.contains("remote SSH host target (`user@host`)"));
    }

    #[test]
    fn prompt_renders_enabled_tools_list() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell", "capture-pane", "time"],
            custom_instructions: None,
        });
        assert!(prompt.contains("- `run_shell`"));
        assert!(prompt.contains("- `capture-pane`"));
        assert!(prompt.contains("- `time`"));
    }

    #[test]
    fn prompt_renders_custom_instructions() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec![],
            custom_instructions: Some("Always summarize in one sentence."),
        });
        assert!(prompt.contains("Additional operator instructions:"));
        assert!(prompt.contains("Always summarize in one sentence."));
    }
}
