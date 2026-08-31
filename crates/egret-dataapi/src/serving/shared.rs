//! Package 4 — the shared store behind both serving seams, and the refusal that
//! guards the window before one is configured.
//!
//! # The defect this closes
//!
//! `deploy/helm/egret/values.yaml` makes `backend.replicas` a value, and both
//! pieces of serving state were **process-local**: [`super::ratelimit::TokenBucket`]
//! held the token buckets in a `HashMap`, and [`super::cache::ResponseCache`]
//! held the responses in another. Scaling to N replicas therefore multiplied
//! every published rate limit by N — silently, with no instrument that could
//! see it before Package 2 — and fragmented the cache N ways.
//!
//! The two halves are not equally bad and the remedy is not symmetric. A
//! fragmented cache is slower and never wrong. A limit multiplied by the
//! replica count is a limit that **weakens exactly as engine load rises**,
//! because the remedy for egret's Postgres ceiling *is* to scale out. That
//! asymmetry decides the failure policy below.
//!
//! # Atomicity is the whole design of the bucket
//!
//! A token bucket read, modified and written back across a network is a race:
//! two replicas both read `tokens = 1`, both decide they may take it, both
//! write `tokens = 0`, and two requests are admitted where the allowance was
//! one. That over-admission happens under **exactly the concurrency the limiter
//! exists to bound**, so it is at its worst when it matters most, and it is
//! invisible — every individual answer looks correct.
//!
//! So the bucket is a Lua script ([`BUCKET_SCRIPT`]). Redis executes a script
//! atomically: no other command from any client runs between its first and last
//! statement. The read, the refill, the decrement and the write are therefore
//! one indivisible step, and it is also ONE round trip rather than three.
//! `two_shared_stores_cannot_both_take_the_last_token` in
//! `tests/serving_shared_store.rs` drives that from two independent
//! [`SharedStore`] handles at once and asserts the total admitted equals the
//! burst — and asserts, in the same test, that two [`TokenBucket`]s admit
//! twice it, which is the defect stated as a measurement rather than as prose.
//!
//! # Two failure policies, deliberately opposite
//!
//! When the store is unreachable:
//!
//! * **The cache fails OPEN.** A read answers `None` and a write answers
//!   `false`. Both are outcomes every caller already handles on a cold start,
//!   and a miss re-executes the query and re-shapes it against the frozen
//!   contract — the same work the hit was a shortcut for. Nothing about who may
//!   see what is decided here.
//! * **The limiter fails CLOSED.** [`super::ratelimit::Refusal::Unavailable`],
//!   answered as `503 rate_limiter_unavailable`. Admitting instead would
//!   reinstate the very defect this package exists to remove — and reinstate it
//!   unbounded, since nothing would be counting at all — under a condition
//!   nobody is watching.
//!
//! The cost of failing closed is stated rather than hidden: while the store is
//! down, every endpoint **whose author configured a rate limit** answers 503.
//! It is bounded on purpose — [`super::ratelimit::check_request`] consults the
//! store only inside a `Rate::from_spec(..)` guard, so an endpoint with no
//! published limit is untouched by an outage of a store it never reads — and it
//! is retryable, carrying the same one-second `Retry-After` as `overloaded`.
//!
//! Neither failure is silent: [`super::metrics::STORE_CALLS_TOTAL`] counts
//! every round trip by `store`, `op` and `result`, and
//! [`super::metrics::STORE_DURATION_SECONDS`] times it.
//!
//! # What the deployment must assume about eviction
//!
//! Stated rather than inherited, because Redis' *default* `maxmemory-policy` is
//! `noeviction`, and under it a full instance fails **writes** — which would
//! take the limiter down (fail-closed ⇒ 503) to protect a cache that is
//! optional. That is the worst of the available couplings, so the chart sets
//! the policy explicitly to `allkeys-lru` beside a `maxmemory`
//! (`deploy/helm/egret/values.yaml`, `sharedStore.maxmemoryPolicy`).
//!
//! The consequence of `allkeys-lru`, stated because it is real: an evicted
//! limiter bucket comes back **full**, granting that key at most one extra
//! `burst`. This is the identical caveat [`super::ratelimit::IDLE_SECS`] already
//! documents for the in-process map.
//!
//! An earlier version of this paragraph went on to claim the exposure was
//! bounded to idle clients, "since a client that is currently sending is by
//! definition among the most recently used". **That is not true and the
//! sentence is gone.** Redis' LRU is approximate — it samples a handful of keys
//! and evicts the oldest of the sample — so a bucket belonging to a client
//! mid-burst can be evicted, and that client gets a fresh full `burst`. The
//! honest bound is *one burst per eviction*, on any key, not *only idle keys*.
//!
//! What is true is where the pressure comes from, and it is worth an operator's
//! attention: a bucket is a two-field hash of a few dozen bytes, while a cache
//! entry is up to [`MAX_ENTRY_BYTES`] (4 MiB). The chart's default `maxmemory`
//! is 256 MB, which is as few as ~64 largest-size entries — so the instance
//! reaching its ceiling is almost always a cache-pressure event, and the keys
//! in the sample are almost always cache entries. Raising `sharedStore.maxmemory`
//! is the lever; `egret_dataapi_store_calls_total` will not show eviction (an
//! evicted key is a MISS and a fresh bucket, never an error), so the instrument
//! for it is Valkey's own `evicted_keys`, not egret's.
//!
//! Eviction does **not** interact with the fail-closed limiter: it is not a
//! write failure. `noeviction` would be — which is the whole reason the policy
//! is set. Every key this module writes also carries its own expiry, so the
//! *steady* state is bounded by TTL rather than by eviction.
//!
//! # Sharing one Redis between two deployments
//!
//! Do not. Both key spaces are prefixed ([`RL_PREFIX`], [`RC_PREFIX`]) so the
//! limiter and the cache cannot collide with each other, but two egret
//! deployments pointed at the same instance would share buckets and cache
//! entries. Redis' numbered logical databases are the cheap separation and need
//! no code: `redis://host:6379/1`.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use redis::AsyncCommands;

