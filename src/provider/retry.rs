//! Bounded retry policy for transient LLM transport failures.
//!
//! Mainstream agent clients (Claude Code, Codex, Pi, OpenCode, Goose) retry a
//! small, fixed number of times when a provider is rate limiting or briefly
//! unavailable, wait with exponential backoff plus jitter, and honor a server
//! `Retry-After` hint. This module keeps that policy pure and deterministic so
//! it can be unit-tested without the network: classification, the
//! `Retry-After` hint, and the backoff computation all take plain values.
//!
//! The policy deliberately never covers a successful HTTP response with
//! invalid content. Re-promoting a parsed error would be a hidden repair and
//! could silently double spend; only transport-level failures are retried.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Cap the exponential growth so `base * 2^attempt` cannot overflow.
const MAX_BACKOFF_SHIFT: u32 = 6;

/// How many attempts and how long to wait between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first. `1` disables retries.
    pub max_attempts: u32,
    /// Delay before the second attempt; doubles each attempt.
    pub base_delay: Duration,
    /// Ceiling for any single computed delay.
    pub max_delay: Duration,
}

impl RetryPolicy {
    pub fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            base_delay,
            max_delay: max_delay.max(base_delay),
        }
    }

    /// Build from millisecond bounds, as exposed by the CLI.
    pub fn from_millis(max_attempts: u32, base_ms: u64, max_ms: u64) -> Self {
        Self::new(
            max_attempts,
            Duration::from_millis(base_ms),
            Duration::from_millis(max_ms),
        )
    }

    pub fn disabled() -> Self {
        Self::new(1, Duration::ZERO, Duration::ZERO)
    }

    /// A fresh attempt is allowed when fewer than `max_attempts` were used.
    pub fn allows_retry(&self, attempt: u32) -> bool {
        attempt + 1 < self.max_attempts
    }
}

/// Status codes that indicate a transient condition worth retrying.
///
/// 408 (timeout), 425 (too early), 429 (rate limit), and the 5xx family are
/// consistent across providers. 529 is Anthropic's "overloaded" code and is
/// harmless to include for other providers.
pub fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Parse an RFC 7231 `Retry-After` value in delta-seconds form.
///
/// HTTP-date values are uncommon for model APIs; returning `None` for them
/// lets the caller fall back to jittered exponential backoff.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let seconds: u64 = value.trim().parse().ok()?;
    Some(Duration::from_secs(seconds))
}

/// Delay before the next attempt.
///
/// With a server hint the delay is the hint, capped by `max_delay`. Otherwise
/// it is full jitter in `[0, backoff_ceiling]`, which spreads concurrent
/// workers (up to `--jobs 32`) instead of retrying in lockstep.
pub fn backoff(
    policy: &RetryPolicy,
    attempt: u32,
    retry_after: Option<Duration>,
    seed: u64,
) -> Duration {
    if let Some(hint) = retry_after {
        return hint.min(policy.max_delay);
    }
    let bound_ms = backoff_ceiling(policy, attempt).as_millis().max(1) as u64;
    let random = splitmix64(seed ^ u64::from(attempt).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    Duration::from_millis(random % (bound_ms + 1))
}

/// Upper bound of the jittered delay for an attempt: `min(base * 2^attempt, max)`.
pub fn backoff_ceiling(policy: &RetryPolicy, attempt: u32) -> Duration {
    policy
        .base_delay
        .saturating_mul(1u32 << attempt.min(MAX_BACKOFF_SHIFT))
        .min(policy.max_delay)
}

/// A per-attempt seed derived from the clock, process, and a counter.
pub fn seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ (u64::from(std::process::id()) << 32) ^ COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// SplitMix64: a small, well-distributed mixer for jitter, no extra dependency.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transient_status_only() {
        for status in [408, 425, 429, 500, 502, 503, 504, 529] {
            assert!(retryable_status(status), "{status}");
        }
        for status in [200, 201, 400, 401, 403, 404, 409, 422, 501] {
            assert!(!retryable_status(status), "{status}");
        }
    }

    #[test]
    fn parses_delta_seconds_and_rejects_dates() {
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("  10 "), Some(Duration::from_secs(10)));
        assert_eq!(parse_retry_after("0"), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
    }

    #[test]
    fn server_hint_is_capped_by_max_delay() {
        let policy = RetryPolicy::from_millis(3, 500, 8_000);
        assert_eq!(
            backoff(&policy, 0, Some(Duration::from_secs(30)), 7),
            Duration::from_secs(8)
        );
        assert_eq!(
            backoff(&policy, 0, Some(Duration::from_secs(2)), 7),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn jitter_stays_within_exponential_ceiling() {
        let policy = RetryPolicy::from_millis(5, 500, 4_000);
        for attempt in 0..6 {
            let ceiling = backoff_ceiling(&policy, attempt);
            for seed in 0..64 {
                let delay = backoff(&policy, attempt, None, seed);
                assert!(delay <= ceiling, "attempt {attempt} seed {seed}: {delay:?}");
            }
        }
    }

    #[test]
    fn backoff_ceiling_grows_then_saturates() {
        let policy = RetryPolicy::from_millis(6, 100, 1_000);
        assert_eq!(backoff_ceiling(&policy, 0), Duration::from_millis(100));
        assert_eq!(backoff_ceiling(&policy, 2), Duration::from_millis(400));
        assert_eq!(backoff_ceiling(&policy, 20), Duration::from_millis(1_000));
    }

    #[test]
    fn retries_are_bounded() {
        let policy = RetryPolicy::from_millis(3, 10, 100);
        assert!(policy.allows_retry(0));
        assert!(policy.allows_retry(1));
        assert!(!policy.allows_retry(2));
        assert!(!RetryPolicy::disabled().allows_retry(0));
    }
}
