//! Internal retry helpers shared by the `web_fetch` and `web_search` tools.
//!
//! Transient provider failures (connection problems, timeouts, 429/5xx, unreadable or
//! unparsable responses) are retried inside the tool with bounded exponential backoff, so the
//! model only sees the final outcome of the attempt sequence.

use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentError, AgentResult};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::StatusCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

/// Total provider attempts per tool call: the initial attempt plus up to two retries.
pub(super) const MAX_ATTEMPTS: usize = 3;

/// Connection-establishment timeout per attempt so transport stalls fail fast enough to retry.
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Stop retrying unless at least this much of the call budget still remains.
pub(super) const MIN_RETRY_REMAINING: Duration = Duration::from_secs(5);

/// Base delay before the first retry; each further retry doubles it, plus bounded jitter.
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(500);
const RETRY_BACKOFF_JITTER: Duration = Duration::from_millis(250);

/// A `Retry-After` longer than this is treated as "do not retry" instead of blocking the tool.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);

/// Diagnostic target for every internal retry event. The Host tracing policy surfaces this one
/// target in development builds, so retry activity stays observable without opening any broader
/// target tree.
const TRACE_TARGET: &str = "mycopilot_core::tools::web_retry";

/// A retryable failure was recorded and another attempt is still possible.
pub(super) fn trace_retry_planned(tool: &str, failed_attempt: usize, reason: &str) {
    debug!(
        target: TRACE_TARGET,
        tool,
        failed_attempt,
        max_attempts = MAX_ATTEMPTS,
        reason,
        "web tool retry: retryable failure recorded"
    );
}

/// The loop is about to wait before the next attempt.
pub(super) fn trace_retry_wait(tool: &str, next_attempt: usize, wait: Duration) {
    debug!(
        target: TRACE_TARGET,
        tool,
        next_attempt,
        delay_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
        "web tool retry: waiting before the next attempt"
    );
}

/// The loop will not retry: `cause` names the decision and `detail` carries the last failure
/// text or the budget explanation.
pub(super) fn trace_retry_skipped(tool: &str, attempt: usize, cause: &str, detail: &str) {
    debug!(
        target: TRACE_TARGET,
        tool,
        attempt,
        cause,
        detail,
        "web tool retry: retry abandoned"
    );
}

/// Attempts are used up; the tool is about to return the last outcome.
pub(super) fn trace_retries_exhausted(tool: &str, attempts: usize, reason: &str) {
    debug!(
        target: TRACE_TARGET,
        tool,
        attempts,
        reason,
        "web tool retry: attempts exhausted"
    );
}

/// A later attempt succeeded, healing the earlier retryable failure.
pub(super) fn trace_retry_recovered(tool: &str, attempts_used: usize) {
    debug!(
        target: TRACE_TARGET,
        tool,
        attempts_used,
        "web tool retry: later attempt succeeded"
    );
}

/// A failed provider attempt together with its retry classification.
pub(super) struct AttemptFailure {
    pub(super) error: AgentError,
    pub(super) retryable: bool,
    pub(super) retry_after: Option<Duration>,
}

impl AttemptFailure {
    /// A failure worth another attempt: transport errors, unreadable bodies, 408/429/5xx.
    pub(super) fn retryable(error: AgentError) -> Self {
        Self {
            error,
            retryable: true,
            retry_after: None,
        }
    }

    /// A failure that repeating the same request will not mend (other 4xx, cancellation).
    pub(super) fn terminal(error: AgentError) -> Self {
        Self {
            error,
            retryable: false,
            retry_after: None,
        }
    }

    pub(super) fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }
}

/// Statuses worth another attempt: request timeout, rate limiting, and server-side errors.
pub(super) fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// `Retry-After` in delta-seconds form; HTTP-date forms fall back to plain backoff.
pub(super) fn retry_after_delay(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    value.parse::<u64>().ok().map(Duration::from_secs)
}

