//! On-demand tmux traceui regression integration tests.
//!
//! These tests validate the real terminal behavior of `buddy traceui` through
//! tmux + asciinema so pane composition and raw-key navigation regressions are
//! caught outside the unit-test renderer path.

mod ui_tmux;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use ui_tmux::{
    assertion_report_json, built_buddy_binary, record_contains_assertion, verify_tooling_prereqs,
    AssertionRecord, ScenarioArtifacts, TmuxHarness,
};

#[test]
#[ignore = "on-demand tmux/asciinema traceui regression suite"]
fn ui_tmux_traceui_keeps_right_pane_visible_with_long_left_rows() {
    run_traceui_case("traceui-visible-right-pane", traceui_visibility_scenario);
}

#[test]
#[ignore = "on-demand tmux/asciinema traceui regression suite"]
fn ui_tmux_traceui_detail_scroll_reveals_lower_content() {
    run_traceui_case("traceui-detail-scroll", traceui_detail_scroll_scenario);
}

#[test]
#[ignore = "on-demand tmux/asciinema traceui regression suite"]
fn ui_tmux_traceui_stream_pause_and_resume_behavior() {
    run_traceui_case(
        "traceui-stream-pause-resume",
        traceui_stream_pause_resume_scenario,
    );
}

fn run_traceui_case(
    scenario: &str,
    runner: fn(&ScenarioArtifacts, &mut Vec<AssertionRecord>) -> Result<(), String>,
) {
    let artifacts = ScenarioArtifacts::create(scenario).expect("artifacts");
    let mut assertions = Vec::<AssertionRecord>::new();
    let mut error_text = None;

    if let Err(err) = runner(&artifacts, &mut assertions) {
        error_text = Some(err);
    }

    let report = assertion_report_json(scenario, error_text.is_none(), &assertions, &artifacts);
    artifacts.write_report(&report).expect("write report");

    if let Some(err) = error_text {
        panic!(
            "traceui regression scenario failed: {err}\nartifacts: {}\nreport: {}",
            artifacts.root.display(),
            artifacts.report_path.display()
        );
    }
}

fn traceui_visibility_scenario(
    artifacts: &ScenarioArtifacts,
    assertions: &mut Vec<AssertionRecord>,
) -> Result<(), String> {
    verify_tooling_prereqs()?;
    let binary = built_buddy_binary()?;
    let trace_path = artifacts.work_dir.join("visibility.trace.jsonl");
    write_trace_file(
        &trace_path,
        &[json!({
            "seq": 1,
            "ts_unix_ms": 1,
            "event": {
                "type": "Tool",
                "payload": {
                    "result": {
                        "name": "tmux_capture_pane",
                        "result": "LONG_LEFT_SUMMARY ".repeat(40),
                        "task": { "task_id": 1 }
                    }
                }
            }
        })],
    )?;

    let session_name = format!("buddy-traceui-visible-{}", std::process::id());
    let mut tmux = TmuxHarness::start_sized(&session_name, 120, 34)?;
    tmux.enable_pipe_logging(&artifacts.pipe_log_path)?;
    launch_traceui(&tmux, &binary, artifacts, &trace_path, false)?;

    let ready = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-visible-ready",
        "buddy traceui",
        Duration::from_secs(30),
        false,
    )?;
    artifacts.save_snapshot("traceui-visible-ready", &ready)?;
    record_contains_assertion(assertions, "viewer header present", &ready, "buddy traceui")?;
    record_contains_assertion(assertions, "split divider present", &ready, "│ Tool/result")?;
    record_contains_assertion(
        assertions,
        "right pane payload visible",
        &ready,
        "name: \"tmux_capture_pane\"",
    )?;

    shutdown_traceui(&tmux)?;
    Ok(())
}