use super::cache::{estimate_bytes, CachedResponse, ResponseStore, MAX_ENTRY_BYTES};
use super::metrics::{self, StoreOp};
use super::ratelimit::{Rate, RateLimitStore, Refusal};

/// Key prefix for a token bucket. Distinct from [`RC_PREFIX`] so no cache key —
/// a 64-character hex digest — can be spelled to land on a bucket, and so an
/// operator running `SCAN MATCH` can see the two populations apart.
const RL_PREFIX: &str = "egret:dataapi:rl:";
/// Key prefix for a cached response.
const RC_PREFIX: &str = "egret:dataapi:rc:";

/// How long any single store round trip may take before it counts as a failure.
///
/// Chosen against what the two failure policies cost, not against a typical
/// latency: an in-cluster Redis answers a scripted `HMGET`/`HSET` in well under
/// a millisecond, so 500 ms is ~500× the expected time and will not fire on
/// ordinary jitter or a GC pause — while still bounding how long a served
/// request can be held up by a store that has stopped answering. Without a
/// timeout the fail-closed limiter would not fail closed at all; it would hang,
/// which is the one outcome worse than either policy.
const STORE_TIMEOUT: Duration = Duration::from_millis(500);

/// Extra life given to a bucket key beyond the time it takes to refill from
/// empty to `burst`.
///
/// A bucket that has been idle for `burst / qps` seconds has refilled to full,
/// so a key dropped at that point carries no information and comes back
/// identical. The slack absorbs clock skew between replicas so the key is not
/// dropped a moment early.
const BUCKET_TTL_SLACK: Duration = Duration::from_secs(1);

/// Hard ceiling on a bucket key's lifetime.
///
/// The refill time bounds it in every configuration [`Rate::from_spec`] can
/// produce — the widest is `burst / qps` at the clamped extremes, under three
/// hours. This exists for the shape `from_spec` cannot reach but the public
/// [`Rate`] fields can: a `qps` of zero makes `burst / qps` infinite, which
/// saturates to `u64::MAX` and would be rejected by `PEXPIRE` as an invalid
/// expiry — i.e. every check on that key failing closed. A clamp turns an
/// unreachable-but-ugly outage into a long TTL.
const BUCKET_TTL_CEILING_MS: u64 = 24 * 60 * 60 * 1_000;

