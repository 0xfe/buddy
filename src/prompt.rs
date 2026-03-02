//! System prompt templating helpers.
//!
//! The full built-in prompt text lives in one template file and is rendered
//! from a single code path with runtime parameters (tools, target, and
//! optional operator instructions).

use std::collections::BTreeMap;

/// Embedded prompt template rendered at runtime with environment/tool context.
const SYSTEM_PROMPT_TEMPLATE: &str = include_str!("templates/system_prompt.template");

/// The execution target selected by CLI flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionTarget<'a> {
    /// Operate on the current local machine.
    Local,
    /// Operate against a named container target.
    Container(&'a str),
    /// Operate against a remote SSH host.
    Ssh(&'a str),
}

/// Parameters used to compile the system prompt template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemPromptParams<'a> {
    /// Runtime execution destination used to render target-specific cautions.
    pub execution_target: ExecutionTarget<'a>,
    /// Tool names exposed to the model in this session.
    pub enabled_tools: Vec<&'a str>,
    /// Optional operator-supplied additive instructions.
    pub custom_instructions: Option<&'a str>,
}

/// Render the single system prompt template using runtime parameters.
pub fn render_system_prompt(params: SystemPromptParams<'_>) -> String {
    // Keep variable binding deterministic for stable renders in tests and logs.
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

/// Replace `{{KEY}}` placeholders in `template` with values from `vars`.
fn render_template(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut rendered = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{{{key}}}}}");
        rendered = rendered.replace(&placeholder, value);
    }
    rendered
}

/// Render a contextual reminder when tools are pointed at non-local environments.
fn render_remote_target_note(target: ExecutionTarget<'_>) -> String {
    match target {
        ExecutionTarget::Local => String::new(),
        ExecutionTarget::Container(name) => format!(
            "You are currently operating against a remote container target (`{name}`).\n\
             The `run_shell`, `read_file`, and `write_file` tools (plus tmux tools like \
             `tmux_capture_pane`/`tmux_send_keys` when available) act on that remote target, not on \
             the local host running this agent.\n\
             Treat this conversation as targeting the remote environment unless the user \
             explicitly says otherwise."
        ),
        ExecutionTarget::Ssh(name) => format!(
            "You are currently operating against a remote SSH host target (`{name}`).\n\
             The `run_shell`, `read_file`, and `write_file` tools (plus tmux tools like \
             `tmux_capture_pane`/`tmux_send_keys` when available) act on that remote target, not on \
             the local host running this agent.\n\
             Treat this conversation as targeting the remote environment unless the user \
             explicitly says otherwise."
        ),
    }
}

/// Render the enabled tool list expected by the system prompt template.
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

/// Render the optional operator instructions block when non-empty.
fn render_custom_instructions(custom: Option<&str>) -> String {
    let Some(custom) = custom.map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    format!(
        "## Operator Instructions (Additive)\n\
The following operator instructions are additional constraints for this run:\n\
```text\n\
{custom}\n\
```\n\
Conflict policy:\n\
- Apply these only when consistent with higher-priority safety/protocol rules.\n\
- If they conflict with an explicit user request, ask for clarification unless \
safety requires immediate refusal.\n\
- If they conflict with system/tool policy, follow system/tool policy and state \
that briefly."
    )
}

/// Collapse repeated blank lines and trim trailing whitespace per line.
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

    // Ensures template render preserves core static policy text.
    #[test]
    fn prompt_contains_static_core_text() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell", "read_file"],
            custom_instructions: None,
        });

        assert!(prompt.contains("## Role"));
        assert!(prompt.contains("friendly systems engineer"));
        assert!(prompt.contains("## Rule Priority"));
        assert!(prompt.contains("Default to action over suggestion"));
        assert!(prompt.contains("prefer `rg` before `grep`"));
        assert!(prompt.contains("`wc`"));
        assert!(prompt.contains("`cat -l`"));
        assert!(prompt.contains("`patch`"));
        assert!(prompt.contains("valid Markdown"));
    }

    // Ensures container mode injects a remote-target warning block.
    #[test]
    fn prompt_renders_remote_note_for_container() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Container("devbox"),
            enabled_tools: vec![],
            custom_instructions: None,
        });
        assert!(prompt.contains("remote container target (`devbox`)"));
    }

    // Ensures SSH mode injects the corresponding remote-target warning block.
    #[test]
    fn prompt_renders_remote_note_for_ssh() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Ssh("user@host"),
            enabled_tools: vec![],
            custom_instructions: None,
        });
        assert!(prompt.contains("remote SSH host target (`user@host`)"));
    }

    // Confirms tool names are emitted as a Markdown bullet list.
    #[test]
    fn prompt_renders_enabled_tools_list() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell", "tmux_capture_pane", "time"],
            custom_instructions: None,
        });
        assert!(prompt.contains("- `run_shell`"));
        assert!(prompt.contains("- `tmux_capture_pane`"));
        assert!(prompt.contains("- `time`"));
    }

    // Ensures optional operator instructions are included verbatim when set.
    #[test]
    fn prompt_renders_custom_instructions() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec![],
            custom_instructions: Some("Always summarize in one sentence."),
        });
        assert!(prompt.contains("## Operator Instructions (Additive)"));
        assert!(prompt.contains("Conflict policy:"));
        assert!(prompt.contains("```text"));
        assert!(prompt.contains("Always summarize in one sentence."));
    }

    // Ensures prompt sections render in stable priority-first order.
    #[test]
    fn prompt_sections_render_in_deterministic_order() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell"],
            custom_instructions: None,
        });
        let role = prompt.find("## Role").expect("role section");
        let priority = prompt.find("## Rule Priority").expect("priority section");
        let behavior = prompt.find("## Core Behavior").expect("behavior section");
        let planning = prompt
            .find("## Plan Before Tool Actions")
            .expect("planning section");
        let tmux = prompt
            .find("## tmux Execution Model")
            .expect("tmux section");
        let guide = prompt
            .find("## Tool Choice Quick Guide")
            .expect("guide section");
        let enabled = prompt
            .find("## Enabled Tools")
            .expect("enabled tools section");
        let final_checklist = prompt.find("## Final Checklist").expect("final checklist");

        assert!(role < priority);
        assert!(priority < behavior);
        assert!(behavior < planning);
        assert!(planning < tmux);
        assert!(tmux < guide);
        assert!(guide < enabled);
        assert!(enabled < final_checklist);
    }

    // Ensures run_shell vs tmux_send_keys guidance remains explicit for tool routing.
    #[test]
    fn prompt_contains_tool_choice_scenarios() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell", "tmux_capture_pane", "tmux_send_keys"],
            custom_instructions: None,
        });
        assert!(prompt.contains("Use `run_shell` to execute shell commands"));
        assert!(prompt.contains("Use `tmux_capture_pane` to observe in-progress"));
        assert!(prompt.contains("Use `tmux_send_keys` to control interactive/stuck"));
        assert!(prompt.contains("Before the first tool call for a non-trivial request"));
    }

    // Snapshot guard for the default local prompt shape and wording.
    #[test]
    fn prompt_matches_local_snapshot() {
        let prompt = render_system_prompt(SystemPromptParams {
            execution_target: ExecutionTarget::Local,
            enabled_tools: vec!["run_shell", "read_file", "tmux_capture_pane"],
            custom_instructions: None,
        });
        let expected = include_str!("templates/system_prompt.snapshot.local.txt")
            .trim()
            .to_string();
        assert_eq!(prompt, expected);
    }
}