fn traceui_detail_scroll_scenario(
    artifacts: &ScenarioArtifacts,
    assertions: &mut Vec<AssertionRecord>,
) -> Result<(), String> {
    verify_tooling_prereqs()?;
    let binary = built_buddy_binary()?;
    let trace_path = artifacts.work_dir.join("scroll.trace.jsonl");
    write_trace_file(&trace_path, &[scroll_trace_event(1, 48, "top", "bottom")])?;

    let session_name = format!("buddy-traceui-scroll-{}", std::process::id());
    let mut tmux = TmuxHarness::start_sized(&session_name, 120, 28)?;
    tmux.enable_pipe_logging(&artifacts.pipe_log_path)?;
    launch_traceui(&tmux, &binary, artifacts, &trace_path, false)?;

    let initial = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-scroll-initial",
        "marker_top_01",
        Duration::from_secs(30),
        false,
    )?;
    artifacts.save_snapshot("traceui-scroll-initial", &initial)?;
    record_contains_assertion(
        assertions,
        "initial top marker visible",
        &initial,
        "marker_top_01",
    )?;

    tmux.send_keys(&["d", "d", "d"])?;
    let scrolled = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-scroll-after-d",
        "marker_bottom_48",
        Duration::from_secs(10),
        false,
    )?;
    artifacts.save_snapshot("traceui-scroll-after-d", &scrolled)?;
    record_contains_assertion(
        assertions,
        "detail scroll reaches lower content",
        &scrolled,
        "marker_bottom_48",
    )?;

    shutdown_traceui(&tmux)?;
    Ok(())
}

fn traceui_stream_pause_resume_scenario(
    artifacts: &ScenarioArtifacts,
    assertions: &mut Vec<AssertionRecord>,
) -> Result<(), String> {
    verify_tooling_prereqs()?;
    let binary = built_buddy_binary()?;
    let trace_path = artifacts.work_dir.join("stream.trace.jsonl");
    write_trace_file(&trace_path, &[marker_trace_event(1, "FIRST")])?;

    let session_name = format!("buddy-traceui-stream-{}", std::process::id());
    let mut tmux = TmuxHarness::start_sized(&session_name, 120, 30)?;
    tmux.enable_pipe_logging(&artifacts.pipe_log_path)?;
    launch_traceui(&tmux, &binary, artifacts, &trace_path, true)?;

    let first = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-stream-first",
        "marker: \"FIRST\"",
        Duration::from_secs(30),
        false,
    )?;
    artifacts.save_snapshot("traceui-stream-first", &first)?;

    append_trace_event(&trace_path, &marker_trace_event(2, "SECOND"))?;
    let second = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-stream-second",
        "marker: \"SECOND\"",
        Duration::from_secs(10),
        false,
    )?;
    artifacts.save_snapshot("traceui-stream-second", &second)?;
    record_contains_assertion(
        assertions,
        "follow mode tracks appended event",
        &second,
        "marker: \"SECOND\"",
    )?;

    tmux.send_keys(&["k"])?;
    let inspect = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-stream-inspect",
        "marker: \"FIRST\"",
        Duration::from_secs(10),
        false,
    )?;
    artifacts.save_snapshot("traceui-stream-inspect", &inspect)?;
    record_contains_assertion(
        assertions,
        "navigating selects prior event",
        &inspect,
        "marker: \"FIRST\"",
    )?;

    append_trace_event(&trace_path, &marker_trace_event(3, "THIRD"))?;
    let paused = wait_for_snapshot(&tmux, Duration::from_secs(10), false, |snapshot| {
        snapshot.contains("[1 new while paused]")
            && snapshot.contains("marker: \"FIRST\"")
            && !snapshot.contains("marker: \"THIRD\"")
    })?;
    artifacts.save_snapshot("traceui-stream-paused", &paused)?;
    record_contains_assertion(
        assertions,
        "paused indicator visible",
        &paused,
        "[1 new while paused]",
    )?;

    tmux.send_keys(&["Escape"])?;
    let resumed = checkpoint_contains(
        &tmux,
        artifacts,
        "traceui-stream-resumed",
        "marker: \"THIRD\"",
        Duration::from_secs(10),
        false,
    )?;
    artifacts.save_snapshot("traceui-stream-resumed", &resumed)?;
    record_contains_assertion(
        assertions,
        "escape resumes follow mode",
        &resumed,
        "marker: \"THIRD\"",
    )?;

    shutdown_traceui(&tmux)?;
    Ok(())
}