/// The atomic token bucket.
///
/// `KEYS[1]` is the bucket. `ARGV` is `qps`, `burst`, `now_ms`, `ttl_ms`.
/// Returns the milliseconds to wait: `0` means a token was taken.
///
/// The arithmetic is [`super::ratelimit::TokenBucket::take`]'s, deliberately —
/// a first sighting starts FULL, refill is `elapsed * qps` clamped to `burst`,
/// and a refusal answers the time until one WHOLE token exists, because a
/// shorter answer sends the client back to be refused again.
/// `shared_and_in_process_buckets_agree_on_a_single_callers_sequence` pins that
/// the two implementations do not drift apart.
///
/// `now` is passed in rather than read with `redis.call('TIME')`: the caller
/// already has an injected clock (so tests never sleep), and a script that
/// reads the clock itself is one that cannot be driven deterministically from
/// either side.
///
/// # The stored timestamp is a MONOTONE ENVELOPE, and that is the whole defence
///
/// Because `now` comes from the caller, N replicas send N slightly different
/// numbers for the same real moment. The first version of this script wrote
/// `l = now` unconditionally, and that made the skew a **token generator**: a
/// replica whose clock lagged rewound the stored timestamp, and the next call
/// from a leading replica measured `elapsed` over an interval that had already
/// been billed. Every alternation minted tokens, and nothing bounded it —
/// measured on Valkey 8 at `qps = 10, burst = 10`, 400 calls at ONE real
/// instant, alternating two clocks:
///
/// | skew | admitted, before | admitted, now |
/// |---|---|---|
/// | 0 ms | 10 | 10 |
/// | 5 ms | 20 | 10 |
/// | 50 ms | 110 | 10 |
/// | 500 ms | **400 — no limit at all** | 11 |
/// | 1 hour | **400** | 11 |
///
/// `if now < last then now = last end` is what closes it, and it is a fix to
/// the timestamp SOURCE rather than a clamp on a symptom: the value the bucket
/// stores is now non-decreasing by construction, so the bucket runs on the
/// monotone envelope of every replica's clock — one clock, which advances at
/// exactly real rate because a maximum of clocks that each tick at real rate
/// does. The old `if elapsed < 0 then elapsed = 0 end` guarded the *arithmetic*
/// and then wrote the rewound value anyway, which is why it read as a fix and
/// was not one; with the floor in place `elapsed` cannot be negative, so that
/// line is gone rather than kept as reassurance.
///
/// What remains is a ONE-TIME offset bounded by `burst`, not a per-alternation
/// leak: a clock that jumps the envelope forward (a lagging replica catching
/// up, an NTP step) can refill the bucket once, and `tokens > burst` clamps
/// that to the allowance the author published. That is the 11 above, at every
/// skew from 500 ms to an hour. It is also why [`SharedStore::anchor_ms`] does
/// not need re-syncing: a correction can only move the envelope forward, once,
/// for at most one burst.
///
/// # `qps <= 0`
///
/// Unreachable through [`Rate::from_spec`], which returns `None` for `0`, but
/// [`Rate`]'s fields are public and a zero divides. It used to produce
/// `wait = inf`, whose conversion to a Redis integer reply is undefined and
/// could come back as `0` — an ADMISSION, from a rate of zero per second. It
/// now answers the key's own TTL: refused, with the longest wait this bucket
/// can honestly express, since a bucket that never refills has no shorter one.
const BUCKET_SCRIPT: &str = r#"
local qps    = tonumber(ARGV[1])
local burst  = tonumber(ARGV[2])
local now    = tonumber(ARGV[3])
local ttl_ms = tonumber(ARGV[4])

local state  = redis.call('HMGET', KEYS[1], 't', 'l')
local tokens = tonumber(state[1])
local last   = tonumber(state[2])
if tokens == nil or last == nil then
  tokens = burst
  last   = now
end

