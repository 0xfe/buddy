//! Spinner/progress primitives for terminal liveness indicators.

use crate::tui::settings;
use crossterm::style::Stylize;
use std::io::{self, IsTerminal, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

static PROGRESS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Optional key/value metrics displayed next to a spinner label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressMetrics {
    /// Ordered key/value metrics appended to the spinner label.
    entries: Vec<(String, String)>,
}

impl ProgressMetrics {
    /// Append one metric key/value pair.
    pub fn with_entry(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push((key.into(), value.into()));
        self
    }

    /// Render metrics as `[k:v ...]` suffix text.
    fn render_suffix(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let pairs = self
            .entries
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!(" [{pairs}]")
    }
}

/// RAII handle for an active spinner/progress indicator.
pub struct ProgressHandle {
    /// Stop signal shared with the spinner thread.
    stop: Arc<AtomicBool>,
    /// Background writer thread, present only when spinner is active.
    thread: Option<thread::JoinHandle<()>>,
}

impl ProgressHandle {
    /// Construct a no-op handle used when progress output is disabled.
    pub(crate) fn disabled() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            thread: None,
        }
    }

    /// Stop and clean up the spinner thread.
    pub fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ProgressHandle {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Globally enable/disable live progress rendering.
pub fn set_progress_enabled(enabled: bool) {
    PROGRESS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Start a spinner on stderr with an optional metric suffix.
pub fn start_progress(
    label: impl Into<String>,
    color: bool,
    metrics: Option<ProgressMetrics>,
) -> ProgressHandle {
    if !PROGRESS_ENABLED.load(Ordering::Relaxed) {
        return ProgressHandle::disabled();
    }
    if !io::stderr().is_terminal() {
        return ProgressHandle::disabled();
    }

    let label = label.into();
    let metrics = metrics.unwrap_or_default();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let thread = thread::spawn(move || {
        let start = Instant::now();

        while !stop_flag.load(Ordering::Relaxed) {
            let frame = spinner_frame_for_elapsed(start.elapsed());
            let line = progress_line(frame, &label, start.elapsed(), color, &metrics);
            let mut err = io::stderr();
            let _ = write!(err, "{line}");
            let _ = err.flush();
            thread::sleep(Duration::from_millis(settings::PROGRESS_TICK_MS));
        }

        clear_progress_line();
    });

    ProgressHandle {
        stop,
        thread: Some(thread),
    }
}

/// Resolve spinner frame from elapsed time using shared global spinner cadence.
///
/// This is the canonical frame-selection path for all UI spinners so both
/// inline liveness status and threaded progress indicators stay in sync.
pub fn spinner_frame_for_elapsed(elapsed: Duration) -> char {
    let tick_ms = u128::from(settings::PROGRESS_TICK_MS.max(1));
    let steps = elapsed.as_millis() / tick_ms;
    let idx = (steps as usize) % settings::PROGRESS_FRAMES.len();
    settings::PROGRESS_FRAMES[idx]
}

fn progress_line(
    frame: char,
    label: &str,
    elapsed: Duration,
    color: bool,
    metrics: &ProgressMetrics,
) -> String {
    // Keep elapsed formatting stable so tests can assert deterministic text.
    let elapsed_s = elapsed.as_millis() as f64 / 1000.0;
    let suffix = metrics.render_suffix();
    if color {
        format!(
            "{}{} {} {}{}",
            settings::PROGRESS_CLEAR_LINE,
            format!("[{frame}]").white().bold(),
            label.with(settings::color_progress_label()),
            format!("({elapsed_s:.1}s)").with(settings::color_progress_elapsed()),
            suffix,
        )
    } else {
        format!(
            "{}[{frame}] {label} ({elapsed_s:.1}s){suffix}",
            settings::PROGRESS_CLEAR_LINE
        )
    }
}

fn clear_progress_line() {
    let mut err = io::stderr();
    let _ = write!(err, "{}", settings::PROGRESS_CLEAR_LINE);
    let _ = err.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_line_plain_contains_label() {
        // Plain mode should still include frame, label, and elapsed seconds.
        let out = progress_line(
            '|',
            "calling model",
            Duration::from_millis(1500),
            false,
            &ProgressMetrics::default(),
        );
        assert!(out.contains("[|] calling model (1.5s)"));
    }

    #[test]
    fn progress_line_includes_metrics_suffix() {
        // Metric entries should render as a bracketed suffix.
        let metrics = ProgressMetrics::default()
            .with_entry("in", "12kb")
            .with_entry("out", "42kb");
        let out = progress_line('|', "fetch", Duration::from_millis(200), false, &metrics);
        assert!(out.contains("[in:12kb out:42kb]"));
    }

    #[test]
    fn spinner_frame_for_elapsed_uses_global_tick_and_frame_sequence() {
        // Spinner frame selection should advance exactly one frame per global tick.
        assert_eq!(spinner_frame_for_elapsed(Duration::from_millis(0)), '|');
        assert_eq!(
            spinner_frame_for_elapsed(Duration::from_millis(settings::PROGRESS_TICK_MS)),
            '/'
        );
        assert_eq!(
            spinner_frame_for_elapsed(Duration::from_millis(settings::PROGRESS_TICK_MS * 2)),
            '-'
        );
    }
}
