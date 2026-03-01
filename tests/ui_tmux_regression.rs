//! On-demand tmux UI regression integration tests.
//!
//! These tests are intentionally ignored by default because they require
//! external tools (`tmux`, `asciinema`) and execute a full terminal flow.

mod ui_tmux;

use serde_json::json;
use std::fs;
use std::time::Duration;
use ui_tmux::{
    assertion_report_json, built_buddy_binary, record_contains_assertion, verify_tooling_prereqs,
    write_ui_regression_config, AssertionRecord, MockModelServer, ScenarioArtifacts, TmuxHarness,
};

/// Full REPL flow regression: startup banner/prompt, spinner, approval, command output, and final prompt.
#[test]
#[ignore = "on-demand tmux/asciinema ui regression suite"]
fn ui_tmux_prompt_spinner_approval_and_output_flow() {
    let scenario = "prompt-spinner-approval-output";
    let artifacts = ScenarioArtifacts::create(scenario).expect("artifacts");
    let mut assertions = Vec::<AssertionRecord>::new();
    let mut error_text: Option<String> = None;

    if let Err(err) = run_scenario(&artifacts, &mut assertions) {
        error_text = Some(err);
    }

    let report = assertion_report_json(scenario, error_text.is_none(), &assertions, &artifacts);
    artifacts.write_report(&report).expect("write report");

    if let Some(err) = error_text {
        panic!(
            "ui regression scenario failed: {err}\nartifacts: {}\nreport: {}",
            artifacts.root.display(),
            artifacts.report_path.display()
        );
    }
}

/// Managed tmux lifecycle flow regression: create pane, run command on that pane, and finish.
#[test]
#[ignore = "on-demand tmux/asciinema ui regression suite"]
fn ui_tmux_managed_pane_create_and_targeted_shell_flow() {
    let scenario = "tmux-management-targeted-shell";
    let artifacts = ScenarioArtifacts::create(scenario).expect("artifacts");
    let mut assertions = Vec::<AssertionRecord>::new();
    let mut error_text: Option<String> = None;

    if let Err(err) = run_tmux_management_scenario(&artifacts, &mut assertions) {
        error_text = Some(err);
    }

    let report = assertion_report_json(scenario, error_text.is_none(), &assertions, &artifacts);
    artifacts.write_report(&report).expect("write report");

    if let Some(err) = error_text {
        panic!(
            "ui regression scenario failed: {err}\nartifacts: {}\nreport: {}",
            artifacts.root.display(),
            artifacts.report_path.display()
        );
    }
}

fn run_scenario(
    artifacts: &ScenarioArtifacts,
    assertions: &mut Vec<AssertionRecord>,
) -> Result<(), String> {
    verify_tooling_prereqs()?;
    let binary = built_buddy_binary()?;
    let server = MockModelServer::start()?;
    write_ui_regression_config(artifacts, &server.base_url_v1())?;

    let session_name = format!("buddy-ui-test-{}", std::process::id());
    let mut tmux = TmuxHarness::start(&session_name)?;
    tmux.enable_pipe_logging(&artifacts.pipe_log_path)?;

    let launch = tmux.launch_buddy_command(&binary, artifacts, "ui-test");
    tmux.send_line(&launch)?;

    let startup_plain = checkpoint_contains(
        &tmux,
        artifacts,
        "startup-ready",
        "used)>",
        Duration::from_secs(45),
        false,
    )?;
    artifacts.save_snapshot("startup-plain", &startup_plain)?;
    record_contains_assertion(
        assertions,
        "startup banner",
        &startup_plain,
        "buddy running on localhost with model ui-test-model",
    )?;
    record_contains_assertion(
        assertions,
        "startup attach line",
        &startup_plain,
        "attach with: tmux attach -t buddy-",
    )?;

    let startup_ansi = tmux.capture_pane(true)?;
    artifacts.save_snapshot("startup-ansi", &startup_ansi)?;
    record_contains_assertion(
        assertions,
        "color escapes present",
        &startup_ansi,
        "\u{1b}[",
    )?;

    tmux.send_line("list files")?;

    let approval = checkpoint_contains(
        &tmux,
        artifacts,
        "approval-ready",
        "approve",
        Duration::from_secs(45),
        false,
    )?;
    artifacts.save_snapshot("approval", &approval)?;
    record_contains_assertion(
        assertions,
        "approval header includes risk",
        &approval,
        "low risk shell command on local (tmux:",
    )?;
    record_contains_assertion(
        assertions,
        "approval command block present",
        &approval,
        "$ sleep 1; printf 'UI_HARNESS_OK\\n'",
    )?;
    record_contains_assertion(assertions, "approval prompt present", &approval, "approve")?;

    tmux.send_line("y")?;

    let completion = checkpoint_contains(
        &tmux,
        artifacts,
        "completion-ready",
        "Harness complete.",
        Duration::from_secs(60),
        false,
    )?;
    artifacts.save_snapshot("completion", &completion)?;
    record_contains_assertion(
        assertions,
        "shell output present",
        &completion,
        "UI_HARNESS_OK",
    )?;
    record_contains_assertion(
        assertions,
        "assistant text present",
        &completion,
        "Harness complete.",
    )?;

    // Ensure liveness status lines showed up while task was running.
    let pipe_log = fs::read_to_string(&artifacts.pipe_log_path)
        .map_err(|e| format!("failed reading pipe-pane log: {e}"))?;
    artifacts.save_snapshot("pipe-log", &pipe_log)?;
    record_contains_assertion(
        assertions,
        "spinner/liveness line",
        &pipe_log,
        "task #1 running",
    )?;

    tmux.send_line("/quit")?;
    let _command =
        tmux.wait_until_command_exits(&["asciinema", "buddy"], Duration::from_secs(30))?;
    tmux.disable_pipe_logging();

    let request_count = server.request_count();
    let request_expect = json!({ "expected_requests": 2, "actual_requests": request_count });
    artifacts.save_snapshot("mock-server-requests", &request_expect.to_string())?;
    if request_count != 2 {
        return Err(format!(
            "mock server request count mismatch: expected 2, got {request_count}"
        ));
    }

    Ok(())
}

