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

impl Entry {
    /// Whether this entry is still tracking something a later check needs.
    ///
    /// **An active lockout keeps the entry alive** even with no attempts left
    /// in the window, and that is the entire reason this is a predicate rather
    /// than `attempts.is_empty()`. The lockout is deliberately the longer of
    /// the two durations; dropping a locked-out entry once its *window* passed
    /// would let a locked-out caller come back by waiting out the shorter of
    /// the two clocks — precisely the reset the lockout exists to deny.
    fn holds_information(&self, now: Instant, window: Duration) -> bool {
        if let Some(until) = self.locked_until {
            if now < until {
                return true;
            }
        }
        self.attempts
            .iter()
            .any(|t| now.duration_since(*t) < window)
    }
}

/// Everything the limiter mutates, behind the one lock: the buckets, plus when
/// the next sweep over them is due. See [`RateLimiter::check_at`].
#[derive(Default)]
struct State {
    buckets: HashMap<KeyDigest, Entry>,
    /// `None` until the first check, which arms it rather than sweeping an
    /// empty map.
    next_sweep_at: Option<Instant>,
}

/// Sliding-window limiter with lockout.
///
/// # Why the key is hashed rather than stored
///
/// The map is keyed by the SHA-256 of the caller's key, not by the key itself.
/// Storing the raw string would make the per-request memory cost equal to the
/// length of an **attacker-supplied** value: `egret-machine-idp` keys on
/// `ip:client_id`, and `client_id` comes straight off the wire of a public,
/// unauthenticated `POST /oauth/token`. A 50 KB `client_id` is accepted (it
/// simply fails to match any client and returns 401) and, keyed raw, would
/// install a 50 KB entry per distinct value. With the ingress body cap at 1 MB
/// that is up to ~1 MB of process memory per request held for a window, and
/// the limiter cannot bound it: every distinct `client_id` is a fresh bucket,
/// so flooding does not even trip the limit it is supposed to trip. An
/// unbounded key is therefore not acceptable on any endpoint reachable from
/// the internet.
///
/// Hashing bounds the cost at 32 bytes per bucket regardless of input length,
/// while keeping every distinct key in its own bucket — the property a length
/// cap would destroy, since truncation collapses long keys that share a prefix
/// into one bucket and would then rate-limit unrelated callers together. The
/// digest must be collision-resistant for the same reason, so it is SHA-256 and
/// not a fast non-cryptographic hash.
///
/// Hashing bounds what one bucket **costs**; the sweep in [`Self::check_at`]
/// bounds **how many** of them there are.
/// What [`RateLimiter::verdict_at`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    /// Over the limit. `first` is true for the one call that TRIPPED the limiter (it
    /// started the lockout) and false for every call refused inside that lockout, so
    /// a caller that records denials can record one per lockout rather than one per
    /// refused call.
    Denied {
        first: bool,
    },
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<State>>,
    max: u32,
    window: Duration,
    lockout: Duration,
}

