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

use sha2::{Digest, Sha256};

#[cfg(feature = "http")]
use axum::http::HeaderMap;

/// What the map actually stores per bucket: a SHA-256 of the caller's key.
/// Fixed 32 bytes, inline, no heap allocation — see [`RateLimiter`].
type KeyDigest = [u8; 32];

#[derive(Default)]
struct Entry {
    attempts: Vec<Instant>,
    locked_until: Option<Instant>,
}

/// Sliding-window limiter with lockout.
///
/// # Why the key is hashed rather than stored
///
/// The map is keyed by the SHA-256 of the caller's key, not by the key itself,
/// and an entry is never removed. Storing the raw string would make the
/// per-request memory cost equal to the length of an **attacker-supplied**
/// value: `egret-machine-idp` keys on `ip:client_id`, and `client_id` comes
/// straight off the wire of a public, unauthenticated `POST /oauth/token`. A
/// 50 KB `client_id` is accepted (it simply fails to match any client and
/// returns 401) and, keyed raw, would install a 50 KB entry that nothing ever
/// evicts. With the ingress body cap at 1 MB that is up to ~1 MB of permanent
/// process memory per request, and the limiter cannot bound it: every distinct
/// `client_id` is a fresh bucket, so flooding does not even trip the limit it
/// is supposed to trip. An unbounded key is therefore not acceptable on any
/// endpoint reachable from the internet.
///
/// Hashing bounds the cost at 32 bytes per bucket regardless of input length,
/// while keeping every distinct key in its own bucket — the property a length
/// cap would destroy, since truncation collapses long keys that share a prefix
/// into one bucket and would then rate-limit unrelated callers together. The
/// digest must be collision-resistant for the same reason, so it is SHA-256 and
/// not a fast non-cryptographic hash.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<KeyDigest, Entry>>>,
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

    fn digest(key: &str) -> KeyDigest {
        let mut h = Sha256::new();
        h.update(key.as_bytes());
        h.finalize().into()
    }

    /// Record an attempt at `now`. Returns true if allowed, false if rate-limited.
    pub fn check_at(&self, key: &str, now: Instant) -> bool {
        let mut map = self.inner.lock().unwrap();
        let e = map.entry(Self::digest(key)).or_default();
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

    /// Total bytes the map spends on keys. Test-only probe for the bound
    /// documented on [`RateLimiter`]; it grows with the caller's input if the
    /// keys are ever stored raw again.
    #[cfg(test)]
    fn stored_key_bytes(&self) -> usize {
        self.inner.lock().unwrap().keys().map(|k| k.len()).sum()
    }
}

/// The caller's IP as the edge reports it — the first hop in `X-Forwarded-For`.
/// Shared by local login (`egret-auth`) and the machine-IdP token endpoint so
/// both limiters agree on what "one caller" means behind the same proxy. Empty
/// when the header is absent, which collapses unlabelled callers into one
/// bucket per subject.
///
/// # How far this can be trusted differs by deployment
///
/// - **Behind the shipped Helm ingress it is not spoofable.** ingress-nginx
///   runs with `use-forwarded-headers=false` and emits
///   `proxy_set_header X-Forwarded-For $remote_addr`, which *overwrites*
///   whatever the client sent, so the first hop is the real peer address.
/// - **On the SPA-fronted path it is trivially spoofable.**
///   `frontend/nginx-prod.conf` proxies with `$proxy_add_x_forwarded_for`,
///   which *appends* to the client's header, so a client-supplied
///   `X-Forwarded-For` becomes the first hop. Measured through the frontend
///   pod: 60 requests with a fixed `X-Forwarded-For` reach a 429, then one
///   request with a different `X-Forwarded-For` and the same subject is
///   allowed again — a full fresh budget on demand.
///
/// So on the SPA-fronted path the `ip:` component is a partition, not a
/// security boundary: the limiter still bounds a naive or accidental flood and
/// still keeps callers from locking each other out, but it does not bound a
/// deliberate one. Widening that is an edge-configuration change (make the SPA
/// server overwrite the header the way the ingress does), not a limiter change.
#[cfg(feature = "http")]
pub fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .unwrap_or("")
        .trim()
        .to_string()
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

    /// Long keys sharing a long prefix must stay in SEPARATE buckets. This is
    /// what rules out "just truncate the key": a limiter that stored, say, the
    /// first 256 bytes would put these two in one bucket and reject the second
    /// caller because of the first one's traffic.
    #[test]
    fn long_keys_with_a_shared_prefix_are_still_separate_buckets() {
        let rl = RateLimiter::new(1, 300, 300);
        let t0 = Instant::now();
        let prefix = "c".repeat(100_000);
        let a = format!("{prefix}-a");
        let b = format!("{prefix}-b");

        assert!(rl.check_at(&a, t0), "first caller's one attempt");
        assert!(
            rl.check_at(&b, t0),
            "a different caller sharing a 100 KB prefix must have its own budget"
        );
        assert!(!rl.check_at(&a, t0), "first caller is now out of budget");
        assert!(!rl.check_at(&b, t0), "second caller is now out of budget");
    }

    /// An attacker-supplied key must not buy attacker-chosen permanent memory:
    /// what the map keeps is a fixed-size digest, not the key.
    #[test]
    fn stored_key_footprint_does_not_grow_with_the_callers_key() {
        let rl = RateLimiter::new(5, 300, 300);
        let t0 = Instant::now();

        rl.check_at("1.2.3.4:short", t0);
        let one_bucket = rl.stored_key_bytes();
        assert!(
            one_bucket <= 64,
            "one bucket should cost a fixed digest, got {one_bucket} bytes"
        );

        // A 1 MiB key — the ingress body cap, i.e. the worst case a single
        // public request can hand us.
        rl.check_at(&format!("1.2.3.4:{}", "z".repeat(1024 * 1024)), t0);
        assert_eq!(
            rl.stored_key_bytes(),
            one_bucket * 2,
            "a 1 MiB key must cost exactly what a short one costs"
        );
    }
}