fn run_tmux_management_scenario(
    artifacts: &ScenarioArtifacts,
    assertions: &mut Vec<AssertionRecord>,
) -> Result<(), String> {
    verify_tooling_prereqs()?;
    let binary = built_buddy_binary()?;
    let server = MockModelServer::start_with_responses(vec![
        scripted_tool_call_response(
            "call_ui_mgmt_1",
            "tmux-create-pane",
            json!({
                "pane": "worker",
                "risk": "low",
                "mutation": true,
                "privesc": false,
                "why": "Create a dedicated managed pane for this UI regression run."
            }),
        ),
        scripted_tool_call_response(
            "call_ui_mgmt_2",
            "run_shell",
            json!({
                "command": "printf 'UI_TMUX_MGMT_OK\\n'",
                "pane": "worker",
                "risk": "low",
                "mutation": false,
                "privesc": false,
                "why": "Validate targeted run_shell dispatch to a managed non-default pane."
            }),
        ),
        scripted_final_response("Tmux management complete."),
    ])?;
    write_ui_regression_config(artifacts, &server.base_url_v1())?;

    let session_name = format!("buddy-ui-mgmt-test-{}", std::process::id());
    let mut tmux = TmuxHarness::start(&session_name)?;
    tmux.enable_pipe_logging(&artifacts.pipe_log_path)?;

    let launch = tmux.launch_buddy_command(&binary, artifacts, "ui-test");
    tmux.send_line(&launch)?;

    let startup_plain = checkpoint_contains(
        &tmux,
        artifacts,
        "mgmt-startup-ready",
        "used)>",
        Duration::from_secs(45),
        false,
    )?;
    artifacts.save_snapshot("mgmt-startup-plain", &startup_plain)?;
    record_contains_assertion(
        assertions,
        "mgmt startup banner",
        &startup_plain,
        "buddy running on localhost with model ui-test-model",
    )?;

    tmux.send_line("create a worker pane and run a command there")?;

    let approval_create = checkpoint_contains(
        &tmux,
        artifacts,
        "mgmt-approval-create-pane",
        "tmux create-pane",
        Duration::from_secs(45),
        false,
    )?;
    artifacts.save_snapshot("mgmt-approval-create-pane", &approval_create)?;
    record_contains_assertion(
        assertions,
        "create-pane approval is rendered",
        &approval_create,
        "tmux create-pane",
    )?;
    tmux.send_line("y")?;

    let approval_shell = checkpoint_contains(
        &tmux,
        artifacts,
        "mgmt-approval-shell",
        "$ printf 'UI_TMUX_MGMT_OK",
        Duration::from_secs(45),
        false,
    )?;
    artifacts.save_snapshot("mgmt-approval-shell", &approval_shell)?;
    record_contains_assertion(
        assertions,
        "targeted shell approval is rendered",
        &approval_shell,
        "UI_TMUX_MGMT_OK",
    )?;
    tmux.send_line("y")?;

    let completion = checkpoint_contains(
        &tmux,
        artifacts,
        "mgmt-completion-ready",
        "Tmux management complete.",
        Duration::from_secs(60),
        false,
    )?;
    artifacts.save_snapshot("mgmt-completion", &completion)?;
    record_contains_assertion(
        assertions,
        "targeted shell output present",
        &completion,
        "UI_TMUX_MGMT_OK",
    )?;

    tmux.send_line("/quit")?;
    let _command =
        tmux.wait_until_command_exits(&["asciinema", "buddy"], Duration::from_secs(30))?;
    tmux.disable_pipe_logging();

    let request_count = server.request_count();
    let request_expect = json!({ "expected_requests": 3, "actual_requests": request_count });
    artifacts.save_snapshot("mgmt-mock-server-requests", &request_expect.to_string())?;
    if request_count != 3 {
        return Err(format!(
            "mock server request count mismatch: expected 3, got {request_count}"
        ));
    }

    Ok(())
}

fn scripted_tool_call_response(
    call_id: &str,
    tool_name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    json!({
        "id": format!("chatcmpl-{call_id}"),
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": args.to_string()
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 18,
            "completion_tokens": 22,
            "total_tokens": 40
        }
    })
}

fn scripted_final_response(text: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-ui-final",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": text,
                "tool_calls": null
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 8,
            "total_tokens": 28
        }
    })
}

fn checkpoint_contains(
    tmux: &TmuxHarness,
    artifacts: &ScenarioArtifacts,
    label: &str,
    needle: &str,
    timeout: Duration,
    with_escapes: bool,
) -> Result<String, String> {
    match tmux.wait_for_contains(needle, timeout, with_escapes) {
        Ok(snapshot) => Ok(snapshot),
        Err(err) => {
            let latest = tmux
                .capture_pane(with_escapes)
                .unwrap_or_else(|capture_err| format!("<failed to capture pane: {capture_err}>"));
            let _ = artifacts.save_snapshot(&format!("timeout-{label}"), &latest);
            Err(format!(
                "{err} (saved timeout snapshot: timeout-{label}.txt)"
            ))
        }
    }
}
