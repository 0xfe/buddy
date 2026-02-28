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
        let mut idx = 0usize;

        while !stop_flag.load(Ordering::Relaxed) {
            let line = progress_line(
                settings::PROGRESS_FRAMES[idx % settings::PROGRESS_FRAMES.len()],
                &label,
                start.elapsed(),
                color,
                &metrics,
            );
            let mut err = io::stderr();
            let _ = write!(err, "{line}");
            let _ = err.flush();
            idx += 1;
            thread::sleep(Duration::from_millis(settings::PROGRESS_TICK_MS));
        }

        clear_progress_line();
    });

    ProgressHandle {
        stop,
        thread: Some(thread),
    }
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
            format!("[{frame}]").with(settings::COLOR_PROGRESS_FRAME),
            label.with(settings::COLOR_PROGRESS_LABEL),
            format!("({elapsed_s:.1}s)").with(settings::COLOR_PROGRESS_ELAPSED),
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
}