-- The bucket's clock never runs backwards. A caller behind the stored value
-- contributes NO refill and, crucially, does not rewind `l` for the caller
-- that is ahead. See the doc comment: without this line the skew between two
-- replicas mints tokens on every alternation, without bound.
if now < last then now = last end

local elapsed = now - last
tokens = tokens + (elapsed / 1000.0) * qps
if tokens > burst then tokens = burst end

local wait = 0
if tokens >= 1.0 then
  tokens = tokens - 1.0
elseif qps > 0 then
  wait = math.ceil(((1.0 - tokens) / qps) * 1000.0)
  if wait < 1 then wait = 1 end
else
  -- A bucket that never refills. `(1 - tokens) / 0` is inf, and an inf reply
  -- converts to a Redis integer as anything at all -- including 0, which this
  -- caller would read as ADMITTED. The key's own TTL instead: refused.
  wait = ttl_ms
end

redis.call('HSET', KEYS[1], 't', tokens, 'l', now)
redis.call('PEXPIRE', KEYS[1], ttl_ms)
return wait
"#;

/// What a cached response looks like in the store.
///
/// JSON rather than a binary encoding so an operator can `GET` a key and read
/// it, and so a version of egret that cannot parse an entry written by another
/// fails to deserialise it — which is a miss, the safe outcome — rather than
/// misreading it. The `row_counts` list is carried for the reason
/// [`CachedResponse`] carries it: `meta` reports a count PER STATEMENT and a hit
/// runs none of them, so a single total cannot rebuild the body a miss would
/// have produced.
///
/// **The cache KEY is not touched by any of this.** It is
/// [`super::cache::cache_key`]'s digest verbatim, behind [`RC_PREFIX`] — the
/// length-prefixed stream with the principal trailing, whose collision proof and
/// forgery tests are the whole cross-principal safety argument. A shared store
/// that re-derived or shortened it would be a cache that leaks across
/// principals, which is far worse than a cache that fragments.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredEntry {
    data: serde_json::Value,
    row_counts: Vec<u64>,
}

/// A Redis/Valkey-backed implementation of BOTH serving seams.
///
/// One type rather than two because it is one connection and one operational
/// dependency: two would double the connection count per replica to no end, and
/// would let a deployment end up with a shared limiter and a process-local
/// cache (or the reverse) by configuring only half of it.
pub struct SharedStore {
    conn: redis::aio::ConnectionManager,
    /// Built once. `redis::Script` computes the SHA1 at construction and each
    /// call is an `EVALSHA` that falls back to `EVAL` on `NOSCRIPT` — so a
    /// Redis that restarted and lost its script cache repairs itself on the
    /// next request rather than failing one.
    bucket: redis::Script,
    /// `Instant` is opaque and monotonic *within one process*, so it cannot be
    /// compared against a value another replica minted. Every caller and every
    /// existing test passes one, so rather than change that, the store anchors
    /// its own process's `Instant` to the wall clock ONCE and converts.
    ///
    /// Two replicas therefore agree to within their NTP skew, which is the
    /// accuracy any wall-clock scheme would have had. It is also why the
    /// injected clock still works: a test that passes `t0 + 3600s` produces a
    /// `now_ms` an hour ahead, and the script sees exactly that.
    ///
    /// **The anchor is captured once and never re-synced, and that is now
    /// safe.** It was not safe before [`BUCKET_SCRIPT`] gained its monotone
    /// floor: any skew — including a permanent one left behind by an NTP step
    /// after pod start, which this anchor by design never sees — minted tokens
    /// on every alternation between replicas, without bound. With the floor,
    /// the disagreement can only cost a ONE-TIME forward jump of the bucket's
    /// envelope, itself clamped to `burst`. Re-syncing would not improve that
    /// and would introduce backward steps, which is the direction that used to
    /// hurt.
    ///
    /// Anchoring to the wall clock at all (rather than to `Instant` alone) is
    /// what lets a restarted replica land near its peers instead of at zero:
    /// `Instant`'s origin is arbitrary per process, so an un-anchored replica
    /// would present a `now_ms` far below every stored `l` and — thanks to the
    /// floor — simply contribute no refill, forever.
    anchor_instant: Instant,
    anchor_ms: u64,
}