/// Delay before the given 1-based retry: exponential backoff with jitter, raised to the
/// provider's `Retry-After` when one was supplied. `None` means the provider asked to wait
/// longer than this tool is willing to block, so the caller should stop retrying.
pub(super) fn retry_delay(retry_number: u32, retry_after: Option<Duration>) -> Option<Duration> {
    let base = RETRY_BACKOFF_BASE.saturating_mul(1u32 << retry_number.saturating_sub(1).min(6));
    let backoff = base.saturating_add(jitter());
    match retry_after {
        Some(wait) if wait <= MAX_RETRY_WAIT => Some(backoff.max(wait)),
        Some(_) => None,
        None => Some(backoff),
    }
}

/// Cancellation-aware wait between attempts.
pub(super) async fn wait_before_retry(
    wait: Duration,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<()> {
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        _ = tokio::time::sleep(wait) => Ok(()),
    }
}

/// Maps a clock's sub-second microseconds to the jitter, in milliseconds.
///
/// Microseconds, not nanoseconds: on microsecond-resolution clocks (e.g. macOS) the sub-second
/// nanosecond value is always a multiple of 1000, so the modulo against a divisor that divides
/// 1000 (the 250 ms `RETRY_BACKOFF_JITTER`) silently collapsed to constant zero. Microsecond
/// input keeps the spread meaningful on those clocks.
fn jitter_millis_from_micros(micros: u32) -> u64 {
    u64::from(micros) % RETRY_BACKOFF_JITTER.as_millis().max(1) as u64
}

fn jitter() -> Duration {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_micros())
        .unwrap_or_default();
    Duration::from_millis(jitter_millis_from_micros(micros))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_grows_exponentially_with_bounded_jitter() {
        let first = retry_delay(1, None).expect("first retry waits");
        let second = retry_delay(2, None).expect("second retry waits");

        assert!(first >= Duration::from_millis(500));
        assert!(first <= Duration::from_millis(750));
        assert!(second >= Duration::from_millis(1000));
        assert!(second <= Duration::from_millis(1250));
    }

    #[test]
    fn retry_delay_honors_retry_after_within_the_wait_budget() {
        let wait =
            retry_delay(1, Some(Duration::from_secs(2))).expect("two seconds is within budget");
        assert!(wait >= Duration::from_secs(2));

        let wait = retry_delay(2, Some(Duration::from_millis(10))).expect("keeps plain backoff");
        assert!(wait >= Duration::from_secs(1));
    }

    #[test]
    fn retry_delay_refuses_waits_longer_than_the_budget() {
        assert!(retry_delay(1, Some(Duration::from_secs(11))).is_none());
    }

    #[test]
    fn parses_retry_after_delta_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(retry_after_delay(&headers), Some(Duration::from_secs(3)));

        headers.insert(RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(retry_after_delay(&headers), None);

        assert_eq!(retry_after_delay(&HeaderMap::new()), None);
    }

    #[test]
    fn classifies_retryable_statuses() {
        assert!(is_retryable_status(StatusCode::REQUEST_TIMEOUT));
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn jitter_is_not_inert_on_microsecond_resolution_clocks() {
        // Regression: on microsecond-resolution clocks (e.g. macOS) the sub-second nanosecond
        // value is always a multiple of 1000, so the old `nanos % 250` was constantly zero and
        // the jitter never applied. 288_999 microseconds is such a clock reading: as
        // microseconds it must stay varying (249 ms of the current 250 ms bound), not zero.
        assert_eq!(jitter_millis_from_micros(288_999), 249);
    }

    #[test]
    fn jitter_millis_stays_within_the_jitter_bound() {
        let bound = RETRY_BACKOFF_JITTER.as_millis().max(1) as u64;
        assert_eq!(jitter_millis_from_micros(0), 0);
        for micros in [1_u32, 125_001, 288_999, 500_000, 999_999] {
            assert!(
                jitter_millis_from_micros(micros) < bound,
                "micros = {micros}"
            );
        }
    }
}
