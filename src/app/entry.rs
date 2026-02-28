//! Application entry orchestration for the buddy CLI.

#[cfg(test)]
use crate::app::commands::model::handle_model_command;
use crate::app::commands::model::{configured_model_profile_names, resolve_model_profile_selector};
#[cfg(test)]
use crate::app::commands::session::handle_session_command;
use crate::app::commands::session::resume_request_from_command;
#[cfg(test)]
use crate::app::tasks::background_liveness_line;
#[cfg(test)]
use crate::app::tasks::{process_runtime_events, ProcessRuntimeEventsContext};
use crate::cli;
use buddy::agent::Agent;
use buddy::auth::{
    complete_openai_device_login, has_legacy_profile_token_records,
    login_provider_key_for_base_url, provider_login_health, reset_provider_tokens,
    save_provider_tokens, start_openai_device_login, supports_openai_login, try_open_browser,
};
use buddy::config::ensure_default_global_config;
use buddy::config::initialize_default_global_config;
use buddy::config::load_config_with_diagnostics;
use buddy::config::select_model_profile;
use buddy::config::{AuthMode, Config, GlobalConfigInitResult, ToolsConfig};
use buddy::preflight::validate_active_profile_ready;
use buddy::prompt::{render_system_prompt, ExecutionTarget, SystemPromptParams};
#[cfg(test)]
use buddy::repl::{
    apply_task_timeout_command, mark_task_waiting_for_approval, parse_duration_arg,
    parse_shell_tool_result, tool_result_display_text, update_approval_policy, ApprovalDecision,
};
#[cfg(test)]
use buddy::repl::{
    parse_approval_decision, task_is_waiting_for_approval, ApprovalPolicy, BackgroundTask,
    BackgroundTaskState, RuntimeContextState,
};
#[cfg(test)]
use buddy::runtime::BuddyRuntimeHandle;
#[cfg(test)]
use buddy::runtime::RuntimeCommand;
#[cfg(test)]
use buddy::runtime::{RuntimeEvent, RuntimeEventEnvelope};
#[cfg(test)]
use buddy::session::SessionStore;
use buddy::tools::capture_pane::CapturePaneTool;
use buddy::tools::execution::ExecutionContext;
use buddy::tools::fetch::FetchTool;
use buddy::tools::files::{ReadFileTool, WriteFileTool};
use buddy::tools::search::WebSearchTool;
use buddy::tools::send_keys::SendKeysTool;
use buddy::tools::shell::{ShellApprovalBroker, ShellTool};
use buddy::tools::time::TimeTool;
use buddy::tools::ToolRegistry;
use buddy::ui::render::{RenderSink, Renderer};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::sync::mpsc;

/// Enforce non-interactive `exec` safety defaults for shell tool confirmations.
fn enforce_exec_shell_guardrails(
    is_exec_command: bool,
    dangerously_auto_approve: bool,
    tools: &mut ToolsConfig,
) -> Result<Option<String>, String> {
    if !is_exec_command || !tools.shell_enabled || !tools.shell_confirm {
        return Ok(None);
    }
    if dangerously_auto_approve {
        tools.shell_confirm = false;
        return Ok(Some(
            "Dangerous mode enabled: auto-approving run_shell commands for this exec invocation."
                .to_string(),
        ));
    }
    Err(
        "buddy exec is non-interactive and fails closed when tools.shell_confirm=true. Run interactive buddy, set tools.shell_confirm=false, or pass --dangerously-auto-approve."
            .to_string(),
    )
}

