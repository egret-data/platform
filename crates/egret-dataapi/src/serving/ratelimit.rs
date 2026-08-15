//! Step 5 of spec §6.2 — the serving surface's rate limiter.
//!
//! # Not the login limiter, and the difference is the point
//!
//! `crates/egret-auth/src/ratelimit.rs` has the shape this module copies —
//! `check_at(key, now)` with an injected clock, so no test sleeps — and
//! **semantics that are wrong here**. That one is login-style lockout: exceed the
//! threshold and you are locked out for a fixed period regardless of how quiet
//! you then are. An API client that trips a limit must be able to retry as soon
//! as its allowance has refilled, and must be TOLD when that is. So this is a
//! token bucket, answering `Err(Duration)` — the `Retry-After` a 429 carries.
//!
//! # Two buckets, never one
//!
//! A snapshot configures two limits and they are checked against two separate
//! keys: `per_key_qps` against the calling client, `per_endpoint_qps` against the
//! endpoint. Folding them into one combined key would let a single client's burst
//! exhaust the allowance every other client shares — the endpoint-wide limit
//! exists to bound the ENGINE, and the per-client limit exists to keep one
//! client from being the reason another is refused.
//!
//! Both keys are built from the `endpoint_id`, never from the request path:
//! axum percent-decodes the wildcard capture, so `reports%2Fdaily` and
//! `reports/daily` reach the same endpoint and would otherwise get two buckets —
//! a limit a caller doubles by alternating spellings.
//!
//! # What a rate limit is not
//!
//! It is not backpressure on the engine. `QueryExecPort::execute_as` bounds the
//! CLIENT with `tokio::time::timeout`, and StarRocks has no `query_max_run_time`
//! equivalent, so a query this limiter never admits is unrelated to a query the
//! engine is still running. Sizing these numbers as if they capped engine load
//! would be sizing them against a mechanism that does not exist.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egret_dataapi_api::RateLimitSpec;
use uuid::Uuid;

/// Ceiling applied to a published `per_key_qps`/`per_endpoint_qps`/`burst`.
///
/// `rate_limit` is frozen with no validation rule anywhere (spec §5.3), and
/// spec §7 says the serving path must sanity-check it before a bucket is sized
/// from it. A `u32::MAX` burst is an allowance nobody can exhaust — i.e. a limit
/// that silently is not one.
const RATE_CEILING: f64 = 10_000.0;

/// How long a bucket may sit untouched before a prune may drop it, and the size
/// at which pruning runs at all. A dropped bucket comes back full, which is what
/// this much idleness would have refilled it to anyway for any rate at or above
/// `burst / IDLE_SECS` — the alternative is a map that grows with every client
/// id ever seen.
const IDLE_SECS: u64 = 600;
const PRUNE_ABOVE: usize = 4_096;

/// A sized allowance: `qps` tokens per second, at most `burst` held at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub qps: f64,
    pub burst: f64,
}

impl Rate {
    /// The rate a published pair describes, or `None` for "no limit configured".
    ///
    /// `RateLimitSpec` defaults every field to `0` and P3 validates none of
    /// them, so `0` is what an author who never thought about rate limiting has.
    /// Treating that as "zero requests per second" would make every default
    /// endpoint unreachable; it means unlimited, and the author opts in by
    /// setting a number.
    ///
    /// An absent `burst` becomes `qps`: one second of allowance, the smallest
    /// burst that does not refuse a client sending exactly at its configured
    /// rate. [`check_request`] passes `0` deliberately for the endpoint-wide
    /// bucket — see the comment there.
    pub fn from_spec(qps: u32, burst: u32) -> Option<Self> {
        if qps == 0 {
            return None;
        }
        let qps = (qps as f64).min(RATE_CEILING);
        let burst = if burst == 0 {
            qps
        } else {
            (burst as f64).min(RATE_CEILING)
        };
        Some(Self {
            qps,
            burst: burst.max(1.0),
        })
    }
}