impl SharedStore {
    /// Open the store, or fail.
    ///
    /// **A failure here is a startup failure, not a fallback.** An operator who
    /// configured a shared store and cannot reach it must not silently boot
    /// into the process-local implementation: that is the multiplied-limit
    /// defect again, arrived at by a different road and with the configuration
    /// on disk saying it cannot happen. `ConnectionManager` retries the initial
    /// connection with backoff before this returns, so a store that is merely
    /// slow to come up does not fail a pod.
    ///
    /// After construction the manager reconnects on its own; a connection lost
    /// mid-flight surfaces as a per-call error and is handled by the two
    /// policies above, not by another connect.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = redis::aio::ConnectionManager::new(client).await?;
        Ok(Self {
            conn,
            bucket: redis::Script::new(BUCKET_SCRIPT),
            anchor_instant: Instant::now(),
            anchor_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default(),
        })
    }

    /// This process's `Instant` as a wall-clock millisecond every replica can
    /// compare.
    ///
    /// `saturating_duration_since`, so a `now` older than the anchor (a caller
    /// passing an earlier instant, which the in-process bucket also tolerates)
    /// yields the anchor rather than panicking.
    fn to_ms(&self, now: Instant) -> u64 {
        self.anchor_ms
            + now
                .saturating_duration_since(self.anchor_instant)
                .as_millis() as u64
    }
}

#[async_trait::async_trait]
impl RateLimitStore for SharedStore {
    async fn check_at(&self, key: &str, rate: Rate, now: Instant) -> Result<(), Refusal> {
        // A bucket is worthless once it has refilled to `burst`, so it expires
        // then. Unlike the in-process map's fixed `IDLE_SECS` prune this cannot
        // drop a bucket that still holds information — see the module comment.
        let ttl_ms = ((rate.burst / rate.qps * 1000.0).ceil().max(1.0) as u64)
            .saturating_add(BUCKET_TTL_SLACK.as_millis() as u64)
            .min(BUCKET_TTL_CEILING_MS);

        let started = Instant::now();
        let mut conn = self.conn.clone();
        // Bound to a `let` rather than chained into the `invoke_async` call: the
        // builder is a temporary the returned future borrows from, so chaining
        // drops it at the end of the statement.
        let mut invocation = self.bucket.prepare_invoke();
        invocation
            .key(format!("{RL_PREFIX}{key}"))
            .arg(rate.qps)
            .arg(rate.burst)
            .arg(self.to_ms(now))
            .arg(ttl_ms);
        let call = invocation.invoke_async::<i64>(&mut conn);

        let wait_ms = match tokio::time::timeout(STORE_TIMEOUT, call).await {
            Ok(Ok(ms)) => ms,
            // Both arms are the SAME outcome on purpose: a store that answered
            // an error and a store that did not answer are equally "no
            // allowance was consulted", and inventing a distinction here would
            // invite one of them to be treated as admissible later.
            Ok(Err(e)) => {
                metrics::record_store_call(StoreOp::RateLimit, false, started.elapsed());
                tracing::warn!(
                    error = %e,
                    "data-api shared rate-limit store failed; refusing rather than admitting \
                     an unlimited request"
                );
                return Err(Refusal::Unavailable);
            }
            Err(_elapsed) => {
                metrics::record_store_call(StoreOp::RateLimit, false, started.elapsed());
                tracing::warn!(
                    timeout_ms = STORE_TIMEOUT.as_millis() as u64,
                    "data-api shared rate-limit store timed out; refusing rather than admitting \
                     an unlimited request"
                );
                return Err(Refusal::Unavailable);
            }
        };
        metrics::record_store_call(StoreOp::RateLimit, true, started.elapsed());

        if wait_ms <= 0 {
            Ok(())
        } else {
            Err(Refusal::Exhausted(Duration::from_millis(wait_ms as u64)))
        }
    }
}

