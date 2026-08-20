//! In-memory per-process rate limiter over an opaque string key. Resets on
//! restart.
//!
//! Shared, not login-specific: the caller chooses the key, and that choice is
//! the security-relevant part. Both current callers key on the pair
//! `ip:<subject>` — `ip:email` for local login, `ip:client_id` for the
//! machine-IdP token endpoint — so a flood only ever exhausts the flooder's own
//! bucket. Keying on the subject alone would let anyone lock a legitimate
//! account or client out by spamming an identifier that is not a secret.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Entry {
    attempts: Vec<Instant>,
    locked_until: Option<Instant>,
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    max: u32,
    window: Duration,
    lockout: Duration,
}

impl RateLimiter {
    pub fn new(max: u32, window_secs: u64, lockout_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max,
            window: Duration::from_secs(window_secs),
            lockout: Duration::from_secs(lockout_secs),
        }
    }

    /// Record an attempt at `now`. Returns true if allowed, false if rate-limited.
    pub fn check_at(&self, key: &str, now: Instant) -> bool {
        let mut map = self.inner.lock().unwrap();
        let e = map.entry(key.to_string()).or_default();
        if let Some(until) = e.locked_until {
            if now < until {
                return false;
            }
            e.locked_until = None;
            e.attempts.clear();
        }
        e.attempts.retain(|t| now.duration_since(*t) < self.window);
        if e.attempts.len() as u32 >= self.max {
            e.locked_until = Some(now + self.lockout);
            return false;
        }
        e.attempts.push(now);
        true
    }

    pub fn check(&self, key: &str) -> bool {
        self.check_at(key, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_attempts_then_unlocks() {
        let rl = RateLimiter::new(2, 300, 300);
        let t0 = Instant::now();
        assert!(rl.check_at("k", t0)); // 1
        assert!(rl.check_at("k", t0)); // 2
        assert!(!rl.check_at("k", t0)); // 3 → blocked, locks
                                        // still locked within lockout
        assert!(!rl.check_at("k", t0 + Duration::from_secs(100)));
        // after lockout expires → allowed again
        assert!(rl.check_at("k", t0 + Duration::from_secs(601)));
    }

    #[test]
    fn separate_keys_are_independent() {
        let rl = RateLimiter::new(1, 300, 300);
        let t0 = Instant::now();
        assert!(rl.check_at("a", t0));
        assert!(rl.check_at("b", t0));
        assert!(!rl.check_at("a", t0));
    }
}