impl RateLimiter {
    pub fn new(max: u32, window_secs: u64, lockout_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State::default())),
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
    ///
    /// # Why this also sweeps
    ///
    /// An entry with no attempts left in the window and no active lockout is
    /// tracking nothing, and keeping it means the map grows with every distinct
    /// key the process has ever seen. The keys are attacker-chosen —
    /// `egret-machine-idp` keys on `ip:client_id`, straight off a public,
    /// unauthenticated `POST /oauth/token` — so "ever seen" has no ceiling.
    ///
    /// **Cost, and why this shape.** One sweep is `O(n)` over the map with the
    /// lock held, and it runs at most once per `window`, so it is amortised
    /// over every request in that window: a flood of `n` distinct keys pays one
    /// `O(n)` pass, not `n` of them. Sweeping on *every* check was rejected
    /// because it makes the same flood quadratic while holding a lock that is
    /// on the login path; a background task was rejected because it adds a
    /// thread and a second contender for the lock to a type that is otherwise
    /// one `Mutex` around one `HashMap`.
    ///
    /// **What it leaves behind.** Between sweeps the map holds at most the
    /// distinct keys seen since the last sweep plus everything still locked
    /// out — one window of lag on an already-bounded set, not open growth.
    pub fn check_at(&self, key: &str, now: Instant) -> bool {
        self.verdict_at(key, now) == Verdict::Allowed
    }

    /// Drop the entries that track nothing, at most once per window. See
    /// [`Self::check_at`] for why the sweep rides on the request path.
    fn sweep(&self, st: &mut State, now: Instant) {
        match st.next_sweep_at {
            Some(due) if now >= due => {
                st.next_sweep_at = Some(now + self.window);
                let window = self.window;
                st.buckets.retain(|_, e| e.holds_information(now, window));
            }
            // First check: arm the timer rather than sweep an empty map.
            None => st.next_sweep_at = Some(now + self.window),
            _ => {}
        }
    }

    /// [`Self::check_at`], saying whether a denial was the first of its lockout.
    pub fn verdict_at(&self, key: &str, now: Instant) -> Verdict {
        let mut st = self.inner.lock().unwrap();
        self.sweep(&mut st, now);

        let e = st.buckets.entry(Self::digest(key)).or_default();
        if let Some(until) = e.locked_until {
            if now < until {
                return Verdict::Denied { first: false };
            }
            e.locked_until = None;
            e.attempts.clear();
        }
        e.attempts.retain(|t| now.duration_since(*t) < self.window);
        if e.attempts.len() as u32 >= self.max {
            e.locked_until = Some(now + self.lockout);
            return Verdict::Denied { first: true };
        }
        e.attempts.push(now);
        Verdict::Allowed
    }

    /// How long `key` must still wait, or `None` if it may try now. Records
    /// nothing: this is the read half of the failure-counting pair used by
    /// local login, where only a FAILED attempt costs budget (see
    /// [`Self::record_failure_at`]).
    ///
    /// Used where a caller can legitimately succeed several times in a row
    /// (a person signing in from a few tabs): counting successes there locks
    /// out the right password, and the budget exists to slow down guessing,
    /// which only failures are.
    pub fn retry_after_at(&self, key: &str, now: Instant) -> Option<Duration> {
        let st = self.inner.lock().unwrap();
        let until = st.buckets.get(&Self::digest(key))?.locked_until?;
        (now < until).then(|| until - now)
    }

    /// Count one failed attempt for `key` at `now`. The `max`-th failure
    /// inside the window starts the lockout, so the attempt after it is the
    /// first one [`Self::retry_after_at`] refuses — the same "`max` attempts,
    /// then wait" budget [`Self::check_at`] gives, counting failures only.
    pub fn record_failure_at(&self, key: &str, now: Instant) {
        let mut st = self.inner.lock().unwrap();
        self.sweep(&mut st, now);
        let e = st.buckets.entry(Self::digest(key)).or_default();
        if e.locked_until.is_some_and(|until| now < until) {
            return;
        }
        e.locked_until = None;
        e.attempts.retain(|t| now.duration_since(*t) < self.window);
        e.attempts.push(now);
        if e.attempts.len() as u32 >= self.max {
            e.locked_until = Some(now + self.lockout);
            e.attempts.clear();
        }
    }

    /// Forget `key`'s failures: a success proves the caller holds the secret,
    /// so the guesses before it no longer count against them.
    pub fn clear(&self, key: &str) {
        self.inner
            .lock()
            .unwrap()
            .buckets
            .remove(&Self::digest(key));
    }

    pub fn retry_after(&self, key: &str) -> Option<Duration> {
        self.retry_after_at(key, Instant::now())
    }

    pub fn record_failure(&self, key: &str) {
        self.record_failure_at(key, Instant::now())
    }

    pub fn check(&self, key: &str) -> bool {
        self.check_at(key, Instant::now())
    }

    pub fn verdict(&self, key: &str) -> Verdict {
        self.verdict_at(key, Instant::now())
    }

    /// Total bytes the map spends on keys. Test-only probe for the bound
    /// documented on [`RateLimiter`]; it grows with the caller's input if the
    /// keys are ever stored raw again.
    #[cfg(test)]
    fn stored_key_bytes(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .buckets
            .keys()
            .map(|k| k.len())
            .sum()
    }

    /// How many buckets the map is holding.
    ///
    /// Test-only, and it exists because eviction is **otherwise unobservable**:
    /// dropping an entry that holds no information changes nothing a caller can
    /// see — `check` answers identically whether the stale entry was kept or
    /// removed — so the leak and the fix are behaviourally identical from
    /// outside. The only way to test the sweep at all is to look at the map,
    /// and this is the smallest thing that allows it.
    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.inner.lock().unwrap().buckets.len()
    }
}

