//! Tmux-backed UI regression harness helpers.
//!
//! This module intentionally keeps shell/tmux orchestration in one place so
//! ignored integration tests can focus on expected UI behavior assertions.

use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Convenience result alias for harness operations.
pub type HarnessResult<T> = Result<T, String>;

/// Aggregated filesystem paths for one regression scenario run.
#[derive(Debug, Clone)]
pub struct ScenarioArtifacts {
    /// Root artifact directory for this scenario.
    pub root: PathBuf,
    /// Asciinema cast output path.
    pub cast_path: PathBuf,
    /// `tmux pipe-pane` continuous output log path.
    pub pipe_log_path: PathBuf,
    /// Directory for checkpoint snapshots captured via `tmux capture-pane`.
    pub snapshots_dir: PathBuf,
    /// JSON report path with pass/fail assertion details.
    pub report_path: PathBuf,
    /// Ephemeral config used by the scenario.
    pub config_path: PathBuf,
    /// Isolated HOME used for the buddy process under test.
    pub home_dir: PathBuf,
    /// Isolated working directory used for session/history writes.
    pub work_dir: PathBuf,
}

impl ScenarioArtifacts {
    /// Create a new artifact bundle rooted under `artifacts/ui-regression/`.
    pub fn create(scenario: &str) -> HarnessResult<Self> {
        let stamp = unique_suffix();
        let root = PathBuf::from("artifacts")
            .join("ui-regression")
            .join(format!("{scenario}-{stamp}"));
        let snapshots_dir = root.join("snapshots");
        let home_dir = root.join("home");
        let work_dir = root.join("work");
        fs::create_dir_all(&snapshots_dir)
            .map_err(|e| format!("failed creating snapshots dir: {e}"))?;
        fs::create_dir_all(&home_dir).map_err(|e| format!("failed creating home dir: {e}"))?;
        fs::create_dir_all(&work_dir).map_err(|e| format!("failed creating work dir: {e}"))?;
        Ok(Self {
            cast_path: root.join("session.cast"),
            pipe_log_path: root.join("pipe.log"),
            report_path: root.join("report.json"),
            config_path: root.join("buddy.toml"),
            root,
            snapshots_dir,
            home_dir,
            work_dir,
        })
    }

    /// Save one captured pane snapshot to the scenario artifact directory.
    pub fn save_snapshot(&self, name: &str, content: &str) -> HarnessResult<PathBuf> {
        let path = self.snapshots_dir.join(format!("{name}.txt"));
        fs::write(&path, content).map_err(|e| format!("failed writing snapshot {name}: {e}"))?;
        Ok(path)
    }

    /// Write scenario report JSON to disk.
    pub fn write_report(&self, value: &serde_json::Value) -> HarnessResult<()> {
        let rendered = serde_json::to_string_pretty(value)
            .map_err(|e| format!("failed serializing report json: {e}"))?;
        fs::write(&self.report_path, rendered).map_err(|e| format!("failed writing report: {e}"))
    }
}

/// One recorded assertion in the regression report.
#[derive(Debug, Clone)]
pub struct AssertionRecord {
    /// Human-readable assertion identifier.
    pub label: String,
    /// Required substring expected in captured output.
    pub needle: String,
    /// Whether the assertion was satisfied.
    pub matched: bool,
}

/// Run state for one isolated tmux pane/session.
pub struct TmuxHarness {
    session_name: String,
    pane_target: String,
    pipe_enabled: bool,
}

impl TmuxHarness {
    /// Create a detached tmux session and resolve the primary pane target.
    pub fn start(session_name: &str) -> HarnessResult<Self> {
        run_tmux(["new-session", "-d", "-s", session_name, "-n", "harness"])?;
        let pane_target = run_tmux([
            "display-message",
            "-p",
            "-t",
            &format!("{session_name}:harness.0"),
            "#{pane_id}",
        ])?
        .trim()
        .to_string();
        if pane_target.is_empty() {
            return Err("failed to resolve tmux pane target".to_string());
        }
        Ok(Self {
            session_name: session_name.to_string(),
            pane_target,
            pipe_enabled: false,
        })
    }