/// Where a bucket lives.
///
/// In-memory per process is correct while the serving surface is in-process. The
/// trait is the seam for when it is not: a multi-replica deployment swaps a
/// shared implementation in — Redis, or a `query`-side table — without a call
/// site changing. Splitting the surface into its own binary would otherwise
/// multiply every published limit by the replica count, silently.
pub trait RateLimitStore: Send + Sync {
    /// `Ok(())` if a token was taken, or the [`Duration`] until one exists.
    ///
    /// `now` is a parameter so tests never sleep and never depend on wall-clock
    /// scheduling.
    fn check_at(&self, key: &str, rate: Rate, now: Instant) -> Result<(), Duration>;
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token bucket with an injected clock, keyed by an arbitrary string.
///
/// The rate is supplied per CALL rather than held as a field, because it comes
/// from the snapshot: two endpoints share this store with different limits, and
/// re-publishing one changes its rate without disturbing anyone else's bucket.
#[derive(Clone, Default)]
pub struct TokenBucket {
    inner: Arc<Mutex<HashMap<String, Bucket>>>,
}

impl TokenBucket {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RateLimitStore for TokenBucket {
    fn check_at(&self, key: &str, rate: Rate, now: Instant) -> Result<(), Duration> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > PRUNE_ABOVE {
            let idle = Duration::from_secs(IDLE_SECS);
            map.retain(|_, b| now.saturating_duration_since(b.last) < idle);
        }
        let b = map.entry(key.to_string()).or_insert(Bucket {
            // A key seen for the first time starts FULL: a client's first request
            // must not be refused because a bucket had not filled yet.
            tokens: rate.burst,
            last: now,
        });
        // `saturating_duration_since`, so a `now` older than `last` (a caller
        // passing an earlier instant) adds nothing rather than panicking.
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * rate.qps).min(rate.burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            // Time until one WHOLE token exists — a shorter answer would send the
            // client back to be refused again.
            Err(Duration::from_secs_f64((1.0 - b.tokens) / rate.qps))
        }
    }
}

/// The per-client bucket key: this client, on THIS endpoint.
///
/// Scoped to the endpoint because the limit it enforces is the endpoint's own
/// published `per_key_qps` — a client-global bucket would mix two endpoints'
/// different rates into one allowance and enforce neither.
pub fn client_key(endpoint_id: Uuid, client_id: &str) -> String {
    format!("c:{endpoint_id}:{client_id}")
}

/// The endpoint-wide bucket key. The `e:`/`c:` prefixes keep the two namespaces
/// apart, so no client id can be spelled to land in the endpoint's bucket.
pub fn endpoint_key(endpoint_id: Uuid) -> String {
    format!("e:{endpoint_id}")
}

/// Apply both of a snapshot's limits, returning the `Retry-After` of whichever
/// refused.
///
/// The client's own bucket is checked FIRST, deliberately: if a single client is
/// the one flooding, it is refused without having consumed a token from the
/// endpoint-wide allowance that every other client shares. The reverse order
/// makes one client able to starve the rest even while being refused itself.
///
/// The converse cost is stated rather than hidden: when the endpoint-wide limit
/// is what refuses, the client's own bucket has already paid a token for a
/// request it never got. That is a slightly stricter limit under contention, not
/// a leak, and it is the cheaper of the two mistakes.
pub fn check_request(
    store: &dyn RateLimitStore,
    endpoint_id: Uuid,
    client_id: &str,
    spec: &RateLimitSpec,
    now: Instant,
) -> Result<(), Duration> {
    if let Some(rate) = Rate::from_spec(spec.per_key_qps, spec.burst) {
        store.check_at(&client_key(endpoint_id, client_id), rate, now)?;
    }
    // **The published `burst` belongs to the per-KEY limit, and is not reused
    // here.** `RateLimitSpec` carries one burst, written next to `per_key_qps`;
    // applying it to the endpoint-wide bucket would size a limit shared by every
    // client with a number the author picked for one of them. Concretely, a
    // `{per_key_qps: 1, burst: 1, per_endpoint_qps: 1000}` endpoint — a strict
    // per-client limit and a deliberately generous shared one — would admit ONE
    // request per second in total, and the second client would be refused by a
    // limit the author set to 1000. The endpoint-wide bucket therefore takes one
    // second of its own rate as its burst.
    if let Some(rate) = Rate::from_spec(spec.per_endpoint_qps, 0) {
        store.check_at(&endpoint_key(endpoint_id), rate, now)?;
    }
    Ok(())
}