/// The caller's IP as the edge reports it — the first hop in `X-Forwarded-For`.
/// Shared by local login (`egret-auth`) and the machine-IdP token endpoint so
/// both limiters agree on what "one caller" means behind the same proxy. Empty
/// when the header is absent, which collapses unlabelled callers into one
/// bucket per subject.
///
/// # This is only as trustworthy as the edge in front of it
///
/// Reading hop one is sound only where the edge **overwrites** the header
/// rather than appending to it. Both edges this repo ships do:
///
/// - **The Helm ingress.** ingress-nginx runs with
///   `use-forwarded-headers=false` and emits
///   `proxy_set_header X-Forwarded-For $remote_addr`.
/// - **The SPA server.** `frontend/nginx-prod.conf` sets `$remote_addr` in
///   every one of its seven proxy blocks. It used to append with
///   `$proxy_add_x_forwarded_for`, which made hop one the *client's own*
///   value and the limiter resettable on demand: measured through the frontend
///   pod, 60 `POST /oauth/token` with a fixed spoofed `X-Forwarded-For`
///   reached 429, then one header edit with the same `client_id` was allowed
///   again — a full fresh budget. `location /api/` carried the same line, so
///   the login limiter went the same way. Fixed at the edge, not here.
///
/// Any **new** edge placed in front of the backend has to take the same
/// posture. This function cannot enforce it: from in here an appended header is
/// indistinguishable from a genuine multi-proxy chain, which is why the rule
/// lives in the edge config and is written down there.
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

    /// An entry with no attempts left in the window and no lockout is tracking
    /// nothing. It must not still be there after a later check.
    #[test]
    fn an_entry_that_tracks_nothing_is_dropped() {
        let rl = RateLimiter::new(5, 300, 600);
        let t0 = Instant::now();

        assert!(rl.check_at("one-and-done", t0));
        assert_eq!(rl.tracked_keys(), 1, "the caller is being tracked");

        // Past the window: that single attempt no longer counts, and this
        // caller was never locked out. A later check by someone else sweeps.
        assert!(rl.check_at("someone-else", t0 + Duration::from_secs(301)));
        assert_eq!(
            rl.tracked_keys(),
            1,
            "only the caller still being tracked should remain; the entry that \
             held nothing must be gone, not sitting next to it"
        );
    }

    /// The map must not grow with every distinct key the process has ever seen.
    /// The keys are attacker-chosen (`ip:client_id` off a public
    /// `POST /oauth/token`), so unbounded growth here is reachable from the
    /// internet by anyone willing to vary one form field.
    #[test]
    fn the_map_does_not_grow_without_bound_across_distinct_keys() {
        let rl = RateLimiter::new(5, 300, 600);
        let t0 = Instant::now();

        for i in 0..10_000 {
            rl.check_at(&format!("198.51.100.1:client-{i}"), t0);
        }
        assert_eq!(
            rl.tracked_keys(),
            10_000,
            "every caller is tracked at first"
        );

        // Two windows later, none of them has come back.
        rl.check_at("a-later-caller", t0 + Duration::from_secs(601));
        assert_eq!(
            rl.tracked_keys(),
            1,
            "10,000 windows-expired callers must not still be held; without \
             eviction this is 10,001 and climbs with every new key forever"
        );
    }

    /// THE ONE THAT BITES. An entry under an ACTIVE LOCKOUT has no attempts
    /// left in the window — the lockout is the longer of the two clocks by
    /// design — so an eviction that looks only at `attempts` drops it and hands
    /// the locked-out caller a fresh budget after the SHORTER wait. That is the
    /// exact reset the lockout exists to deny, and a test that only checked
    /// "the map shrinks" would pass against it.
    #[test]
    fn an_entry_under_an_active_lockout_survives_the_sweep() {
        // Lockout (900s) deliberately longer than the window (300s).
        let rl = RateLimiter::new(2, 300, 900);
        let t0 = Instant::now();

        assert!(rl.check_at("attacker", t0));
        assert!(rl.check_at("attacker", t0));
        assert!(
            !rl.check_at("attacker", t0),
            "the third attempt locks them out"
        );

        // Past the window, well inside the lockout: the attacker's attempts
        // have all aged out of the window, which is the moment a naive sweep
        // would decide this entry holds nothing.
        let t1 = t0 + Duration::from_secs(400);
        rl.check_at("an-unrelated-caller", t1);

        assert_eq!(
            rl.tracked_keys(),
            2,
            "the locked-out entry must survive the sweep"
        );
        assert!(
            !rl.check_at("attacker", t1),
            "still locked out — evicting the entry would have answered true here"
        );

        // And the lockout runs its full length, not the window's.
        assert!(!rl.check_at("attacker", t0 + Duration::from_secs(899)));
        assert!(rl.check_at("attacker", t0 + Duration::from_secs(901)));
    }

    /// KI-144: only failures cost budget. A run of successes never locks the
    /// caller out, and a success forgives the failures before it.
    #[test]
    fn successes_do_not_count_and_clear_forgives_failures() {
        let rl = RateLimiter::new(5, 300, 300);
        let t0 = Instant::now();
        for _ in 0..4 {
            rl.record_failure_at("ip:a", t0);
        }
        assert_eq!(
            rl.retry_after_at("ip:a", t0),
            None,
            "4 failures of 5: may try"
        );
        rl.clear("ip:a"); // the 5th attempt succeeded
        for _ in 0..4 {
            rl.record_failure_at("ip:a", t0);
        }
        assert_eq!(rl.retry_after_at("ip:a", t0), None);
    }

    /// KI-002: after the fifth failure the caller is told how long to wait;
    /// attempts refused inside the lockout neither extend it nor count.
    #[test]
    fn five_failures_lock_and_report_the_remaining_wait() {
        let rl = RateLimiter::new(5, 300, 300);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(rl.retry_after_at("ip:a", t0), None);
            rl.record_failure_at("ip:a", t0);
        }
        assert_eq!(
            rl.retry_after_at("ip:a", t0),
            Some(Duration::from_secs(300))
        );
        rl.record_failure_at("ip:a", t0 + Duration::from_secs(100));
        assert_eq!(
            rl.retry_after_at("ip:a", t0 + Duration::from_secs(100)),
            Some(Duration::from_secs(200))
        );
        assert_eq!(
            rl.retry_after_at("ip:a", t0 + Duration::from_secs(300)),
            None,
            "the lockout ends on time"
        );
        rl.record_failure_at("ip:a", t0 + Duration::from_secs(301));
        assert_eq!(
            rl.retry_after_at("ip:a", t0 + Duration::from_secs(301)),
            None,
            "after the lockout the caller has a fresh budget"
        );
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
