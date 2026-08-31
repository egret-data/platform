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
/// at which pruning runs at all. The alternative is a map that grows with every
/// client id ever seen.
///
/// **A dropped bucket comes back FULL**, which is what this much idleness would
/// have refilled it to for any rate at or above `burst / IDLE_SECS` — and that
/// caveat is reachable, not vacuous: `{per_key_qps: 1, burst: 10000}` refills
/// 600 tokens in 600 seconds and returns with 10 000. It takes an odd
/// configuration (a burst thousands of times the rate) AND a map over
/// [`PRUNE_ABOVE`], so the exposure is one extra burst for a client that was
/// idle ten minutes on an endpoint whose author asked for a 10 000-deep burst.
/// Stated rather than hidden; a shared store (the reason [`RateLimitStore`] is a
/// trait) would not have the same freedom.
const IDLE_SECS: u64 = 600;
const PRUNE_ABOVE: usize = 4_096;

/// What a bucket that can never refill answers.
///
/// Reachable only by constructing [`Rate`] directly with a non-positive `qps`,
/// which [`Rate::from_spec`] cannot produce. A day, matching
/// `shared::BUCKET_TTL_CEILING_MS`, so the two implementations answer the same
/// shape for the same impossible input: refused, with the longest wait either
/// can express. The alternative — the honest "never" — is `Duration::MAX`, and
/// `retry_after_secs` would put its 584-billion-year value on the wire.
const NEVER_REFILLS: Duration = Duration::from_secs(24 * 60 * 60);

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

/// Why a request was not admitted.
///
/// Two refusals, not one, because they mean opposite things to an operator and
/// answer with different codes. [`Self::Exhausted`] is the limiter working —
/// the published allowance is spent and the caller is told when it refills.
/// [`Self::Unavailable`] is the limiter *unable to work*: the shared store
/// could not be reached, so **no allowance was consulted at all**.
///
/// # Why `Unavailable` is a refusal and not an admission
///
/// Package 4 exists because a process-local limit silently multiplies by the
/// replica count. A store outage that admitted the request would reinstate
/// exactly that defect — unbounded, this time, since nothing would be counting
/// — and it would do it under a condition nobody is watching, which is the
/// worst possible moment for it. The scope of that refusal is bounded on
/// purpose: [`check_request`] only touches the store for a limit an author
/// actually configured, so an endpoint with no `rate_limit` is unaffected by an
/// outage of a store it never consults.
///
/// The counterpart decision is the cache's, and it is deliberately the
/// opposite — see [`super::cache::ResponseStore`]. A cache miss re-executes the
/// query and is correct by construction; an unenforced limit is not correct by
/// anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The published allowance is spent. Retry after this long.
    Exhausted(Duration),
    /// The shared store could not be reached, so the allowance is unknown.
    Unavailable,
}

/// Where a bucket lives.
///
/// In-memory per process ([`TokenBucket`]) is correct only while there is ONE
/// process. A multi-replica deployment swaps
/// [`super::shared::SharedStore`] in — the same trait, the same call
/// site — because otherwise every published limit multiplies by the replica
/// count, silently, and the remedy for egret's Postgres ceiling *is* to scale
/// out.
///
/// # Why this is `async`, and why it may not be bridged
///
/// It was synchronous, and that signature does not survive a store on the other
/// side of a socket. It must not be bridged with `block_on` or a blocking
/// client: both park a tokio worker thread inside the async runtime for the
/// duration of a network round trip, on the request path, which converts a slow
/// store into a stalled executor for every OTHER request the runtime is
/// carrying — a far larger outage than the one being papered over.
#[async_trait::async_trait]
pub trait RateLimitStore: Send + Sync {
    /// `Ok(())` if a token was taken, else why not.
    ///
    /// `now` is a parameter so tests never sleep and never depend on wall-clock
    /// scheduling. A shared implementation cannot compare an `Instant` from
    /// another process, so it converts through a per-process anchor — see
    /// [`super::shared::SharedStore`] — which keeps every existing caller and
    /// every existing test writing the same thing they wrote before.
    async fn check_at(&self, key: &str, rate: Rate, now: Instant) -> Result<(), Refusal>;
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

    /// The token-bucket arithmetic, synchronously, against this process's map.
    ///
    /// Inherent and separate from the [`RateLimitStore`] impl because the two
    /// claims are different: this one is "the arithmetic is right", which needs
    /// no runtime and is what most of the tests below assert; the trait impl
    /// adds "and a refusal from THIS store is `Exhausted`, never
    /// `Unavailable`" — an in-process map has no unreachable state, which is
    /// the whole reason it stays the single-replica implementation.
    pub fn take(&self, key: &str, rate: Rate, now: Instant) -> Result<(), Duration> {
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
        // The stored timestamp never runs backwards. Vacuous here — one process
        // has one monotonic clock, so `Instant::now()` never regresses — and
        // written anyway because the SHARED bucket's identical line is
        // load-bearing (see `shared::BUCKET_SCRIPT`: without it, skew between
        // replicas mints tokens without bound), and the two implementations of
        // this one rule are supposed to read the same.
        b.last = now.max(b.last);
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else if rate.qps > 0.0 {
            // Time until one WHOLE token exists — a shorter answer would send the
            // client back to be refused again.
            Err(Duration::from_secs_f64((1.0 - b.tokens) / rate.qps))
        } else {
            // A bucket that never refills. `(1 - tokens) / 0.0` is `inf`, and
            // `Duration::from_secs_f64(inf)` **panics** — a panic on the serving
            // path, from a value the public `Rate` fields can hold.
            // `Rate::from_spec` answers `None` for a zero rate so nothing
            // published can reach this, which is exactly why it would have gone
            // unnoticed. The shared bucket has the same hole and the same fix.
            Err(NEVER_REFILLS)
        }
    }
}