/// Top-level CLI entrypoint that dispatches init/login/exec/repl flows.
pub(crate) async fn run(args: crate::cli::Args) -> i32 {
    // Entrypoint walkthrough:
    // 1) handle init/login early-return commands,
    // 2) load + validate config/runtime setup,
    // 3) dispatch into one-shot exec mode or interactive REPL mode.
    let bootstrap_renderer = Renderer::new(!args.no_color);
    if let Some(cli::Command::Init { force }) = args.command.as_ref() {
        if let Err(msg) = run_init_flow(&bootstrap_renderer, *force) {
            bootstrap_renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    let loaded = match load_config_state(&args) {
        Ok(state) => state,
        Err(msg) => {
            bootstrap_renderer.error(&msg);
            return 1;
        }
    };
    let renderer = Renderer::new(loaded.config.display.color);
    for warning in &loaded.warnings {
        renderer.warn(warning);
    }

    if let Some(cli::Command::Login {
        model,
        reset,
        check,
    }) = args.command.as_ref()
    {
        if let Err(msg) =
            run_login_flow(&renderer, &loaded.config, model.as_deref(), *reset, *check).await
        {
            renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    let runtime_setup = match prepare_runtime_setup(&args, &renderer, loaded).await {
        Ok(setup) => setup,
        Err(msg) => {
            renderer.error(&msg);
            return 1;
        }
    };

    if let Some(cli::Command::Exec { prompt }) = args.command.as_ref() {
        return crate::app::exec_mode::run_exec_mode(
            &renderer,
            runtime_setup.agent,
            runtime_setup.config,
            prompt.clone(),
        )
        .await;
    }

    crate::app::repl_mode::run_repl_mode(crate::app::repl_mode::ReplModeInputs {
        renderer: &renderer,
        cli_args: &args,
        config: runtime_setup.config,
        execution: runtime_setup.execution,
        capture_pane_enabled: runtime_setup.capture_pane_enabled,
        agent: runtime_setup.agent,
        resume_request: runtime_setup.resume_request,
        shell_approval_rx: runtime_setup.shell_approval_rx,
    })
    .await
}

/// Loaded config plus derived warnings/startup resume request.
struct LoadedConfigState {
    /// Effective runtime configuration after CLI overrides.
    config: Config,
    /// User-facing warnings collected during config/auth diagnostics.
    warnings: Vec<String>,
    /// Optional session resume request parsed from CLI command.
    resume_request: Option<buddy::repl::ResumeRequest>,
}

/// Runtime-ready wiring produced before entering exec/repl flows.
struct RuntimeSetup {
    /// Effective runtime configuration.
    config: Config,
    /// Execution backend context (local/ssh/container/tmux).
    execution: ExecutionContext,
    /// Whether capture-pane/send-keys can be exposed.
    capture_pane_enabled: bool,
    /// Agent instance bound to the configured tool registry.
    agent: Agent,
    /// Optional startup session resume request.
    resume_request: Option<buddy::repl::ResumeRequest>,
    /// Shell approval request stream when interactive confirmations are enabled.
    shell_approval_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<buddy::tools::shell::ShellApprovalRequest>>,
}

/// Tool registry plus optional shell-approval channel wiring.
struct ToolSetup {
    /// Registered tool implementations for the agent.
    tools: ToolRegistry,
    /// Shell approval request receiver, if confirmations are enabled.
    shell_approval_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<buddy::tools::shell::ShellApprovalRequest>>,
}

/// Load config, apply CLI overrides, and collect startup diagnostics/warnings.
fn load_config_state(args: &crate::cli::Args) -> Result<LoadedConfigState, String> {
    if let Err(err) = ensure_default_global_config() {
        eprintln!("warning: failed to initialize ~/.config/buddy/buddy.toml: {err}");
    }

    let loaded =
        load_config_with_diagnostics(args.config.as_deref()).map_err(|err| err.to_string())?;
    let mut config = loaded.config;
    apply_cli_overrides(args, &mut config)?;

    let mut warnings = loaded.diagnostics.deprecations;
    match has_legacy_profile_token_records() {
        Ok(true) => warnings.push(
            "Auth store uses deprecated profile-scoped login records; run `buddy login` to migrate to provider-scoped records before v0.4."
                .to_string(),
        ),
        Ok(false) => {}
        Err(err) => warnings.push(format!(
            "failed to inspect auth store for legacy credentials: {err}"
        )),
    }
    let resume_request = resume_request_from_command(args.command.as_ref())?;

    Ok(LoadedConfigState {
        config,
        warnings,
        resume_request,
    })
}

/// Apply CLI runtime overrides that intentionally outrank config files.
fn apply_cli_overrides(args: &crate::cli::Args, config: &mut Config) -> Result<(), String> {
    if let Some(model) = &args.model {
        if config.models.contains_key(model) {
            select_model_profile(config, model)
                .map_err(|err| format!("failed to select model profile `{model}`: {err}"))?;
        } else {
            // Backward-compatible behavior: if the argument is not a configured
            // profile key, treat it as a direct API model-id override.
            config.api.model = model.clone();
        }
    }
    if let Some(url) = &args.base_url {
        config.api.base_url = url.clone();
    }
    if args.no_color {
        config.display.color = false;
    }
    Ok(())
}

/// Validate runtime prerequisites and build execution/tools/agent wiring.
async fn prepare_runtime_setup(
    args: &crate::cli::Args,
    renderer: &dyn RenderSink,
    mut loaded: LoadedConfigState,
) -> Result<RuntimeSetup, String> {
    // Setup sequence:
    // 1) validate profile and execution flags,
    // 2) initialize execution context,
    // 3) render system prompt with tool target metadata,
    // 4) build tool registry and agent.
    if loaded.config.api.base_url.is_empty() {
        return Err(
            "No API base URL configured. Set models.<name>.api_base_url in buddy.toml or BUDDY_BASE_URL env var."
                .to_string(),
        );
    }
    validate_active_profile_ready(&loaded.config)?;

    let is_exec_command = matches!(args.command.as_ref(), Some(cli::Command::Exec { .. }));
    match enforce_exec_shell_guardrails(
        is_exec_command,
        args.dangerously_auto_approve,
        &mut loaded.config.tools,
    ) {
        Ok(Some(warning)) => renderer.warn(&warning),
        Ok(None) => {}
        Err(msg) => return Err(msg),
    }
    validate_execution_target_flags(args, &loaded.config)?;

    let execution = initialize_execution_context(args, &loaded.config).await?;
    let capture_pane_enabled = execution.capture_pane_available();
    configure_system_prompt(&mut loaded.config, args, capture_pane_enabled);
    let tool_setup = build_tools(
        &loaded.config,
        &execution,
        !is_exec_command,
        capture_pane_enabled,
    );
    let agent = Agent::new(loaded.config.clone(), tool_setup.tools);

    Ok(RuntimeSetup {
        config: loaded.config,
        execution,
        capture_pane_enabled,
        agent,
        resume_request: loaded.resume_request,
        shell_approval_rx: tool_setup.shell_approval_rx,
    })
}

/// Validate CLI execution-target flags against enabled tool capabilities.
fn validate_execution_target_flags(args: &crate::cli::Args, config: &Config) -> Result<(), String> {
    if (args.container.is_some() || args.ssh.is_some() || args.tmux.is_some())
        && !config.tools.shell_enabled
        && !config.tools.files_enabled
    {
        return Err(
            "Execution target flags (--container/--ssh/--tmux) require `run_shell` or file tools to be enabled. Enable `tools.shell_enabled` and/or `tools.files_enabled` in config."
                .to_string(),
        );
    }
    Ok(())
}

/// Build execution context from CLI target flags and current config.
async fn initialize_execution_context(
    args: &crate::cli::Args,
    config: &Config,
) -> Result<ExecutionContext, String> {
    let execution_tools_enabled = config.tools.shell_enabled || config.tools.files_enabled;
    let requested_tmux_session = args.tmux.clone().flatten();
    if !execution_tools_enabled {
        return Ok(ExecutionContext::local());
    }

    if let Some(container) = &args.container {
        return ExecutionContext::container_tmux(
            container.clone(),
            requested_tmux_session,
            &config.agent.name,
        )
        .await
        .map_err(|err| format!("failed to initialize container execution: {err}"));
    }

    if let Some(target) = &args.ssh {
        return ExecutionContext::ssh(target.clone(), requested_tmux_session, &config.agent.name)
            .await
            .map_err(|err| format!("failed to initialize ssh execution: {err}"));
    }

    ExecutionContext::local_tmux(requested_tmux_session, &config.agent.name)
        .await
        .map_err(|err| format!("failed to initialize local tmux execution: {err}"))
}

/// Render and install the final system prompt string for this invocation.
fn configure_system_prompt(
    config: &mut Config,
    args: &crate::cli::Args,
    capture_pane_enabled: bool,
) {
    let prompt_tool_names = enabled_tool_names(config, capture_pane_enabled);
    let custom_prompt = config.agent.system_prompt.trim().to_string();
    config.agent.system_prompt = render_system_prompt(SystemPromptParams {
        execution_target: if let Some(container) = args.container.as_deref() {
            ExecutionTarget::Container(container)
        } else if let Some(host) = args.ssh.as_deref() {
            ExecutionTarget::Ssh(host)
        } else {
            ExecutionTarget::Local
        },
        enabled_tools: prompt_tool_names,
        custom_instructions: (!custom_prompt.is_empty()).then_some(custom_prompt.as_str()),
    });
}

/// Register tools according to config flags and execution capabilities.
fn build_tools(
    config: &Config,
    execution: &ExecutionContext,
    interactive_mode: bool,
    capture_pane_enabled: bool,
) -> ToolSetup {
    let mut tools = ToolRegistry::new();
    let needs_approval_broker = interactive_mode
        && ((config.tools.shell_enabled && config.tools.shell_confirm)
            || (config.tools.fetch_enabled && config.tools.fetch_confirm));
    let (shell_approval_broker, shell_approval_rx) = if needs_approval_broker {
        let (broker, rx) = ShellApprovalBroker::channel();
        (Some(broker), Some(rx))
    } else {
        (None, None)
    };

    if config.tools.shell_enabled {
        tools.register(ShellTool {
            confirm: config.tools.shell_confirm,
            denylist: config.tools.shell_denylist.clone(),
            color: config.display.color,
            execution: execution.clone(),
            approval: shell_approval_broker.clone(),
        });
    }
    if capture_pane_enabled {
        tools.register(CapturePaneTool {
            execution: execution.clone(),
        });
        tools.register(SendKeysTool {
            execution: execution.clone(),
        });
    }
    if config.tools.fetch_enabled {
        tools.register(FetchTool::new(
            Duration::from_secs(config.network.fetch_timeout_secs),
            config.tools.fetch_confirm,
            config.tools.fetch_allowed_domains.clone(),
            config.tools.fetch_blocked_domains.clone(),
            shell_approval_broker,
        ));
    }
    if config.tools.files_enabled {
        tools.register(ReadFileTool {
            execution: execution.clone(),
        });
        tools.register(WriteFileTool {
            execution: execution.clone(),
            allowed_paths: config.tools.files_allowed_paths.clone(),
        });
    }
    if config.tools.search_enabled {
        tools.register(WebSearchTool::new(Duration::from_secs(
            config.network.fetch_timeout_secs,
        )));
    }
    tools.register(TimeTool);

    ToolSetup {
        tools,
        shell_approval_rx,
    }
}

/// Handle `buddy init` flow and render user-facing status messages.
fn run_init_flow(renderer: &dyn RenderSink, force: bool) -> Result<(), String> {
    match initialize_default_global_config(force)
        .map_err(|e| format!("failed to initialize ~/.config/buddy: {e}"))?
    {
        GlobalConfigInitResult::Created { path } => {
            renderer.section("initialized buddy config");
            renderer.field("path", &path.display().to_string());
            eprintln!();
            Ok(())
        }
        GlobalConfigInitResult::Overwritten { path, backup_path } => {
            renderer.section("reinitialized buddy config");
            renderer.field("path", &path.display().to_string());
            renderer.field("backup", &backup_path.display().to_string());
            eprintln!();
            Ok(())
        }
        GlobalConfigInitResult::AlreadyInitialized { path } => Err(format!(
            "buddy is already initialized at {}. Use `buddy init --force` to overwrite.",
            path.display()
        )),
    }
}

/// Handle `buddy login` health/reset/device-auth flow.
pub(crate) async fn run_login_flow(
    renderer: &dyn RenderSink,
    config: &Config,
    selector: Option<&str>,
    reset: bool,
    check: bool,
) -> Result<(), String> {
    // Login flow:
    // 1) select profile/provider and render saved-login health,
    // 2) optionally reset existing credentials,
    // 3) run OpenAI device flow and persist resulting tokens.
    if config.models.is_empty() {
        return Err(
            "No configured model profiles. Add `[models.<name>]` entries to buddy.toml."
                .to_string(),
        );
    }

    let names = configured_model_profile_names(config);
    let profile_name = if let Some(selector) = selector {
        resolve_model_profile_selector(config, &names, selector)?
    } else {
        config.agent.model.clone()
    };

    let Some(profile) = config.models.get(&profile_name) else {
        return Err(format!("unknown profile `{profile_name}`"));
    };
    if !supports_openai_login(&profile.api_base_url) {
        return Err(format!(
            "profile `{profile_name}` points to `{}`. Login auth currently supports OpenAI endpoints only.",
            profile.api_base_url
        ));
    }

    let Some(provider) = login_provider_key_for_base_url(&profile.api_base_url) else {
        return Err(format!(
            "profile `{profile_name}` points to `{}`. Login auth currently supports OpenAI endpoints only.",
            profile.api_base_url
        ));
    };

    let health = provider_login_health(provider)
        .map_err(|err| format!("failed to check existing login health: {err}"))?;
    renderer.section("login health");
    renderer.field("provider", provider);
    renderer.field(
        "saved_credentials",
        if health.has_tokens { "yes" } else { "no" },
    );
    if let Some(expires_at_unix) = health.expires_at_unix {
        renderer.field("expires_at_unix", &expires_at_unix.to_string());
        renderer.field(
            "expiring_soon",
            if health.expiring_soon { "yes" } else { "no" },
        );
    }
    eprintln!();

    if check {
        return Ok(());
    }

    if reset {
        let removed = reset_provider_tokens(provider)
            .map_err(|err| format!("failed to reset saved login credentials: {err}"))?;
        if removed {
            renderer.section("login reset");
            renderer.field("provider", provider);
            renderer.field("status", "removed saved credentials");
            eprintln!();
        } else {
            renderer.section("login reset");
            renderer.field("provider", provider);
            renderer.field("status", "no saved credentials found");
            eprintln!();
        }
    }

    let login = start_openai_device_login()
        .await
        .map_err(|err| format!("failed to start login flow: {err}"))?;

    renderer.section("login");
    renderer.field("profile", &profile_name);
    renderer.field("url", &login.verification_url);
    renderer.field("code", &login.user_code);
    if try_open_browser(&login.verification_url) {
        renderer.field("browser", "opened");
    } else {
        renderer.field("browser", "not available (open URL manually)");
    }
    eprintln!();

    let tokens = {
        let _progress = renderer.progress("waiting for authorization");
        complete_openai_device_login(&login)
            .await
            .map_err(|err| format!("login failed: {err}"))?
    };
    save_provider_tokens(provider, tokens)
        .map_err(|err| format!("failed to save login credentials: {err}"))?;

    renderer.section("login successful");
    renderer.field("profile", &profile_name);
    renderer.field("provider", provider);
    if profile.auth != AuthMode::Login {
        renderer.field(
            "note",
            "this profile currently uses auth=api-key; set auth=login to use saved login tokens",
        );
    }
    eprintln!();

    Ok(())
}

/// Return enabled tool identifiers for prompt rendering.
fn enabled_tool_names(config: &Config, capture_pane_enabled: bool) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if config.tools.shell_enabled {
        tools.push("run_shell");
    }
    if capture_pane_enabled {
        tools.push("capture-pane");
        tools.push("send-keys");
    }
    if config.tools.fetch_enabled {
        tools.push("fetch_url");
    }
    if config.tools.files_enabled {
        tools.push("read_file");
        tools.push("write_file");
    }
    if config.tools.search_enabled {
        tools.push("web_search");
    }
    tools.push("time");
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Clone, Default)]
    struct MockRenderer {
        /// Captured `(kind, message)` render events for assertions.
        entries: Arc<StdMutex<Vec<(String, String)>>>,
    }

    impl MockRenderer {
        /// Record one render event emitted through the `RenderSink` interface.
        fn record(&self, kind: &str, message: &str) {
            self.entries
                .lock()
                .expect("mock renderer lock")
                .push((kind.to_string(), message.to_string()));
        }

        /// Return true when a recorded event of `kind` contains `needle`.
        fn saw(&self, kind: &str, needle: &str) -> bool {
            self.entries
                .lock()
                .expect("mock renderer lock")
                .iter()
                .any(|(k, msg)| k == kind && msg.contains(needle))
        }
    }

    impl RenderSink for MockRenderer {
        fn prompt(&self) {}

        fn assistant_message(&self, content: &str) {
            self.record("assistant", content);
        }

        fn progress(&self, label: &str) -> buddy::ui::render::ProgressHandle {
            self.record("progress", label);
            Renderer::new(false).progress(label)
        }

        fn progress_with_metrics(
            &self,
            label: &str,
            _metrics: buddy::ui::render::ProgressMetrics,
        ) -> buddy::ui::render::ProgressHandle {
            self.record("progress", label);
            Renderer::new(false).progress(label)
        }

        fn header(&self, model: &str) {
            self.record("header", model);
        }

        fn tool_call(&self, name: &str, args: &str) {
            self.record("tool_call", &format!("{name}({args})"));
        }

        fn tool_result(&self, result: &str) {
            self.record("tool_result", result);
        }

        fn token_usage(&self, prompt: u64, completion: u64, session_total: u64) {
            self.record(
                "token_usage",
                &format!("{prompt}/{completion}/{session_total}"),
            );
        }

        fn reasoning_trace(&self, field: &str, trace: &str) {
            self.record("reasoning", &format!("{field}:{trace}"));
        }

        fn warn(&self, msg: &str) {
            self.record("warn", msg);
        }

        fn section(&self, title: &str) {
            self.record("section", title);
        }

        fn activity(&self, text: &str) {
            self.record("activity", text);
        }

        fn field(&self, key: &str, value: &str) {
            self.record("field", &format!("{key}:{value}"));
        }

        fn detail(&self, text: &str) {
            self.record("detail", text);
        }

        fn error(&self, msg: &str) {
            self.record("error", msg);
        }

        fn tool_output_block(&self, text: &str, _syntax_path: Option<&str>) {
            self.record("tool_output", text);
        }

        fn command_output_block(&self, text: &str) {
            self.record("command_output", text);
        }

        fn reasoning_block(&self, text: &str) {
            self.record("reasoning_block", text);
        }

        fn approval_block(&self, text: &str) {
            self.record("approval_block", text);
        }
    }

    #[test]
    fn exec_shell_guardrails_fail_closed_without_override() {
        // Non-interactive exec must fail when shell confirmations are still required.
        let mut tools = ToolsConfig {
            shell_enabled: true,
            shell_confirm: true,
            ..ToolsConfig::default()
        };
        let err = enforce_exec_shell_guardrails(true, false, &mut tools).unwrap_err();
        assert!(
            err.contains("--dangerously-auto-approve"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn exec_shell_guardrails_auto_approve_disables_shell_confirm() {
        // Dangerous override should disable confirm and return an explicit warning.
        let mut tools = ToolsConfig {
            shell_enabled: true,
            shell_confirm: true,
            ..ToolsConfig::default()
        };
        let warning = enforce_exec_shell_guardrails(true, true, &mut tools)
            .expect("guardrail should allow override")
            .expect("warning expected");
        assert!(warning.contains("Dangerous mode"));
        assert!(!tools.shell_confirm);
    }

    #[test]
    fn parse_approval_decision_supports_yes_no_and_default_deny() {
        // Approval parser should accept y/yes/n/empty and reject unrelated input.
        assert_eq!(
            parse_approval_decision("y"),
            Some(ApprovalDecision::Approve)
        );
        assert_eq!(
            parse_approval_decision("YES"),
            Some(ApprovalDecision::Approve)
        );
        assert_eq!(parse_approval_decision("n"), Some(ApprovalDecision::Deny));
        assert_eq!(parse_approval_decision(""), Some(ApprovalDecision::Deny));
        assert_eq!(parse_approval_decision("maybe"), None);
    }

    #[test]
    fn parse_shell_tool_result_extracts_code_stdout_and_stderr() {
        // Legacy text shell payloads should parse into structured exit/stdout/stderr fields.
        let parsed = parse_shell_tool_result("exit code: 7\nstdout:\na\nb\nstderr:\nwarn")
            .expect("shell output should parse");
        assert_eq!(parsed.exit_code, 7);
        assert_eq!(parsed.stdout, "a\nb");
        assert_eq!(parsed.stderr, "warn");
    }

    #[test]
    fn parse_shell_tool_result_rejects_unexpected_shape() {
        // Non-shell payloads should not produce false-positive parses.
        assert!(parse_shell_tool_result("not shell output").is_none());
    }

    #[test]
    fn parse_shell_tool_result_extracts_from_enveloped_json() {
        // JSON-enveloped shell payloads should unwrap and parse successfully.
        let result = serde_json::json!({
            "harness_timestamp": { "source": "harness", "unix_millis": 123 },
            "result": {
                "exit_code": 9,
                "stdout": "a",
                "stderr": "b"
            }
        })
        .to_string();
        let parsed = parse_shell_tool_result(&result).expect("parse shell payload");
        assert_eq!(parsed.exit_code, 9);
        assert_eq!(parsed.stdout, "a");
        assert_eq!(parsed.stderr, "b");
    }

    #[test]
    fn tool_result_display_text_unwraps_envelope_strings() {
        // Envelope helper should return plain string results without metadata wrapper.
        let result = serde_json::json!({
            "harness_timestamp": { "source": "harness", "unix_millis": 123 },
            "result": "hello"
        })
        .to_string();
        assert_eq!(tool_result_display_text(&result), "hello");
    }

    #[test]
    fn background_liveness_line_includes_running_task_state() {
        // Liveness line should include task id and running-state wording.
        let task = BackgroundTask {
            id: 3,
            kind: "prompt".into(),
            details: "demo".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            final_response: None,
        };
        let line = background_liveness_line(&[task]).expect("line expected");
        assert!(line.contains("task #3 running"), "line: {line}");
    }

    #[test]
    fn mark_task_waiting_for_approval_marks_selected_task() {
        // Waiting-approval transition should only apply to the targeted task id.
        let mut tasks = vec![
            BackgroundTask {
                id: 1,
                kind: "prompt".into(),
                details: "old".into(),
                started_at: Instant::now() - Duration::from_secs(2),
                state: BackgroundTaskState::Cancelling {
                    since: Instant::now(),
                },
                timeout_at: None,
                final_response: None,
            },
            BackgroundTask {
                id: 2,
                kind: "prompt".into(),
                details: "new".into(),
                started_at: Instant::now() - Duration::from_secs(1),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            },
        ];
        assert!(mark_task_waiting_for_approval(
            &mut tasks,
            2,
            "ls",
            Some("low".to_string()),
            Some(false),
            Some(false),
            Some("inspect files".to_string()),
        ));
        assert!(task_is_waiting_for_approval(&tasks, 2));
        assert!(!task_is_waiting_for_approval(&tasks, 1));
    }

    #[test]
    fn parse_duration_arg_supports_common_units() {
        // Duration parser should accept seconds/minutes/hours/millis shorthands.
        assert_eq!(parse_duration_arg("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration_arg("30"), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_duration_arg("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse_duration_arg("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration_arg("bad"), None);
    }

    #[test]
    fn update_approval_policy_parses_modes_and_duration() {
        // `/approve` parser should support named modes and duration-based policy.
        let mut policy = ApprovalPolicy::Ask;
        assert!(update_approval_policy("all", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::All));
        assert!(update_approval_policy("none", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::None));
        assert!(update_approval_policy("ask", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::Ask));
        assert!(update_approval_policy("10m", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::Until(_)));
    }

    #[test]
    fn apply_task_timeout_requires_task_id_when_ambiguous() {
        // Timeout command should require explicit id when multiple tasks are running.
        let mut tasks = vec![
            BackgroundTask {
                id: 1,
                kind: "prompt".into(),
                details: "a".into(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            },
            BackgroundTask {
                id: 2,
                kind: "prompt".into(),
                details: "b".into(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            },
        ];
        let err =
            apply_task_timeout_command(&mut tasks, Some("10m"), None).expect_err("should fail");
        assert!(err.contains("Task id required"));
    }

    #[test]
    fn apply_task_timeout_sets_deadline_for_single_task_without_id() {
        // Timeout command may target the only running task when id is omitted.
        let mut tasks = vec![BackgroundTask {
            id: 9,
            kind: "prompt".into(),
            details: "single".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            final_response: None,
        }];
        let ok =
            apply_task_timeout_command(&mut tasks, Some("10m"), None).expect("timeout should set");
        assert!(ok.contains("#9"));
        assert!(tasks[0].timeout_at.is_some());
    }

    #[tokio::test]
    async fn handle_session_command_new_submits_runtime_command() {
        // `/session new` should enqueue a runtime `SessionNew` command.
        let temp = std::env::temp_dir().join(format!(
            "buddy-main-session-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = SessionStore::open(&temp).expect("open session store");
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = Renderer::new(false);
        let mut active = "abcd-1234".to_string();

        handle_session_command(&renderer, &store, &runtime, &mut active, Some("new"), None).await;

        let command = rx.recv().await.expect("command expected");
        assert!(matches!(command, RuntimeCommand::SessionNew));
    }

    #[tokio::test]
    async fn handle_session_command_resume_without_id_warns() {
        // `/session resume` without an id should warn and avoid runtime submission.
        let temp = std::env::temp_dir().join(format!(
            "buddy-main-session-test-missing-id-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = SessionStore::open(&temp).expect("open session store");
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut active = "abcd-1234".to_string();

        handle_session_command(
            &renderer,
            &store,
            &runtime,
            &mut active,
            Some("resume"),
            None,
        )
        .await;

        assert!(
            renderer.saw("warn", "Usage: /session resume <session-id|last>"),
            "expected usage warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[tokio::test]
    async fn handle_session_command_resume_last_without_sessions_warns() {
        // `/session resume last` should warn when no saved sessions exist.
        let temp = std::env::temp_dir().join(format!(
            "buddy-main-session-test-last-empty-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = SessionStore::open(&temp).expect("open session store");
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut active = "abcd-1234".to_string();

        handle_session_command(
            &renderer,
            &store,
            &runtime,
            &mut active,
            Some("resume"),
            Some("last"),
        )
        .await;

        assert!(
            renderer.saw("warn", "No saved sessions found."),
            "expected empty-session warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[tokio::test]
    async fn handle_model_command_submits_switch_command() {
        // `/model <selector>` should submit `SwitchModel` with resolved profile.
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = Renderer::new(false);
        let mut config = Config::default();

        handle_model_command(
            &renderer,
            &mut config,
            &runtime,
            Some("openrouter-deepseek"),
        )
        .await;

        let command = rx.recv().await.expect("command expected");
        match command {
            RuntimeCommand::SwitchModel { profile } => {
                assert_eq!(profile, "openrouter-deepseek");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_model_command_warns_on_unknown_selector() {
        // Unknown model selector should warn and avoid runtime command dispatch.
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut config = Config::default();

        handle_model_command(&renderer, &mut config, &runtime, Some("missing-profile")).await;

        assert!(
            renderer.saw("warn", "Unknown model profile"),
            "expected unknown-profile warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[test]
    fn process_runtime_events_routes_warning_to_injected_renderer() {
        // Runtime warning events should route through the injected render sink.
        let renderer = MockRenderer::default();
        let mut events = vec![RuntimeEventEnvelope {
            seq: 1,
            ts_unix_ms: 1,
            event: RuntimeEvent::Warning(buddy::runtime::WarningEvent {
                task: None,
                message: "demo warning".to_string(),
            }),
        }];
        let mut background_tasks = Vec::new();
        let mut completed_tasks = Vec::new();
        let mut pending_approval = None;
        let mut config = Config::default();
        let mut active_session = "session-x".to_string();
        let mut runtime_context = RuntimeContextState::new(None);

        let mut runtime_event_context = ProcessRuntimeEventsContext {
            renderer: &renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut events, &mut runtime_event_context);

        assert!(
            renderer.saw("warn", "demo warning"),
            "runtime warning should flow through render trait"
        );
    }
}
