//! Retry policy utilities for API requests.

use crate::error::ApiError;
use std::time::Duration;

/// Bounded retry policy used by `ApiClient`.
#[derive(Clone, Copy, Debug)]
pub(super) struct RetryPolicy {
    /// Upper bound on total attempts, including the initial request.
    pub(super) max_attempts: u32,
    /// Base delay used for exponential backoff.
    pub(super) initial_backoff: Duration,
    /// Maximum allowed delay between retry attempts.
    pub(super) max_backoff: Duration,
}

impl Default for RetryPolicy {
    /// Conservative retry defaults that recover transient errors quickly.
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(8),
        }
    }
}

impl RetryPolicy {
    /// Decide whether another retry attempt should be scheduled.
    pub(super) fn should_retry(&self, err: &ApiError, attempt: u32) -> bool {
        if attempt.saturating_add(1) >= self.max_attempts {
            return false;
        }
        match err {
            ApiError::Http(inner) => inner.is_timeout() || inner.is_connect(),
            ApiError::Status { code, .. } => *code == 429 || (*code >= 500 && *code <= 599),
            ApiError::LoginRequired(_) | ApiError::InvalidResponse(_) => false,
        }
    }

    /// Compute retry delay, respecting `Retry-After` when present.
    pub(super) fn retry_delay_for(&self, attempt: u32, err: &ApiError) -> Duration {
        if let Some(seconds) = err.retry_after_secs() {
            return Duration::from_secs(seconds.clamp(1, 300));
        }
        let pow = 2u32.saturating_pow(attempt);
        let millis = self
            .initial_backoff
            .as_millis()
            .saturating_mul(pow as u128)
            .min(self.max_backoff.as_millis());
        Duration::from_millis(millis as u64)
    }
}