    /// Start continuous pane logging through `tmux pipe-pane`.
    pub fn enable_pipe_logging(&mut self, output_path: &Path) -> HarnessResult<()> {
        let absolute_output_path = absolute_path(output_path);
        let cmd = format!(
            "cat > {}",
            shell_quote(&absolute_output_path.display().to_string())
        );
        run_tmux(["pipe-pane", "-o", "-t", &self.pane_target, cmd.as_str()])?;
        self.pipe_enabled = true;
        Ok(())
    }

    /// Disable `pipe-pane` when the scenario finishes.
    pub fn disable_pipe_logging(&mut self) {
        if self.pipe_enabled {
            let _ = run_tmux(["pipe-pane", "-t", &self.pane_target]);
            self.pipe_enabled = false;
        }
    }

    /// Send one literal command line into the harness pane and press Enter.
    pub fn send_line(&self, line: &str) -> HarnessResult<()> {
        run_tmux(["send-keys", "-t", &self.pane_target, "-l", line])?;
        run_tmux(["send-keys", "-t", &self.pane_target, "Enter"])?;
        Ok(())
    }

    /// Capture pane text. When `with_escapes` is true, preserve ANSI escapes.
    pub fn capture_pane(&self, with_escapes: bool) -> HarnessResult<String> {
        if with_escapes {
            run_tmux([
                "capture-pane",
                "-p",
                "-e",
                "-J",
                "-S",
                "-",
                "-E",
                "-",
                "-t",
                &self.pane_target,
            ])
        } else {
            run_tmux([
                "capture-pane",
                "-p",
                "-J",
                "-S",
                "-",
                "-E",
                "-",
                "-t",
                &self.pane_target,
            ])
        }
    }

    /// Poll pane snapshots until a required substring appears or timeout elapses.
    pub fn wait_for_contains(
        &self,
        needle: &str,
        timeout: Duration,
        with_escapes: bool,
    ) -> HarnessResult<String> {
        let deadline = Instant::now() + timeout;
        let mut latest: Option<String> = None;
        while Instant::now() < deadline {
            let snapshot = self.capture_pane(with_escapes)?;
            if snapshot.contains(needle) {
                return Ok(snapshot);
            }
            latest = Some(snapshot);
            thread::sleep(Duration::from_millis(200));
        }
        Err(format!(
            "timed out waiting for pane to contain `{needle}` after {timeout:?}. latest snapshot length={}",
            latest.as_ref().map_or(0, String::len)
        ))
    }