#[async_trait::async_trait]
impl ResponseStore for SharedStore {
    /// A live entry, or `None`. Expiry is the store's own `PX`, not a
    /// comparison against `now` — see [`ResponseStore`] for why the two
    /// implementations differ here and what that means for a test.
    async fn get_at(&self, key: &str, _now: Instant) -> Option<CachedResponse> {
        let started = Instant::now();
        let mut conn = self.conn.clone();
        let fut = conn.get::<_, Option<Vec<u8>>>(format!("{RC_PREFIX}{key}"));
        let raw = match tokio::time::timeout(STORE_TIMEOUT, fut).await {
            Ok(Ok(v)) => v,
            // Fails OPEN: a miss. The caller re-executes and re-shapes, which
            // is the work a hit was a shortcut for.
            Ok(Err(e)) => {
                metrics::record_store_call(StoreOp::CacheGet, false, started.elapsed());
                tracing::debug!(error = %e, "data-api shared cache read failed; treating as a miss");
                return None;
            }
            Err(_) => {
                metrics::record_store_call(StoreOp::CacheGet, false, started.elapsed());
                tracing::debug!("data-api shared cache read timed out; treating as a miss");
                return None;
            }
        };
        metrics::record_store_call(StoreOp::CacheGet, true, started.elapsed());

        let raw = raw?;
        // An entry this build cannot parse is a MISS, never a guess. The
        // alternative — reading what fields it recognises — is how one version's
        // response shape gets served under another's contract.
        match serde_json::from_slice::<StoredEntry>(&raw) {
            Ok(e) => Some(CachedResponse {
                data: Arc::new(e.data),
                row_counts: e.row_counts,
            }),
            Err(e) => {
                tracing::warn!(error = %e, "data-api shared cache entry did not parse; re-executing");
                None
            }
        }
    }

    /// Store a response for `ttl`, or answer `false`.
    ///
    /// # What happens above the maximum entry size
    ///
    /// It is not stored, the call answers `false`, and the next request for the
    /// same key misses and re-executes. Correctness is untouched: a response
    /// that is never cached is a response that is always fresh. The bound is
    /// [`MAX_ENTRY_BYTES`], 4 MiB.
    ///
    /// # The two caches share the constant and NOT the quantity it bounds
    ///
    /// This comment used to say the shared bound was "the same 4 MiB the
    /// in-process cache applies, so an endpoint does not start or stop being
    /// cacheable when a deployment scales out". The constant is the same; the
    /// measured quantity is not, and the invariance claim was false.
    /// [`super::cache::ResponseCache::put_at`] bounds [`estimate_bytes`]; this
    /// bounds the **serialised** length.
    ///
    /// They diverge on numbers. [`estimate_bytes`] charges every
    /// `serde_json::Value::Number` a flat 16 bytes, and a full-precision `f64`
    /// serialises to more than that — `-1.2345678901234567e-300` is 24
    /// characters. So a float-heavy body can estimate under 4 MiB and serialise
    /// over it, and such a response **is cached by a single-replica deployment
    /// and stops being cached once that deployment scales out**.
    /// `a_float_heavy_body_caches_in_process_and_not_in_the_store` builds one —
    /// 220 000 full-precision floats — and asserts both halves, so this
    /// paragraph cannot quietly stop being true, in either direction.
    ///
    /// The consequence is a re-execution, never a wrong answer, and it is
    /// strictly the safe direction: the shared store refuses what it cannot
    /// actually afford to hold. Making the two agree would mean serialising on
    /// every in-process `put` — paying the wire's work twice for every response
    /// the process caches, to remove a performance difference on a body shape
    /// that is already at the edge of cacheable.
    ///
    /// # Why the size is checked twice here
    ///
    /// **Only the second check is a bound.** Stated
    /// that way because the first version of this comment claimed the two were
    /// jointly load-bearing and a mutation showed they are not: deleting the
    /// [`estimate_bytes`] guard changes no outcome, because anything it would
    /// refuse the serialised length refuses too (the estimate over-counts
    /// strings, which is what a large body is made of).
    ///
    /// The estimate stays as a **cost** guard, not a correctness one: it refuses
    /// an absurd body before `serde_json::to_vec` allocates a copy of it, so the
    /// work done to reach the refusal is bounded rather than proportional to the
    /// body. `a_response_over_the_maximum_entry_size_is_not_stored` therefore
    /// asserts the SERIALISED bound with a body built to slip past the estimate
    /// — the estimate itself has no assertion because it can have none.
    async fn put_at(
        &self,
        key: String,
        data: Arc<serde_json::Value>,
        row_counts: Vec<u64>,
        ttl: Duration,
        _now: Instant,
    ) -> bool {
        if estimate_bytes(&data) > MAX_ENTRY_BYTES {
            tracing::debug!("data-api response too large to cache (estimate)");
            return false;
        }
        let entry = StoredEntry {
            data: (*data).clone(),
            row_counts,
        };
        let Ok(payload) = serde_json::to_vec(&entry) else {
            return false;
        };
        if payload.len() > MAX_ENTRY_BYTES {
            tracing::debug!(
                bytes = payload.len(),
                "data-api response too large to cache (serialised)"
            );
            return false;
        }

        let started = Instant::now();
        let mut conn = self.conn.clone();
        // `SET … PX` in one command: the value and its expiry are written
        // together, so there is no window in which an entry exists without one.
        // A cached response with no TTL would be served forever.
        let ttl_ms = ttl.as_millis().max(1) as u64;
        let fut = conn.set_options::<_, _, ()>(
            format!("{RC_PREFIX}{key}"),
            payload,
            redis::SetOptions::default().with_expiration(redis::SetExpiry::PX(ttl_ms)),
        );
        match tokio::time::timeout(STORE_TIMEOUT, fut).await {
            Ok(Ok(())) => {
                metrics::record_store_call(StoreOp::CachePut, true, started.elapsed());
                true
            }
            // Fails OPEN, like the read: not caching is always a legal outcome.
            Ok(Err(e)) => {
                metrics::record_store_call(StoreOp::CachePut, false, started.elapsed());
                tracing::debug!(error = %e, "data-api shared cache write failed; not cached");
                false
            }
            Err(_) => {
                metrics::record_store_call(StoreOp::CachePut, false, started.elapsed());
                tracing::debug!("data-api shared cache write timed out; not cached");
                false
            }
        }
    }
}