/// The `Retry-After` header value for a wait: whole seconds, rounded UP and
/// never `0`. A `Retry-After: 0` invites an immediate retry that is refused
/// again, which is a retry storm with extra steps.
pub fn retry_after_secs(d: Duration) -> u64 {
    d.as_secs_f64().ceil().max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test below constructs its instants explicitly from one `t0`. Two
    /// `Instant::now()` readings can coincide, and an assertion whose two time
    /// points may be equal proves nothing about refill.
    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_burst_is_allowed_then_refused_with_a_wait() {
        let tb = TokenBucket::new();
        let rate = Rate {
            qps: 1.0,
            burst: 2.0,
        };
        let t = t0();
        assert!(tb.check_at("k", rate, t).is_ok());
        assert!(tb.check_at("k", rate, t).is_ok());
        let wait = tb
            .check_at("k", rate, t)
            .expect_err("the third call in the same instant must be refused");
        assert!(
            wait > Duration::ZERO && wait <= Duration::from_secs(1),
            "a 1 qps bucket refills a token within a second, got {wait:?}"
        );
    }

    #[test]
    fn the_bucket_refills_and_the_caller_is_retryable_immediately_after() {
        // The property that separates this from the login limiter: after the
        // stated wait the SAME key is allowed again, with no lockout period.
        let tb = TokenBucket::new();
        let rate = Rate {
            qps: 1.0,
            burst: 1.0,
        };
        let t = t0();
        assert!(tb.check_at("k", rate, t).is_ok());
        assert!(tb.check_at("k", rate, t).is_err());
        assert!(
            tb.check_at("k", rate, t + Duration::from_secs(1)).is_ok(),
            "one second at 1 qps is one token"
        );
    }

    #[test]
    fn a_login_style_lockout_would_fail_this() {
        // `egret-auth`'s limiter locks the key for a fixed period once tripped,
        // so a call after the refill window would still be refused. Written as
        // its own test because copying that module's shape without its semantics
        // is the mistake this module exists to avoid.
        let tb = TokenBucket::new();
        let rate = Rate {
            qps: 2.0,
            burst: 2.0,
        };
        let t = t0();
        assert!(tb.check_at("k", rate, t).is_ok());
        assert!(tb.check_at("k", rate, t).is_ok());
        assert!(tb.check_at("k", rate, t).is_err());
        // Half a second at 2 qps is exactly one token.
        assert!(tb
            .check_at("k", rate, t + Duration::from_millis(500))
            .is_ok());
    }

    #[test]
    fn tokens_do_not_accumulate_beyond_the_burst() {
        // Without the `.min(burst)` an endpoint idle for an hour would admit an
        // hour's worth of requests at once — the opposite of a rate limit.
        let tb = TokenBucket::new();
        let rate = Rate {
            qps: 1.0,
            burst: 2.0,
        };
        let t = t0();
        assert!(tb.check_at("k", rate, t).is_ok());
        let later = t + Duration::from_secs(3_600);
        assert!(tb.check_at("k", rate, later).is_ok());
        assert!(tb.check_at("k", rate, later).is_ok());
        assert!(
            tb.check_at("k", rate, later).is_err(),
            "an idle hour must be worth `burst` tokens, not 3600"
        );
    }

    #[test]
    fn separate_keys_hold_separate_allowances() {
        let tb = TokenBucket::new();
        let rate = Rate {
            qps: 1.0,
            burst: 1.0,
        };
        let t = t0();
        assert!(tb.check_at("a", rate, t).is_ok());
        assert!(tb.check_at("a", rate, t).is_err());
        assert!(
            tb.check_at("b", rate, t).is_ok(),
            "one key's exhaustion must not touch another's"
        );
    }

    #[test]
    fn one_clients_burst_does_not_exhaust_another_clients_allowance() {
        // The reason there are two keys rather than one combined one, asserted
        // through `check_request` — the function a handler actually calls.
        let tb = TokenBucket::new();
        let ep = Uuid::new_v4();
        let spec = RateLimitSpec {
            per_key_qps: 1,
            burst: 1,
            // Deliberately generous, so only the PER-CLIENT limit can refuse.
            per_endpoint_qps: 100,
        };
        let t = t0();
        assert!(check_request(&tb, ep, "client-a", &spec, t).is_ok());
        assert!(check_request(&tb, ep, "client-a", &spec, t).is_err());
        // This is the assertion that caught `burst` being applied to the
        // endpoint-wide bucket as well: with `burst = 1` reused there, client-a's
        // one admitted request emptied the shared bucket too, and this endpoint's
        // `per_endpoint_qps: 1000` admitted exactly one request per second.
        assert!(
            check_request(&tb, ep, "client-b", &spec, t).is_ok(),
            "a second client must have its own allowance"
        );
    }

    #[test]
    fn the_endpoint_wide_limit_refuses_clients_that_are_individually_within_theirs() {
        // The other half: each client is under its own per-key limit, and the
        // endpoint's shared allowance is what runs out. Without the second key
        // this endpoint has no ceiling at all as clients are added.
        let tb = TokenBucket::new();
        let ep = Uuid::new_v4();
        let spec = RateLimitSpec {
            per_key_qps: 100,
            burst: 2,
            per_endpoint_qps: 2,
        };
        let t = t0();
        assert!(check_request(&tb, ep, "a", &spec, t).is_ok());
        assert!(check_request(&tb, ep, "b", &spec, t).is_ok());
        assert!(
            check_request(&tb, ep, "c", &spec, t).is_err(),
            "a third client must hit the endpoint-wide bucket"
        );
    }

    #[test]
    fn two_endpoints_do_not_share_a_bucket() {
        let tb = TokenBucket::new();
        let spec = RateLimitSpec {
            per_key_qps: 1,
            burst: 1,
            per_endpoint_qps: 1,
        };
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let t = t0();
        assert!(check_request(&tb, a, "client", &spec, t).is_ok());
        assert!(check_request(&tb, a, "client", &spec, t).is_err());
        assert!(check_request(&tb, b, "client", &spec, t).is_ok());
    }

    #[test]
    fn an_unconfigured_limit_is_unlimited_not_zero_qps() {
        // `RateLimitSpec` defaults every field to 0 and nothing validates it, so
        // reading 0 as "zero requests per second" would make every endpoint an
        // author never configured unreachable.
        assert_eq!(Rate::from_spec(0, 0), None);
        let tb = TokenBucket::new();
        let spec = RateLimitSpec {
            per_key_qps: 0,
            burst: 0,
            per_endpoint_qps: 0,
        };
        let ep = Uuid::new_v4();
        let t = t0();
        for _ in 0..100 {
            assert!(check_request(&tb, ep, "client", &spec, t).is_ok());
        }
    }

    #[test]
    fn published_rates_are_clamped_before_a_bucket_is_sized_from_them() {
        let r = Rate::from_spec(u32::MAX, u32::MAX).expect("configured");
        assert_eq!(r.qps, RATE_CEILING);
        assert_eq!(r.burst, RATE_CEILING);
        // An absent burst is one second of allowance, never zero.
        assert_eq!(
            Rate::from_spec(5, 0),
            Some(Rate {
                qps: 5.0,
                burst: 5.0
            })
        );
    }

    #[test]
    fn retry_after_never_advertises_zero_seconds() {
        assert_eq!(retry_after_secs(Duration::from_millis(1)), 1);
        assert_eq!(retry_after_secs(Duration::from_millis(1_400)), 2);
        assert_eq!(retry_after_secs(Duration::ZERO), 1);
    }
}