    /// Wait until pane foreground command is no longer one of the blocked names.
    pub fn wait_until_command_exits(
        &self,
        blocked: &[&str],
        timeout: Duration,
    ) -> HarnessResult<String> {
        let deadline = Instant::now() + timeout;
        let blocked_lower = blocked
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>();
        while Instant::now() < deadline {
            let current = run_tmux([
                "display-message",
                "-p",
                "-t",
                &self.pane_target,
                "#{pane_current_command}",
            ])?
            .trim()
            .to_ascii_lowercase();
            if !blocked_lower
                .iter()
                .any(|blocked_name| blocked_name == &current)
            {
                return Ok(current);
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(format!(
            "timed out waiting for pane command to exit blocked set: {blocked:?}"
        ))
    }

    /// Build command that launches buddy inside asciinema in this pane.
    pub fn launch_buddy_command(
        &self,
        binary: &Path,
        artifacts: &ScenarioArtifacts,
        model_profile: &str,
    ) -> String {
        let work_dir = canonicalize_path(&artifacts.work_dir);
        let home_dir = canonicalize_path(&artifacts.home_dir);
        let config_path = canonicalize_path(&artifacts.config_path);
        let cast_path = canonicalize_path(&artifacts.cast_path);
        let binary_path = canonicalize_path(binary);
        let run = format!(
            "cd {} && HOME={} {} --config {} --model {} --tmux {}",
            shell_quote(&work_dir.display().to_string()),
            shell_quote(&home_dir.display().to_string()),
            shell_quote(&binary_path.display().to_string()),
            shell_quote(&config_path.display().to_string()),
            shell_quote(model_profile),
            shell_quote(&self.session_name)
        );
        format!(
            "asciinema record --overwrite -q --command {} {}",
            shell_quote(&run),
            shell_quote(&cast_path.display().to_string())
        )
    }

    /// Kill the detached tmux session used by this scenario.
    pub fn cleanup(&mut self) {
        self.disable_pipe_logging();
        let _ = run_tmux(["kill-session", "-t", &self.session_name]);
    }
}

impl Drop for TmuxHarness {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Deterministic local HTTP server that returns scripted chat-completion payloads.
pub struct MockModelServer {
    address: String,
    shutdown: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockModelServer {
    /// Start a local scripted server bound to `127.0.0.1:*`.
    pub fn start() -> HarnessResult<Self> {
        Self::start_with_responses(default_mock_responses())
    }

    /// Start a local scripted server with custom response sequence.
    pub fn start_with_responses(responses: Vec<serde_json::Value>) -> HarnessResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("failed binding mock server: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed setting nonblocking listener: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("failed getting mock server addr: {e}"))?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let responses = Arc::new(responses);
        let shutdown_flag = Arc::clone(&shutdown);
        let request_count = Arc::clone(&requests);
        let scripted_responses = Arc::clone(&responses);
        let thread = thread::spawn(move || {
            while !shutdown_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let idx = request_count.fetch_add(1, Ordering::Relaxed);
                        let _ = handle_mock_request(&mut stream, idx, &scripted_responses);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        });

        Ok(Self {
            address: format!("http://{addr}"),
            shutdown,
            requests,
            thread: Some(thread),
        })
    }

    /// Base URL suitable for profile `api_base_url` fields (`.../v1` included).
    pub fn base_url_v1(&self) -> String {
        format!("{}/v1", self.address)
    }

    /// Number of handled requests.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }
}

impl Drop for MockModelServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(host) = self.address.strip_prefix("http://") {
            let _ = TcpStream::connect(host).and_then(|s| s.shutdown(Shutdown::Both));
        }
        if let Some(join) = self.thread.take() {
            let _ = join.join();
        }
    }
}

/// Ensure external runtime prerequisites exist before running integration flow.
pub fn verify_tooling_prereqs() -> HarnessResult<()> {
    command_exists("tmux")?;
    command_exists("asciinema")?;
    Ok(())
}

/// Create a deterministic minimal config that points buddy at the mock server.
pub fn write_ui_regression_config(
    artifacts: &ScenarioArtifacts,
    base_url_v1: &str,
) -> HarnessResult<()> {
    let content = format!(
        r#"[agent]
name = "ui-test-agent"
model = "ui-test"
max_iterations = 8
system_prompt = "UI regression test harness profile."

[models.ui-test]
api_base_url = "{base_url_v1}"
api = "completions"
auth = "api-key"
api_key = "ui-test-key"
model = "ui-test-model"

[display]
color = true
show_tokens = true
show_tool_calls = true
persist_history = false

[tools]
shell_enabled = true
files_enabled = false
fetch_enabled = false
search_enabled = false
shell_confirm = true
shell_denylist = []
"#
    );
    fs::write(&artifacts.config_path, content)
        .map_err(|e| format!("failed writing test config: {e}"))
}

/// Evaluate one substring assertion and append a report record.
pub fn record_contains_assertion(
    records: &mut Vec<AssertionRecord>,
    label: &str,
    haystack: &str,
    needle: &str,
) -> HarnessResult<()> {
    let matched = haystack.contains(needle);
    records.push(AssertionRecord {
        label: label.to_string(),
        needle: needle.to_string(),
        matched,
    });
    if matched {
        Ok(())
    } else {
        Err(format!("assertion `{label}` failed: missing `{needle}`"))
    }
}

/// Render assertion records as JSON report payload.
pub fn assertion_report_json(
    scenario: &str,
    passed: bool,
    records: &[AssertionRecord],
    artifacts: &ScenarioArtifacts,
) -> serde_json::Value {
    json!({
        "scenario": scenario,
        "passed": passed,
        "artifacts": {
            "root": artifacts.root.display().to_string(),
            "cast": artifacts.cast_path.display().to_string(),
            "pipe_log": artifacts.pipe_log_path.display().to_string(),
            "snapshots": artifacts.snapshots_dir.display().to_string(),
            "report": artifacts.report_path.display().to_string()
        },
        "assertions": records.iter().map(|record| {
            json!({
                "label": record.label,
                "needle": record.needle,
                "matched": record.matched
            })
        }).collect::<Vec<_>>()
    })
}

/// Resolve the built buddy binary path exposed by Cargo integration tests.
pub fn built_buddy_binary() -> HarnessResult<PathBuf> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_buddy") {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest_dir.join("target").join("debug").join("buddy");
    if candidate.exists() {
        return Ok(candidate);
    }

    let status = Command::new("cargo")
        .arg("build")
        .arg("--bin")
        .arg("buddy")
        .current_dir(&manifest_dir)
        .status()
        .map_err(|e| format!("failed to run fallback `cargo build --bin buddy`: {e}"))?;
    if !status.success() {
        return Err(format!(
            "fallback `cargo build --bin buddy` failed with status {status}"
        ));
    }
    if candidate.exists() {
        Ok(candidate)
    } else {
        Err(format!(
            "could not find built buddy binary at {}",
            candidate.display()
        ))
    }
}

fn command_exists(name: &str) -> HarnessResult<()> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .output()
        .map_err(|e| format!("failed running command lookup for `{name}`: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "required command `{name}` not found in PATH; install it before running UI regression tests"
        ))
    }
}

