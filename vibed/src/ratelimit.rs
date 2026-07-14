//! Per-caller token-bucket rate limiting for MCP tool calls.
//!
//! A compromised or runaway agent could hammer vibed with tool calls — flooding
//! the audit log, the memory journal and the approval store, and starving other
//! callers. Each caller uid gets a token bucket: a burst capacity plus a steady
//! refill rate. Over-limit calls are refused (fail-closed) and audited, never
//! executed.
//!
//! The limiter is keyed by the caller uid (from `SO_PEERCRED`, unforgeable) and
//! shared process-wide, NOT per-connection: opening many connections does not
//! multiply an agent's budget. The bucket map holds at most one entry per
//! distinct uid ever seen — a handful on a real machine, and not agent-driven
//! (an agent cannot choose its uid), so it needs no pruning.

use std::collections::HashMap;
use std::sync::Mutex;

/// Default burst capacity (tokens). An interactive agent can issue this many
/// calls back-to-back before the steady rate applies.
pub const DEFAULT_CAPACITY: f64 = 60.0;
/// Default sustained refill (tokens per second). Generous for real work, tight
/// enough that a tight loop is bounded to this rate once the burst is spent.
pub const DEFAULT_REFILL_PER_SEC: f64 = 10.0;

/// Sentinel bucket key for callers whose uid could not be read (peer creds
/// unavailable). They share one bucket — fail-safe, never unbounded.
const UNKNOWN_UID_KEY: u64 = u64::MAX;

struct Bucket {
    tokens: f64,
    last_secs: u64,
}

/// Process-wide, per-uid token-bucket limiter.
pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Mutex<HashMap<u64, Bucket>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_REFILL_PER_SEC)
    }
}

impl RateLimiter {
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        Self {
            capacity,
            refill_per_sec,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Try to consume one token for `uid` at time `now` (epoch seconds). Returns
    /// `true` if the call is allowed, `false` if the caller is over its limit.
    /// `now` is a parameter so the refill logic is deterministically testable.
    pub fn check(&self, uid: Option<u32>, now: u64) -> bool {
        let key = uid.map(u64::from).unwrap_or(UNKNOWN_UID_KEY);
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bucket = buckets.entry(key).or_insert(Bucket {
            tokens: self.capacity,
            last_secs: now,
        });
        // Refill by elapsed wall-clock, capped at capacity. `saturating_sub`
        // guards against a non-monotonic clock (never a negative refill).
        let elapsed = now.saturating_sub(bucket.last_secs) as f64;
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last_secs = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_up_to_capacity_then_refused() {
        let rl = RateLimiter::new(5.0, 1.0);
        // Five calls in the same second: allowed (burst capacity).
        for i in 0..5 {
            assert!(rl.check(Some(1000), 100), "call {i} within burst");
        }
        // Sixth in the same second: no tokens left.
        assert!(!rl.check(Some(1000), 100), "burst exhausted");
    }

    #[test]
    fn tokens_refill_over_time() {
        let rl = RateLimiter::new(5.0, 1.0);
        for _ in 0..5 {
            assert!(rl.check(Some(1000), 100));
        }
        assert!(!rl.check(Some(1000), 100));
        // Two seconds later, two tokens have refilled.
        assert!(rl.check(Some(1000), 102));
        assert!(rl.check(Some(1000), 102));
        assert!(!rl.check(Some(1000), 102), "only two tokens refilled");
    }

    #[test]
    fn refill_is_capped_at_capacity() {
        let rl = RateLimiter::new(5.0, 1.0);
        // Long idle then a burst: never more than `capacity` accumulated.
        for _ in 0..5 {
            assert!(rl.check(Some(1000), 10_000));
        }
        assert!(
            !rl.check(Some(1000), 10_000),
            "cap not exceeded by long idle"
        );
    }

    #[test]
    fn buckets_are_per_uid() {
        let rl = RateLimiter::new(1.0, 0.0);
        assert!(rl.check(Some(1000), 0));
        assert!(!rl.check(Some(1000), 0), "uid 1000 exhausted");
        // A different uid has its own independent bucket.
        assert!(rl.check(Some(1001), 0), "uid 1001 unaffected");
        // Unknown-uid callers share the sentinel bucket, still bounded.
        assert!(rl.check(None, 0));
        assert!(!rl.check(None, 0), "unknown-uid bucket is bounded too");
    }

    #[test]
    fn a_backward_clock_never_refills_tokens() {
        // The `saturating_sub` guard (line: elapsed) means a wall clock that
        // jumps BACKWARD hands out no free tokens — a stepped/adjusted clock
        // cannot be used to reset an exhausted bucket.
        let rl = RateLimiter::new(5.0, 10.0);
        for _ in 0..5 {
            assert!(rl.check(Some(1000), 1000));
        }
        assert!(!rl.check(Some(1000), 1000), "burst spent at t=1000");
        // Clock jumps back 500s: elapsed saturates to 0, so no refill.
        assert!(
            !rl.check(Some(1000), 500),
            "a backward clock must not refill the bucket"
        );
        // And time resuming forward still refills normally (one token at +1s).
        assert!(rl.check(Some(1000), 501), "forward time refills again");
    }
}