fn launch_traceui(
    tmux: &TmuxHarness,
    binary: &Path,
    artifacts: &ScenarioArtifacts,
    trace_path: &Path,
    stream: bool,
) -> Result<(), String> {
    let binary = canonicalize_path(binary)?;
    let work_dir = canonicalize_path(&artifacts.work_dir)?;
    let home_dir = canonicalize_path(&artifacts.home_dir)?;
    let trace_path = canonicalize_path(trace_path)?;
    let mut command = format!(
        "cd {} && HOME={} {} traceui {}",
        shell_quote(&work_dir.display().to_string()),
        shell_quote(&home_dir.display().to_string()),
        shell_quote(&binary.display().to_string()),
        shell_quote(&trace_path.display().to_string())
    );
    if stream {
        command.push_str(" --stream");
    }
    let launch = tmux.launch_recorded_command(&command, &artifacts.cast_path);
    tmux.send_line(&launch)
}

fn shutdown_traceui(tmux: &TmuxHarness) -> Result<(), String> {
    tmux.send_keys(&["q"])?;
    let _ = tmux.wait_until_command_exits(&["asciinema", "buddy"], Duration::from_secs(20))?;
    Ok(())
}

fn marker_trace_event(seq: u64, marker: &str) -> Value {
    json!({
        "seq": seq,
        "ts_unix_ms": seq,
        "event": {
            "type": "TraceUi",
            "payload": {
                "payload": {
                    "marker": marker,
                    "task": { "task_id": 7 }
                }
            }
        }
    })
}

fn scroll_trace_event(seq: u64, lines: usize, top_prefix: &str, bottom_prefix: &str) -> Value {
    let mut map = serde_json::Map::new();
    for idx in 1..=lines {
        let label = format!("line_{idx:02}");
        let value = if idx <= lines / 2 {
            format!("marker_{top_prefix}_{idx:02}")
        } else {
            format!("marker_{bottom_prefix}_{idx:02}")
        };
        map.insert(label, Value::String(value));
    }
    json!({
        "seq": seq,
        "ts_unix_ms": seq,
        "event": {
            "type": "TraceUi",
            "payload": {
                "payload": Value::Object(map)
            }
        }
    })
}

fn write_trace_file(path: &Path, events: &[Value]) -> Result<(), String> {
    let mut rendered = String::new();
    for event in events {
        rendered.push_str(
            &serde_json::to_string(event).map_err(|err| format!("serialize trace event: {err}"))?,
        );
        rendered.push('\n');
    }
    fs::write(path, rendered).map_err(|err| format!("write trace file {}: {err}", path.display()))
}

fn append_trace_event(path: &Path, event: &Value) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|err| format!("open trace file for append {}: {err}", path.display()))?;
    use std::io::Write as _;
    writeln!(
        file,
        "{}",
        serde_json::to_string(event).map_err(|err| format!("serialize trace event: {err}"))?
    )
    .map_err(|err| format!("append trace event {}: {err}", path.display()))
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

fn wait_for_snapshot(
    tmux: &TmuxHarness,
    timeout: Duration,
    with_escapes: bool,
    predicate: impl Fn(&str) -> bool,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    let mut latest = String::new();
    while Instant::now() < deadline {
        latest = tmux.capture_pane(with_escapes)?;
        if predicate(&latest) {
            return Ok(latest);
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "timed out waiting for traceui snapshot condition after {timeout:?}; latest length={}",
        latest.len()
    ))
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|err| format!("canonicalize {}: {err}", path.display()))
}

fn shell_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".to_string();
    }
    let escaped = text.replace('"' as char, "\\\"");
    format!("\"{escaped}\"")
}