fn run_tmux<I, S>(args: I) -> HarnessResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let collected = args
        .into_iter()
        .map(|value| value.as_ref().to_string())
        .collect::<Vec<_>>();
    let output = Command::new("tmux")
        .args(&collected)
        .output()
        .map_err(|e| format!("failed to run tmux {:?}: {e}", collected))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "tmux {:?} failed with status {}: {}{}",
            collected, output.status, stderr, stdout
        ))
    }
}

fn handle_mock_request(
    stream: &mut TcpStream,
    request_index: usize,
    responses: &[serde_json::Value],
) -> HarnessResult<()> {
    let _request = read_http_json_body(stream)?;
    let response = responses
        .get(request_index)
        .or_else(|| responses.last())
        .ok_or_else(|| "mock response script is empty".to_string())?;
    let delay = if request_index == 0 { 1400 } else { 800 };
    thread::sleep(Duration::from_millis(delay));
    write_http_json(stream, response)?;
    Ok(())
}

fn read_http_json_body(stream: &mut TcpStream) -> HarnessResult<serde_json::Value> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("failed setting read timeout: {e}"))?;
    let mut buffer = Vec::<u8>::new();
    let mut temp = [0u8; 2048];
    let mut header_end: Option<usize> = None;
    let mut content_length: usize = 0;

    loop {
        let n = stream
            .read(&mut temp)
            .map_err(|e| format!("failed reading request bytes: {e}"))?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..n]);
        if header_end.is_none() {
            if let Some(idx) = find_header_terminator(&buffer) {
                header_end = Some(idx);
                let headers = String::from_utf8_lossy(&buffer[..idx]).to_string();
                content_length = parse_content_length(&headers).unwrap_or(0);
            }
        }
        if let Some(idx) = header_end {
            let body_len = buffer.len().saturating_sub(idx + 4);
            if body_len >= content_length {
                break;
            }
        }
    }

    let idx =
        header_end.ok_or_else(|| "malformed HTTP request (missing header end)".to_string())?;
    let body = &buffer[idx + 4..];
    serde_json::from_slice(body).map_err(|e| format!("failed parsing request json: {e}"))
}

fn find_header_terminator(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

fn write_http_json(stream: &mut TcpStream, body: &serde_json::Value) -> HarnessResult<()> {
    let payload = serde_json::to_string(body)
        .map_err(|e| format!("failed serializing response json: {e}"))?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|e| format!("failed writing response bytes: {e}"))
}

fn mock_tool_call_response() -> serde_json::Value {
    let args = json!({
        "command": "sleep 1; printf 'UI_HARNESS_OK\\n'",
        "risk": "low",
        "mutation": false,
        "privesc": false,
        "why": "Validate deterministic approval and output rendering in the UI tmux regression harness."
    })
    .to_string();

    json!({
        "id": "chatcmpl-ui-1",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_ui_1",
                    "type": "function",
                    "function": {
                        "name": "run_shell",
                        "arguments": args
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 20,
            "total_tokens": 32
        }
    })
}

fn mock_final_response() -> serde_json::Value {
    json!({
        "id": "chatcmpl-ui-2",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Harness complete.",
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

fn default_mock_responses() -> Vec<serde_json::Value> {
    vec![mock_tool_call_response(), mock_final_response()]
}

fn unique_suffix() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}-{now}", std::process::id())
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn canonicalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
