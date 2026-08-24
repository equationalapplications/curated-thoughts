//! Exponential backoff with full jitter for reconnect scheduling (§4: "1s → 30s cap").
//! `jitter` is injected (rather than sampled internally) so callers can pass `rand::random()`
//! in production and a fixed value in tests for determinism.

use std::time::Duration;

/// `attempt` is 0-based. Returns a duration in `[1ms, min(cap, base * 2^attempt)]`,
/// scaled by `jitter` (expected in `[0.0, 1.0)`; out-of-range values are clamped).
pub fn next_delay(attempt: u32, base: Duration, cap: Duration, jitter: f64) -> Duration {
    let jitter = jitter.clamp(0.0, 1.0);
    let exp_millis = base.as_millis().saturating_mul(1u128 << attempt.min(20));
    let capped_millis = exp_millis.min(cap.as_millis());
    let millis = (capped_millis as f64 * jitter).round() as u64;
    Duration::from_millis(millis.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_secs(1);
    const CAP: Duration = Duration::from_secs(30);

    #[test]
    fn zero_jitter_returns_minimum_one_millisecond() {
        assert_eq!(next_delay(0, BASE, CAP, 0.0), Duration::from_millis(1));
    }

    #[test]
    fn full_jitter_at_attempt_zero_is_base() {
        assert_eq!(next_delay(0, BASE, CAP, 1.0), Duration::from_millis(1000));
    }

    #[test]
    fn full_jitter_doubles_each_attempt_until_cap() {
        assert_eq!(next_delay(1, BASE, CAP, 1.0), Duration::from_millis(2000));
        assert_eq!(next_delay(2, BASE, CAP, 1.0), Duration::from_millis(4000));
        assert_eq!(next_delay(3, BASE, CAP, 1.0), Duration::from_millis(8000));
    }

    #[test]
    fn full_jitter_is_capped_at_thirty_seconds() {
        assert_eq!(
            next_delay(10, BASE, CAP, 1.0),
            Duration::from_millis(30_000)
        );
        assert_eq!(
            next_delay(30, BASE, CAP, 1.0),
            Duration::from_millis(30_000)
        );
    }

    #[test]
    fn jitter_out_of_range_is_clamped() {
        assert_eq!(next_delay(0, BASE, CAP, 5.0), Duration::from_millis(1000));
        assert_eq!(next_delay(0, BASE, CAP, -1.0), Duration::from_millis(1));
    }

    #[test]
    fn half_jitter_scales_linearly() {
        assert_eq!(next_delay(1, BASE, CAP, 0.5), Duration::from_millis(1000));
    }
}