#[async_trait::async_trait]
impl RateLimitStore for TokenBucket {
    /// Never [`Refusal::Unavailable`]: there is no network here to be
    /// unavailable, and reporting one would let a test assert the fail-closed
    /// path against a store that cannot reach it.
    async fn check_at(&self, key: &str, rate: Rate, now: Instant) -> Result<(), Refusal> {
        self.take(key, rate, now).map_err(Refusal::Exhausted)
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
///
/// # An endpoint with no configured limit never touches the store
///
/// Both arms are inside a `Rate::from_spec(..)` guard, and that placement is
/// what bounds the blast radius of a shared-store outage: an author who never
/// set a rate limit has no bucket to consult, so their endpoint keeps serving
/// while the store is down. It also keeps the round-trip cost off every request
/// that has no limit to enforce. This is a load-bearing property of the fail-
/// closed policy, not an optimisation — see [`Refusal::Unavailable`].
pub async fn check_request(
    store: &dyn RateLimitStore,
    endpoint_id: Uuid,
    client_id: &str,
    spec: &RateLimitSpec,
    now: Instant,
) -> Result<(), Refusal> {
    if let Some(rate) = Rate::from_spec(spec.per_key_qps, spec.burst) {
        store
            .check_at(&client_key(endpoint_id, client_id), rate, now)
            .await?;
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
        store
            .check_at(&endpoint_key(endpoint_id), rate, now)
            .await?;
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
        assert!(tb.take("k", rate, t).is_ok());
        assert!(tb.take("k", rate, t).is_ok());
        let wait = tb
            .take("k", rate, t)
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
        assert!(tb.take("k", rate, t).is_ok());
        assert!(tb.take("k", rate, t).is_err());
        assert!(
            tb.take("k", rate, t + Duration::from_secs(1)).is_ok(),
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
        assert!(tb.take("k", rate, t).is_ok());
        assert!(tb.take("k", rate, t).is_ok());
        assert!(tb.take("k", rate, t).is_err());
        // Half a second at 2 qps is exactly one token.
        assert!(tb.take("k", rate, t + Duration::from_millis(500)).is_ok());
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
        assert!(tb.take("k", rate, t).is_ok());
        let later = t + Duration::from_secs(3_600);
        assert!(tb.take("k", rate, later).is_ok());
        assert!(tb.take("k", rate, later).is_ok());
        assert!(
            tb.take("k", rate, later).is_err(),
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
        assert!(tb.take("a", rate, t).is_ok());
        assert!(tb.take("a", rate, t).is_err());
        assert!(
            tb.take("b", rate, t).is_ok(),
            "one key's exhaustion must not touch another's"
        );
    }

    #[tokio::test]
    async fn one_clients_burst_does_not_exhaust_another_clients_allowance() {
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
        assert!(check_request(&tb, ep, "client-a", &spec, t).await.is_ok());
        assert!(check_request(&tb, ep, "client-a", &spec, t).await.is_err());
        // This is the assertion that caught `burst` being applied to the
        // endpoint-wide bucket as well: with `burst = 1` reused there, client-a's
        // one admitted request emptied the shared bucket too, and this endpoint's
        // `per_endpoint_qps: 1000` admitted exactly one request per second.
        assert!(
            check_request(&tb, ep, "client-b", &spec, t).await.is_ok(),
            "a second client must have its own allowance"
        );
    }

    #[tokio::test]
    async fn the_endpoint_wide_limit_refuses_clients_that_are_individually_within_theirs() {
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
        assert!(check_request(&tb, ep, "a", &spec, t).await.is_ok());
        assert!(check_request(&tb, ep, "b", &spec, t).await.is_ok());
        assert!(
            check_request(&tb, ep, "c", &spec, t).await.is_err(),
            "a third client must hit the endpoint-wide bucket"
        );
    }

    #[tokio::test]
    async fn two_endpoints_do_not_share_a_bucket() {
        let tb = TokenBucket::new();
        let spec = RateLimitSpec {
            per_key_qps: 1,
            burst: 1,
            per_endpoint_qps: 1,
        };
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let t = t0();
        assert!(check_request(&tb, a, "client", &spec, t).await.is_ok());
        assert!(check_request(&tb, a, "client", &spec, t).await.is_err());
        assert!(check_request(&tb, b, "client", &spec, t).await.is_ok());
    }

    #[tokio::test]
    async fn an_unconfigured_limit_is_unlimited_not_zero_qps() {
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
            assert!(check_request(&tb, ep, "client", &spec, t).await.is_ok());
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

    /// The in-process store refuses with `Exhausted` and can never answer
    /// `Unavailable`.
    ///
    /// Worth an assertion because [`Refusal::Unavailable`] is answered with a
    /// 503 that says "the limiter could not be consulted". A `TokenBucket` has
    /// no network and therefore no such state; if this ever failed, a
    /// single-replica deployment would be serving 503s for a store it does not
    /// have — and, worse, the shared store's fail-closed test could be
    /// satisfied by the in-process one.
    #[tokio::test]
    async fn the_in_process_store_refuses_with_a_wait_and_never_reports_unavailable() {
        let tb = TokenBucket::new();
        let rate = Rate {
            qps: 1.0,
            burst: 1.0,
        };
        let t = t0();
        assert_eq!(RateLimitStore::check_at(&tb, "k", rate, t).await, Ok(()));
        match RateLimitStore::check_at(&tb, "k", rate, t).await {
            Err(Refusal::Exhausted(d)) => assert!(d > Duration::ZERO),
            other => panic!("an in-process bucket has no unreachable state: {other:?}"),
        }
    }
}