/// What backs the serving surface's limiter and cache in this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreChoice {
    /// [`super::ratelimit::TokenBucket`] and [`super::cache::ResponseCache`].
    InProcess,
    /// [`SharedStore`], at this URL.
    Shared(String),
}

/// Decide what backs the serving state, refusing rather than silently
/// multiplying every published limit — §4 decision B's interim guard.
///
/// # Why the process has to be TOLD its replica count
///
/// It cannot discover it. A pod knows its own name and namespace; the number of
/// peers is a property of the Deployment, and reading it would mean giving every
/// backend pod RBAC to `get deployments` for a value the chart already has as a
/// literal. So `backend.replicas` — which exists at `values.yaml:3` — is passed
/// down as `EGRET_DATA_API__REPLICAS`, and this function is what reads it.
///
/// # Why it is a refusal and not a warning
///
/// A warning is a line in a log that an operator reads *after* the limits have
/// already been wrong for a week. The condition is static, known at startup, and
/// has two one-line remedies — which is exactly the shape that should stop a
/// process rather than annotate it. The message names BOTH remedies, because an
/// operator who scaled out on purpose and an operator who scaled out by accident
/// need opposite fixes.
///
/// # The four cases
///
/// | replicas | store URL | result |
/// |---|---|---|
/// | 1 | absent | in-process — the deployment that needs nothing new |
/// | 1 | set | **shared** — an operator who configured one gets it, so the shared path is exercised before it is depended on |
/// | >1 | set | shared |
/// | >1 | absent | **refuse to start** |
///
/// The second row matters more than it looks: a single-replica deployment that
/// ignored a configured store would mean the shared path first ran in
/// production, on the day someone scaled out.
///
/// A blank URL counts as absent. `EGRET_DATA_API__SHARED_STORE_URL=` in a
/// half-written env file deserialises to `Some("")`, and an empty URL would
/// fail to connect — turning a configuration slip into a boot failure whose
/// message names Redis rather than the empty variable. The same trap and the
/// same treatment as `DataApiConfig::public_url`.
pub fn choose_store(replicas: u32, url: Option<&str>) -> Result<StoreChoice, String> {
    let url = url.map(str::trim).filter(|u| !u.is_empty());
    if let Some(url) = url {
        return Ok(StoreChoice::Shared(url.to_string()));
    }
    // `0` is what an unset-but-present env var and a scaled-to-zero Deployment
    // both look like, and neither is "more than one of me". Refusing on it would
    // stop a deployment that is not running at all.
    if replicas > 1 {
        return Err(format!(
            "refusing to start: backend.replicas is {replicas}, but no shared store is \
             configured, so the Data API's rate-limit buckets and response cache would be \
             held per process — every published rate limit would be multiplied by {replicas} \
             and the response cache fragmented {replicas} ways, silently.\n\
             \n\
             Two remedies, either of which clears this:\n\
             \x20 1. Configure the shared store: set `sharedStore.enabled: true` in the chart \
             (or EGRET_DATA_API__SHARED_STORE_URL=redis://…) so all replicas share one \
             limiter and one cache.\n\
             \x20 2. Set `backend.replicas: 1`, which makes the process-local state correct \
             again."
        ));
    }
    Ok(StoreChoice::InProcess)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole decision table, including the two rows a reader is most likely
    /// to get wrong: a configured store wins at ONE replica (so the shared path
    /// is not first exercised in production), and `0` replicas is not "more than
    /// one".
    #[test]
    fn the_store_choice_covers_every_combination() {
        assert_eq!(choose_store(1, None), Ok(StoreChoice::InProcess));
        assert_eq!(choose_store(0, None), Ok(StoreChoice::InProcess));
        assert_eq!(
            choose_store(1, Some("redis://s:6379")),
            Ok(StoreChoice::Shared("redis://s:6379".into()))
        );
        assert_eq!(
            choose_store(4, Some("redis://s:6379")),
            Ok(StoreChoice::Shared("redis://s:6379".into()))
        );
        assert!(choose_store(2, None).is_err());
    }

    /// A blank or whitespace URL is ABSENT, not a URL.
    ///
    /// Which means it must also still trip the multi-replica refusal — the
    /// failure mode being avoided is a half-written
    /// `EGRET_DATA_API__SHARED_STORE_URL=` reading as "configured" and letting a
    /// four-replica deployment boot with four private limiters.
    #[test]
    fn a_blank_store_url_is_absent_and_still_refuses_more_than_one_replica() {
        assert_eq!(choose_store(1, Some("")), Ok(StoreChoice::InProcess));
        assert_eq!(choose_store(1, Some("   ")), Ok(StoreChoice::InProcess));
        assert!(choose_store(3, Some("")).is_err());
        assert!(choose_store(3, Some("  ")).is_err());
    }

    /// The refusal names BOTH remedies and the number that caused it.
    ///
    /// Asserted because a refusal an operator cannot act on is an outage with
    /// extra steps: the two fixes are opposite (configure a store, or stop
    /// scaling out) and which one is right is a decision only they can make.
    #[test]
    fn the_refusal_names_the_replica_count_and_both_remedies() {
        let msg = choose_store(3, None).expect_err("three replicas, no store");
        assert!(msg.contains('3'), "must name the replica count: {msg}");
        assert!(
            msg.contains("EGRET_DATA_API__SHARED_STORE_URL") || msg.contains("sharedStore"),
            "must name the configure-a-store remedy: {msg}"
        );
        assert!(
            msg.contains("replicas: 1"),
            "must name the scale-back-in remedy: {msg}"
        );
    }

    /// The two key spaces cannot collide.
    ///
    /// A cache key is a 64-character hex digest and a bucket key is
    /// `c:<uuid>:<client id>` — and the client id is caller-supplied. Without
    /// distinct prefixes a client id could in principle be spelled to land on
    /// another key; with them, no string can, because the prefixes differ in
    /// their first characters after a common root.
    #[test]
    fn the_limiter_and_cache_key_spaces_are_disjoint() {
        assert_ne!(RL_PREFIX, RC_PREFIX);
        assert!(!RL_PREFIX.starts_with(RC_PREFIX));
        assert!(!RC_PREFIX.starts_with(RL_PREFIX));
    }
}
