//! Package 4 — the SHARED limiter and response cache, against a real
//! Redis/Valkey.
//!
//! # What this file has to prove, and the three ways it could fail to
//!
//! The package's whole claim is that serving state is no longer per process. A
//! test suite can appear to assert that and assert nothing at all, in three
//! specific ways this file is written against:
//!
//! 1. **A limiter test with ONE caller** cannot see the race a shared store
//!    exists to prevent. Two callers that each read `tokens = 1` and each decide
//!    they may take it produce two admissions from one allowance, and every
//!    individual answer looks correct. So
//!    [`two_shared_stores_cannot_both_take_the_last_token`] drives
//!    `CONCURRENT_CALLERS` tasks across TWO independent [`SharedStore`] handles —
//!    two replicas — and counts. It also measures two [`TokenBucket`]s under the
//!    identical load, which is the defect stated as a number rather than as
//!    prose.
//! 2. **A cache test with ONE principal** cannot see a cross-principal leak,
//!    which is the failure mode that matters far more than fragmentation. So
//!    every cache test here uses two.
//! 3. **A "shared store" test that silently falls back in-process** passes while
//!    measuring nothing. So the store is inspected from an INDEPENDENT redis
//!    connection this crate opens itself: if the code under test had answered
//!    from a `HashMap`, the keys would not be there. That is what
//!    [`the_bucket_and_the_entry_really_live_in_the_store`] asserts, and it is
//!    the assertion the rest of the file rests on.
//!
//! # Not DB-gated; STORE-gated, and a skip is not a pass
//!
//! Nothing here touches Postgres. It needs a Redis/Valkey at
//! `EGRET_TEST_SHARED_STORE_URL`, and with none it **panics** rather than
//! returning early — the same discipline `egret-test-db` applies for the same
//! reason. A suite that self-skips green over the defect it names is how this
//! repo has shipped sixteen of them. `EGRET_ALLOW_STORE_SKIP=1` opts out, in the
//! invocation, where the green and the reason for it are read together.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egret_dataapi::serving::{
    cache_key, CachedResponse, Rate, RateLimitStore, Refusal, ResponseCache, ResponseStore,
    SharedStore, TokenBucket,
};
use uuid::Uuid;

/// Enough concurrency that a read-modify-write across a network is virtually
/// certain to interleave. A smaller number can pass against a racy
/// implementation by luck, which is the same as not testing it.
const CONCURRENT_CALLERS: usize = 40;

const URL_VAR: &str = "EGRET_TEST_SHARED_STORE_URL";
const ALLOW_SKIP_VAR: &str = "EGRET_ALLOW_STORE_SKIP";

/// The store URL, or a panic naming both remedies.
///
/// `None` means "the operator asked to skip", which is only reachable with
/// [`ALLOW_SKIP_VAR`] set. Everything else panics: a shared-store suite that
/// quietly reports `ok` with no store is precisely the shape this package was
/// written to distrust.
fn store_url() -> Option<String> {
    match std::env::var(URL_VAR) {
        Ok(u) if !u.trim().is_empty() => Some(u),
        _ => {
            if std::env::var(ALLOW_SKIP_VAR).is_ok() {
                eprintln!("SKIPPED, asserted nothing — no {URL_VAR}, {ALLOW_SKIP_VAR} is set");
                return None;
            }
            panic!(
                "{URL_VAR} is not set, so this suite cannot exercise the shared store at all.\n\
                 \n\
                 Start one and point at it:\n\
                 \x20     docker run -d --name egret-test-valkey -p 6399:6379 valkey/valkey:8\n\
                 \x20     {URL_VAR}=redis://127.0.0.1:6399 cargo test -p egret-dataapi\n\
                 \n\
                 To run the rest of the crate's suites without one, say so in the command:\n\
                 \x20     {ALLOW_SKIP_VAR}=1 cargo test -p egret-dataapi"
            )
        }
    }
}

macro_rules! skip_without_store {
    () => {
        match store_url() {
            Some(u) => u,
            None => return,
        }
    };
}

/// An independent connection this crate opens itself, used to look at what the
/// code under test actually wrote.
///
/// Deliberately NOT the `SharedStore`: asking the implementation whether it
/// stored something is asking it to confirm its own claim, and would pass just
/// as well against an in-process map.
async fn peek(url: &str) -> redis::aio::MultiplexedConnection {
    redis::Client::open(url)
        .expect("open")
        .get_multiplexed_async_connection()
        .await
        .expect("connect")
}

/// A key nothing else in this run can touch, so the suite is safe to run in
/// parallel and against a store that already holds keys.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn body(tag: &str) -> Arc<serde_json::Value> {
    Arc::new(serde_json::json!({ "rows": [{ "who": tag }] }))
}

// ---------------------------------------------------------------------------
// 1. The race the shared store exists to prevent.
// ---------------------------------------------------------------------------

/// Two replicas, forty concurrent callers, one token — and exactly one
/// admission.
///
/// # Why two STORES and not one
///
/// One `SharedStore` driven concurrently would prove the script is atomic
/// against itself, which is a weaker claim than the one being made: the defect
/// is two *processes*, each with its own connection and its own view. Two
/// handles opened separately is the closest a single test process gets to that,
/// and it is what makes the in-process comparison below meaningful — two
/// `TokenBucket`s are two processes' worth of state in exactly the same shape.
///
/// # The defect, measured
///
/// The second half runs the identical load against two `TokenBucket`s and
/// asserts they admit **twice** the allowance. That is Package 4's premise
/// ("scaling out silently multiplies every published rate limit by the replica
/// count") stated as a measurement in the same test that shows it fixed, so the
/// fix cannot be believed without the defect being reproduced first.
///
/// # Mutation
///
/// Measured, not predicted. Replacing the scripted bucket with the identical
/// arithmetic done as three round trips (`HMGET`, then `HSET`, then `PEXPIRE`)
/// makes this fail with **40 admitted where 1 was allowed** — every caller
/// interleaves between the read and the write and every one of them takes the
/// same last token. That is the failure mode in its worst form: a limiter that
/// admits everything precisely when it is under load.
#[tokio::test]
async fn two_shared_stores_cannot_both_take_the_last_token() {
    let url = skip_without_store!();
    let key = unique("race");
    // burst = 1: exactly one token exists, and at a rate this low nothing
    // refills within the test.
    let rate = Rate {
        qps: 0.001,
        burst: 1.0,
    };

    let a = Arc::new(SharedStore::connect(&url).await.expect("store a"));
    let b = Arc::new(SharedStore::connect(&url).await.expect("store b"));
    // ONE `now` for every caller, so no admission can be explained by refill.
    let now = Instant::now();

    let admitted = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for i in 0..CONCURRENT_CALLERS {
        let store: Arc<dyn RateLimitStore> = if i % 2 == 0 { a.clone() } else { b.clone() };
        let key = key.clone();
        let admitted = admitted.clone();
        tasks.push(tokio::spawn(async move {
            if store.check_at(&key, rate, now).await.is_ok() {
                admitted.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for t in tasks {
        t.await.expect("join");
    }

    assert_eq!(
        admitted.load(Ordering::SeqCst),
        1,
        "{CONCURRENT_CALLERS} concurrent callers across two replicas took more than the one \
         token the bucket held — the read-modify-write raced, which is the exact over-admission \
         a shared limiter exists to prevent"
    );

    // The same load against the state Package 4 replaces. Two processes, two
    // maps, two full buckets: the published allowance of 1 admits 2.
    let (ta, tb) = (Arc::new(TokenBucket::new()), Arc::new(TokenBucket::new()));
    let in_process = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for i in 0..CONCURRENT_CALLERS {
        let store: Arc<dyn RateLimitStore> = if i % 2 == 0 { ta.clone() } else { tb.clone() };
        let key = key.clone();
        let counted = in_process.clone();
        tasks.push(tokio::spawn(async move {
            if store.check_at(&key, rate, now).await.is_ok() {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for t in tasks {
        t.await.expect("join");
    }
    assert_eq!(
        in_process.load(Ordering::SeqCst),
        2,
        "the premise of this package: N process-local limiters admit N times the published \
         allowance. If this is no longer 2, the comparison above has stopped meaning anything"
    );
}

// ---------------------------------------------------------------------------
// 2. Proof that the shared path — not a fallback — is what ran.
// ---------------------------------------------------------------------------

/// After a check and a put, an INDEPENDENT connection finds both in the store.
///
/// This is the assertion every other test in this file leans on. Without it a
/// `SharedStore` that had quietly answered from process memory would satisfy
/// all of them, and the package would be unmeasured.
///
/// It also pins the cache key: the redis key is `<prefix><cache_key digest>`,
/// the digest verbatim. The digest is the length-prefixed stream with the
/// principal trailing, whose collision proof is the cross-principal safety
/// argument — a shared store that re-derived, shortened or re-hashed it would
/// be a cache that leaks across principals, which is far worse than one that
/// fragments.
///
/// # Mutation
///
/// Point `SharedStore` at an in-process map (or drop the `SET`) and this fails
/// on the `EXISTS`/`KEYS` assertions, naming which half went missing.
#[tokio::test]
async fn the_bucket_and_the_entry_really_live_in_the_store() {
    use redis::AsyncCommands;
    let url = skip_without_store!();
    let mut peek = peek(&url).await;

    let store = SharedStore::connect(&url).await.expect("store");
    let now = Instant::now();

    let bucket_key = unique("witness");
    RateLimitStore::check_at(
        &store,
        &bucket_key,
        Rate {
            qps: 1.0,
            burst: 4.0,
        },
        now,
    )
    .await
    .expect("admitted");

    // `KEYS` over the unique suffix rather than a hard-coded prefix: the test
    // asserts the bucket is IN the store, not what the module chose to call the
    // namespace.
    let found: Vec<String> = peek.keys(format!("*{bucket_key}")).await.expect("keys");
    assert_eq!(
        found.len(),
        1,
        "the token bucket is not in the store, so the limiter answered from somewhere else \
         and every other assertion in this file is vacuous"
    );
    let tokens: Option<String> = peek.hget(&found[0], "t").await.expect("hget");
    let tokens: f64 = tokens
        .expect("the bucket carries a token count")
        .parse()
        .unwrap();
    assert!(
        (tokens - 3.0).abs() < 1e-6,
        "one of four tokens should have been taken, found {tokens} left"
    );

    // The cache half, and the key it is stored under.
    let endpoint = Uuid::new_v4();
    let principal = egret_core::UserId(Uuid::new_v4());
    let digest = cache_key(endpoint, 1, &Default::default(), Some(principal));
    assert!(
        ResponseStore::put_at(
            &store,
            digest.clone(),
            body("a"),
            vec![1],
            Duration::from_secs(60),
            now
        )
        .await,
        "a small body must be stored"
    );
    let found: Vec<String> = peek.keys(format!("*{digest}")).await.expect("keys");
    assert_eq!(
        found.len(),
        1,
        "the cached response is not in the store under `cache_key`'s digest verbatim"
    );
    assert!(
        found[0].ends_with(&digest),
        "the store must key on the digest UNCHANGED — the principal-trailing, length-prefixed \
         construction is the whole cross-principal argument: {}",
        found[0]
    );
    // Written together with its expiry, so no entry can exist without a TTL and
    // be served forever.
    let ttl: i64 = peek.pttl(&found[0]).await.expect("pttl");
    assert!(
        ttl > 0 && ttl <= 60_000,
        "the entry must carry the TTL it was written with, got {ttl} ms"
    );
}

// ---------------------------------------------------------------------------
// 3. One bucket and one cache across replicas — the fragmentation fix.
// ---------------------------------------------------------------------------

/// Two replicas share ONE bucket, and it refills on the caller's clock.
///
/// The first half is the fix: a client that exhausted its allowance on replica A
/// is refused by replica B. The second is what keeps the shared implementation
/// honest against the in-process one — the injected `now` still drives refill,
/// so a test never sleeps and a `SharedStore` cannot quietly become
/// wall-clock-only.
#[tokio::test]
async fn one_bucket_is_shared_across_replicas_and_still_refills_on_the_injected_clock() {
    let url = skip_without_store!();
    let a = SharedStore::connect(&url).await.expect("a");
    let b = SharedStore::connect(&url).await.expect("b");
    let key = unique("shared-bucket");
    let rate = Rate {
        qps: 1.0,
        burst: 2.0,
    };
    let t = Instant::now();

    assert!(RateLimitStore::check_at(&a, &key, rate, t).await.is_ok());
    assert!(
        RateLimitStore::check_at(&b, &key, rate, t).await.is_ok(),
        "the second token is the same bucket's, not a second bucket's"
    );
    match RateLimitStore::check_at(&b, &key, rate, t).await {
        Err(Refusal::Exhausted(d)) => assert!(
            d > Duration::ZERO && d <= Duration::from_secs(1),
            "a 1 qps bucket refills within a second, got {d:?}"
        ),
        other => {
            panic!("two replicas must not admit three requests from a two-token bucket: {other:?}")
        }
    }

    // Two seconds, not the one that refills exactly one token.
    //
    // The two stores anchored their `Instant`→wall-clock conversion at two
    // different moments, so `a`'s and `b`'s idea of the same `Instant` differ by
    // the sub-millisecond gap between their constructions (in production, by
    // their NTP skew — see `SharedStore::anchor_instant`). Asserting on the
    // exact one-token boundary across two anchors measures that skew rather than
    // the refill: with `b` a millisecond ahead, the bucket lands at 0.9999… and
    // the call is refused for a reason that has nothing to do with the rule.
    // The claim being made is "an idle bucket refills and the caller's clock is
    // what says so", and two seconds asserts exactly that with no boundary in
    // it. The in-process bucket, which has one clock, keeps the exact-boundary
    // assertions in `serving/ratelimit.rs`.
    assert!(
        RateLimitStore::check_at(&a, &key, rate, t + Duration::from_secs(2))
            .await
            .is_ok(),
        "an idle bucket must refill on the clock the CALLER passes"
    );
}

/// A response cached by one replica is served by the other.
///
/// The cache half of the same fix. Fragmentation is the mild defect — it is
/// slower and never wrong — but it is the reason the cache is here at all, and a
/// package that shared only the limiter would leave every replica re-executing.
#[tokio::test]
async fn a_response_cached_by_one_replica_is_served_by_another() {
    let url = skip_without_store!();
    let a = SharedStore::connect(&url).await.expect("a");
    let b = SharedStore::connect(&url).await.expect("b");
    let key = unique("entry");
    let now = Instant::now();

    assert!(
        ResponseStore::get_at(&b, &key, now).await.is_none(),
        "nothing is cached yet"
    );
    assert!(
        ResponseStore::put_at(
            &a,
            key.clone(),
            body("from-a"),
            vec![7, 9],
            Duration::from_secs(60),
            now
        )
        .await
    );

    let hit = ResponseStore::get_at(&b, &key, now)
        .await
        .expect("the other replica must see it");
    assert_eq!(*hit.data, *body("from-a"));
    assert_eq!(
        hit.row_counts,
        vec![7, 9],
        "the PER-STATEMENT counts must survive the round trip: a hit runs no statements, so a \
         single total could not rebuild `meta`"
    );
}

// ---------------------------------------------------------------------------
// 4. Two principals — the failure that matters more than fragmentation.
// ---------------------------------------------------------------------------

/// Two identities, the same endpoint, the same parameters, two entries — and
/// neither can reach the other's.
///
/// A shared cache makes a cross-principal leak worse than a process-local one
/// did: with one map per replica a leak was at least confined to the replica
/// that cached it, and with one store it is fleet-wide and lasts the whole TTL.
/// So this is asserted through the SHARED store specifically, with the real
/// [`cache_key`], and the fixture has two principals because one cannot see the
/// failure at all.
///
/// # Mutation
///
/// Drop the principal from `cache_key`'s hashed stream and this fails on the
/// first assertion: both identities compute the same digest and B is served A's
/// rows.
#[tokio::test]
async fn two_principals_get_two_entries_and_neither_sees_the_others_rows() {
    let url = skip_without_store!();
    let store = SharedStore::connect(&url).await.expect("store");
    let now = Instant::now();

    let endpoint = Uuid::new_v4();
    let a = egret_core::UserId(Uuid::new_v4());
    let b = egret_core::UserId(Uuid::new_v4());
    let mut params = std::collections::BTreeMap::new();
    params.insert("tenant".to_string(), vec!["acme".to_string()]);

    let key_a = cache_key(endpoint, 1, &params, Some(a));
    let key_b = cache_key(endpoint, 1, &params, Some(b));
    assert_ne!(
        key_a, key_b,
        "same endpoint, same version, same parameters — the ONLY difference is the identity, \
         and it must be the whole difference in the key"
    );

    assert!(
        ResponseStore::put_at(
            &store,
            key_a.clone(),
            body("a-rows"),
            vec![1],
            Duration::from_secs(60),
            now
        )
        .await
    );

    assert!(
        ResponseStore::get_at(&store, &key_b, now).await.is_none(),
        "principal B must MISS on principal A's warm entry; Ranger filters rows per principal, \
         so serving A's rows to B is a leak through the exact control the design leans on"
    );

    assert!(
        ResponseStore::put_at(
            &store,
            key_b.clone(),
            body("b-rows"),
            vec![1],
            Duration::from_secs(60),
            now
        )
        .await
    );
    let got_a = ResponseStore::get_at(&store, &key_a, now).await.expect("a");
    let got_b = ResponseStore::get_at(&store, &key_b, now).await.expect("b");
    assert_eq!(*got_a.data, *body("a-rows"));
    assert_eq!(
        *got_b.data,
        *body("b-rows"),
        "B's own write must not have been overwritten by, or overwrite, A's"
    );
}

/// A `shared`-scope entry (no principal in the key) is a THIRD key space, and
/// neither principal's key reaches it.
///
/// `shared` is legitimate — it is what a non-per-principal connection earns —
/// but only under `shared_cache_allowed`'s live re-check. The property asserted
/// here is the one the key layout guarantees regardless of that check: a
/// principal-less key can never collide with a principal-bearing one.
#[tokio::test]
async fn a_shared_scope_entry_is_its_own_key_space() {
    let url = skip_without_store!();
    let store = SharedStore::connect(&url).await.expect("store");
    let now = Instant::now();
    let endpoint = Uuid::new_v4();
    let a = egret_core::UserId(Uuid::new_v4());
    let params = Default::default();

    let shared = cache_key(endpoint, 1, &params, None);
    let per_principal = cache_key(endpoint, 1, &params, Some(a));
    assert_ne!(shared, per_principal);

    assert!(
        ResponseStore::put_at(
            &store,
            shared.clone(),
            body("shared"),
            vec![1],
            Duration::from_secs(60),
            now
        )
        .await
    );
    assert!(
        ResponseStore::get_at(&store, &per_principal, now)
            .await
            .is_none(),
        "a per-principal request must not pick up a shared-scope entry"
    );
}

// ---------------------------------------------------------------------------
// 5. Bounds.
// ---------------------------------------------------------------------------

/// A body over the maximum entry size is not stored, and nothing is left behind.
///
/// The answer above the bound is `false` — the next call misses and
/// re-executes — which is a performance outcome and never a correctness one.
/// That nothing is left behind is the assertion worth having on its own: a
/// partial write would be an entry that exists and cannot be parsed.
///
/// # Which of the two checks is the bound, measured rather than assumed
///
/// `put_at` checks the size twice — a cheap structural estimate before
/// serialising, then the exact serialised length. A first version of this test
/// used one enormous string, and it passed with the SERIALISED check deleted,
/// because the estimate caught it first: it was measuring one thing and claiming
/// two. Mutating the other way showed the real relationship — deleting the
/// estimate changes no outcome at all, because anything it refuses the
/// serialised length refuses too.
///
/// So the estimate is a cost guard with no behavioural consequence and gets no
/// assertion (it can have none), and the bound that actually decides is the
/// serialised length. Body (b) below is built to prove exactly that: it slips
/// past the estimate and must still be refused. `estimate_bytes` scores every
/// JSON number as 16 bytes and a float near the top of the exponent range
/// serialises to about 20, so 240 000 of them estimate at ~3.8 MiB (under the
/// 4 MiB bound) and serialise to ~4.8 MiB (over it).
#[tokio::test]
async fn a_response_over_the_maximum_entry_size_is_not_stored() {
    use redis::AsyncCommands;
    let url = skip_without_store!();
    let mut peek = peek(&url).await;
    let store = SharedStore::connect(&url).await.expect("store");

    // (a) The ordinary case: one long string, over the bound by any measure.
    //     Refused before it is serialised, so the work is bounded too.
    let key = unique("huge-estimate");
    let huge = Arc::new(serde_json::json!({ "rows": "x".repeat(5 * 1024 * 1024) }));
    assert!(
        !ResponseStore::put_at(
            &store,
            key.clone(),
            huge,
            vec![1],
            Duration::from_secs(60),
            Instant::now()
        )
        .await,
        "a body over the entry-size bound must be declined"
    );
    let found: Vec<String> = peek.keys(format!("*{key}")).await.expect("keys");
    assert!(
        found.is_empty(),
        "declining must leave NOTHING behind: {found:?}"
    );
    assert!(
        ResponseStore::get_at(&store, &key, Instant::now())
            .await
            .is_none(),
        "and the next request must miss"
    );

    // (b) Under the estimate, over the serialised length — so ONLY the bound
    //     that actually decides can refuse it.
    let key = unique("huge-serialised");
    let numbers: Vec<serde_json::Value> = (0..240_000)
        .map(|_| serde_json::json!(1.2345678901234e300f64))
        .collect();
    let sneaky = Arc::new(serde_json::json!({ "rows": numbers }));
    assert!(
        egret_dataapi::serving::cache_entry_estimate(&sneaky) < 4 * 1024 * 1024,
        "this body must pass the cheap estimate, or it is not testing the second bound"
    );
    assert!(
        serde_json::to_vec(&*sneaky).unwrap().len() > 4 * 1024 * 1024,
        "and it must be over the bound once serialised"
    );
    assert!(
        !ResponseStore::put_at(
            &store,
            key.clone(),
            sneaky,
            vec![1],
            Duration::from_secs(60),
            Instant::now()
        )
        .await,
        "a body the ESTIMATE lets through must still be refused on its serialised length — \
         that is what stops an entry larger than the bound from reaching the store"
    );
    let found: Vec<String> = peek.keys(format!("*{key}")).await.expect("keys");
    assert!(
        found.is_empty(),
        "and nothing may be left behind: {found:?}"
    );
}

/// The bucket key expires once it has refilled, and not before.
///
/// The in-process map prunes on a fixed `IDLE_SECS`, which can drop a bucket
/// that still holds information — a documented caveat there. The shared store
/// sets the expiry from the bucket's OWN refill time, so a dropped key carries
/// nothing. This asserts the TTL is sized from `burst / qps` rather than from a
/// constant, because a constant is how that caveat would come back.
#[tokio::test]
async fn a_bucket_expires_only_once_it_would_have_refilled() {
    use redis::AsyncCommands;
    let url = skip_without_store!();
    let mut peek = peek(&url).await;
    let store = SharedStore::connect(&url).await.expect("store");

    // 10 tokens at 1 qps: ten seconds to refill from empty.
    let key = unique("ttl");
    RateLimitStore::check_at(
        &store,
        &key,
        Rate {
            qps: 1.0,
            burst: 10.0,
        },
        Instant::now(),
    )
    .await
    .expect("admitted");
    let found: Vec<String> = peek.keys(format!("*{key}")).await.expect("keys");
    let ttl: i64 = peek.pttl(&found[0]).await.expect("pttl");
    assert!(
        (10_000..=12_000).contains(&ttl),
        "a 10-token, 1 qps bucket must live about the 10 s it takes to refill, got {ttl} ms"
    );

    // A bucket that refills in a second must not inherit the same lifetime.
    let fast = unique("ttl-fast");
    RateLimitStore::check_at(
        &store,
        &fast,
        Rate {
            qps: 100.0,
            burst: 10.0,
        },
        Instant::now(),
    )
    .await
    .expect("admitted");
    let found: Vec<String> = peek.keys(format!("*{fast}")).await.expect("keys");
    let fast_ttl: i64 = peek.pttl(&found[0]).await.expect("pttl");
    assert!(
        fast_ttl < ttl,
        "the TTL must come from burst/qps, not from a constant: {fast_ttl} ms vs {ttl} ms"
    );
}

// ---------------------------------------------------------------------------
// 6. The two failure policies, against a store that really is unreachable.
// ---------------------------------------------------------------------------

/// A TCP proxy in front of the store, which the test can cut.
///
/// Needed because both failure policies are about a store that stops answering
/// AFTER the process connected: `SharedStore::connect` failing is a different
/// (and deliberately fatal) path. Killing a proxy is the only way to produce the
/// real condition without asserting against a mock — and a mock would be
/// asserting that a function was called, which is not the claim.
struct CuttableProxy {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl CuttableProxy {
    async fn start(upstream: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().unwrap().port();
        let host_port = upstream
            .trim_start_matches("redis://")
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let task = tokio::spawn(async move {
            // A `JoinSet` and NOT `tokio::spawn`, which is the difference
            // between this proxy cutting the connection and not: a detached
            // task outlives the abort of the task that spawned it, so the
            // already-established socket kept forwarding and the store stayed
            // perfectly healthy. Dropping a `JoinSet` aborts everything in it,
            // so aborting the accept loop really does close every pump.
            let mut pumps = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((mut down, _)) = accepted else { return };
                        let host_port = host_port.clone();
                        pumps.spawn(async move {
                            let Ok(mut up) = tokio::net::TcpStream::connect(&host_port).await
                            else {
                                return;
                            };
                            let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
                        });
                    }
                    _ = pumps.join_next(), if !pumps.is_empty() => {}
                }
            }
        });
        Self { port, task }
    }

    fn url(&self) -> String {
        format!("redis://127.0.0.1:{}", self.port)
    }

    /// Stop accepting and drop every forwarded connection. Reconnection attempts
    /// then get a refused connection, which is what a store outage looks like.
    fn cut(self) {
        self.task.abort();
    }
}

/// With the store gone, the LIMITER refuses and the CACHE misses.
///
/// Both halves in one test because the point is the contrast, and a reader
/// reaching for "make them consistent" should see the two answers side by side.
///
/// * The limiter fails **closed** — [`Refusal::Unavailable`], which the router
///   answers `503 rate_limiter_unavailable`. Admitting instead would reinstate
///   the multiplied-limit defect this package removes, unbounded, at the one
///   moment nobody is watching.
/// * The cache fails **open** — a miss, which re-executes and re-shapes. It is
///   an optimisation, never a control.
///
/// # Mutation
///
/// Change the limiter's error arm to `Ok(())` and the first assertion fails,
/// naming the admission. Change the cache's to a panic or an error and the
/// second does.
#[tokio::test]
async fn an_unreachable_store_refuses_a_limit_check_and_merely_misses_the_cache() {
    let url = skip_without_store!();
    let proxy = CuttableProxy::start(url).await;
    let store = SharedStore::connect(&proxy.url())
        .await
        .expect("connects while the proxy is up");
    let now = Instant::now();
    let key = unique("outage");
    let rate = Rate {
        qps: 100.0,
        burst: 100.0,
    };

    // Healthy first, so a later refusal cannot be explained by the store having
    // never worked.
    RateLimitStore::check_at(&store, &key, rate, now)
        .await
        .expect("admitted while the store is reachable");
    assert!(
        ResponseStore::put_at(
            &store,
            key.clone(),
            body("warm"),
            vec![1],
            Duration::from_secs(60),
            now
        )
        .await,
        "stored while the store is reachable"
    );

    proxy.cut();
    // Let the abort take effect and the sockets close.
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        RateLimitStore::check_at(&store, &key, rate, now).await,
        Err(Refusal::Unavailable),
        "with the store unreachable the limiter must REFUSE. Admitting would mean an unbounded, \
         uncounted allowance under exactly the condition nobody is watching — the defect this \
         package exists to remove, arrived at by another road"
    );

    assert!(
        ResponseStore::get_at(&store, &key, now).await.is_none(),
        "with the store unreachable the cache must MISS. It is an optimisation, and a miss \
         re-executes the query and re-shapes it against the frozen contract"
    );
    assert!(
        !ResponseStore::put_at(
            &store,
            unique("outage-write"),
            body("x"),
            vec![1],
            Duration::from_secs(60),
            now
        )
        .await,
        "a write that could not happen must say so rather than claim success"
    );
}

// ---------------------------------------------------------------------------
// 7. The two implementations must not drift apart.
// ---------------------------------------------------------------------------

/// The shared bucket and the in-process bucket answer a single caller's
/// sequence identically.
///
/// The Lua script is a second implementation of arithmetic that already exists
/// in Rust, and two implementations of one rule drift. This drives the same
/// sequence through both and compares admissions — including the first-sighting
/// rule (a new key starts FULL, so a client's first request is never refused)
/// and the clamp (an idle hour is worth `burst`, not an hour's tokens).
#[tokio::test]
async fn shared_and_in_process_buckets_agree_on_a_single_callers_sequence() {
    let url = skip_without_store!();
    let shared = SharedStore::connect(&url).await.expect("store");
    let local = TokenBucket::new();
    let key = unique("agree");
    let rate = Rate {
        qps: 2.0,
        burst: 3.0,
    };
    let t = Instant::now();

    // Offsets chosen to hit each rule: the initial full bucket, exhaustion,
    // a partial refill, and an idle hour that must NOT be worth 7200 tokens.
    let offsets = [
        0, 0, 0, 0,   // three admitted from a full bucket, the fourth refused
        500, // half a second at 2 qps is one token
        500, // no further refill
        3_600_000, 3_600_000, 3_600_000, 3_600_000, // an hour: burst, not 7200
    ];
    for (i, ms) in offsets.iter().enumerate() {
        let at = t + Duration::from_millis(*ms);
        let s = RateLimitStore::check_at(&shared, &key, rate, at)
            .await
            .is_ok();
        let l = RateLimitStore::check_at(&local, &key, rate, at)
            .await
            .is_ok();
        assert_eq!(
            s, l,
            "step {i} (+{ms} ms): the Lua bucket and the Rust bucket disagree — shared={s}, \
             in-process={l}. Two implementations of one rule have drifted"
        );
    }
}

/// The in-process cache and the shared cache answer the same reads the same way.
///
/// Not expiry — that is the one question the two answer differently on purpose
/// (the shared store's expiry is Redis' own `PX` and not a comparison against
/// the injected `now`), and it is documented on `ResponseStore`. What must agree
/// is hit, miss, the round-tripped body, and the per-statement counts.
#[tokio::test]
async fn shared_and_in_process_caches_agree_on_hit_miss_and_payload() {
    let url = skip_without_store!();
    let shared = SharedStore::connect(&url).await.expect("store");
    let local = ResponseCache::new();
    let now = Instant::now();
    let key = unique("cache-agree");
    let absent = unique("cache-absent");

    for store in [&shared as &dyn ResponseStore, &local as &dyn ResponseStore] {
        assert!(store.get_at(&absent, now).await.is_none());
        assert!(
            store
                .put_at(
                    key.clone(),
                    body("payload"),
                    vec![3, 4],
                    Duration::from_secs(60),
                    now
                )
                .await
        );
        let CachedResponse { data, row_counts } =
            store.get_at(&key, now).await.expect("a warm entry");
        assert_eq!(*data, *body("payload"));
        assert_eq!(row_counts, vec![3, 4]);
    }
}

// ---------------------------------------------------------------------------
// 8. The clocks. Two replicas do not share one, and the bucket must survive it.
// ---------------------------------------------------------------------------

/// How far apart two replicas' clocks are taken to be. Absurd for NTP, chosen
/// on purpose: the point of the fix is that the leak is bounded by `burst`
/// **regardless of the magnitude**, so a small number would let a bounded-leak
/// implementation and an unbounded one look alike.
const CLOCK_SKEW: Duration = Duration::from_millis(500);

/// Two replicas whose clocks disagree cannot mint tokens.
///
/// # The defect this closes, measured before the fix
///
/// `now` is supplied by the caller, so N replicas send N slightly different
/// numbers for one real moment. The first version of the Lua bucket wrote
/// `l = now` unconditionally, which meant a lagging replica REWOUND the stored
/// timestamp and the leading replica then re-billed an interval it had already
/// been charged for. Every alternation minted tokens and nothing bounded it.
/// Driving the shipped script directly on Valkey 8 at `qps = 10, burst = 10`,
/// 400 calls at one real instant:
///
/// | skew | admitted, before | admitted, after |
/// |---|---|---|
/// | 0 ms | 10 | 10 |
/// | 5 ms | 20 | 10 |
/// | 50 ms | 110 | 10 |
/// | 500 ms | **400 — the limiter is gone** | 11 |
/// | 1 hour | **400** | 11 |
///
/// This test is that measurement through the real `check_at`, with two real
/// `SharedStore` handles. Skew is applied as the `now` each replica reports for
/// the same real moment, which is the only place skew is observable at all: the
/// script cannot tell a clock that is behind from a caller that passed an
/// earlier instant, and does not need to.
///
/// # Why this could not have been caught by the tests that were here
///
/// Every other bucket test in this file passes ONE clock reading to every
/// handle. Under one clock the defect is invisible — the stored timestamp never
/// goes backwards, so nothing is ever re-billed. A skew test that reuses a
/// single `now` proves nothing, which is exactly how this survived the first
/// round.
///
/// # The two-sided bound
///
/// `>= burst` because a limiter that admitted nothing would sail past a
/// one-sided ceiling; `<= burst + 1` because the envelope's ONE-TIME forward
/// jump can refill the bucket once and `tokens > burst` clamps that to the
/// published allowance. There is no third possibility that is not a leak.
#[tokio::test]
async fn two_replicas_whose_clocks_disagree_cannot_mint_tokens() {
    use redis::AsyncCommands;
    let url = skip_without_store!();
    let behind = SharedStore::connect(&url)
        .await
        .expect("the lagging replica");
    let ahead = SharedStore::connect(&url)
        .await
        .expect("the leading replica");
    let mut peek = peek(&url).await;
    let key = unique("skew");
    let rate = Rate {
        qps: 10.0,
        burst: 10.0,
    };

    // ONE real instant, so no admission can be explained by refill: the only
    // time that passes in this loop is the difference between the two clocks.
    let t = Instant::now();
    let mut admitted = 0usize;
    for _ in 0..200 {
        if RateLimitStore::check_at(&behind, &key, rate, t)
            .await
            .is_ok()
        {
            admitted += 1;
        }
        if RateLimitStore::check_at(&ahead, &key, rate, t + CLOCK_SKEW)
            .await
            .is_ok()
        {
            admitted += 1;
        }
    }
    assert!(
        admitted >= rate.burst as usize,
        "a 10-token bucket must still admit its burst; {admitted} admissions means the \
         clock guard refuses traffic the author published for"
    );
    assert!(
        admitted <= rate.burst as usize + 1,
        "{admitted} of 400 calls admitted from a {}-token bucket at ONE real instant. \
         The clocks of two replicas are being turned into tokens: each alternation \
         rewinds the stored timestamp and lets the other replica re-bill the same \
         interval. Bounded by nothing — at {CLOCK_SKEW:?} of skew the published limit \
         does not exist",
        rate.burst
    );

    // And the MECHANISM, not only the count: the stored timestamp is a monotone
    // envelope, so a call from the replica that is behind cannot move it back.
    let found: Vec<String> = peek.keys(format!("*{key}")).await.expect("keys");
    let before: f64 = peek.hget(&found[0], "l").await.expect("l before");
    RateLimitStore::check_at(&behind, &key, rate, t)
        .await
        .expect_err("the bucket is empty by now");
    let after: f64 = peek.hget(&found[0], "l").await.expect("l after");
    assert_eq!(
        before, after,
        "a call from the lagging replica moved the stored timestamp from {before} to \
         {after}. Every millisecond it rewinds is a millisecond the leading replica is \
         charged for twice"
    );
}

/// A rate of zero refuses; it does not admit, and it does not panic.
///
/// Unreachable through `Rate::from_spec`, which answers `None` for a zero rate
/// — and reachable through `Rate`'s public fields, which is the whole reason
/// this is asserted rather than assumed away. The two implementations used to
/// fail differently and both badly: the Lua bucket computed `(1 - tokens) / 0`,
/// i.e. `inf`, whose conversion to a Redis integer reply is undefined and can
/// arrive as `0` — which this caller reads as **admitted**, from a rate of zero
/// per second; and `TokenBucket` handed the same `inf` to
/// `Duration::from_secs_f64`, which **panics**.
///
/// Both now answer a bounded refusal. The exact wait is not asserted (they pick
/// it differently, and neither number means anything beyond "not soon"); that
/// it is a refusal at all is the claim.
#[tokio::test]
async fn a_rate_of_zero_refuses_on_both_implementations() {
    let url = skip_without_store!();
    let shared = SharedStore::connect(&url).await.expect("store");
    let local = TokenBucket::new();
    let rate = Rate {
        qps: 0.0,
        burst: 1.0,
    };
    let now = Instant::now();

    for (name, store) in [
        ("shared", &shared as &dyn RateLimitStore),
        ("in-process", &local as &dyn RateLimitStore),
    ] {
        let key = unique("zero-qps");
        // The first sighting is a FULL bucket, so one token exists even here.
        store
            .check_at(&key, rate, now)
            .await
            .unwrap_or_else(|e| panic!("{name}: a full bucket must admit the first call: {e:?}"));
        match store.check_at(&key, rate, now).await {
            Err(Refusal::Exhausted(d)) => assert!(
                d > Duration::ZERO,
                "{name}: a bucket that never refills must not answer a zero wait"
            ),
            other => panic!(
                "{name}: a rate of zero per second must REFUSE the second call, got {other:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 9. The one place the two caches do NOT agree, held as a witness.
// ---------------------------------------------------------------------------

/// The 4 MiB entry bound is the same constant on both sides and **not the same
/// quantity**, so one body shape changes its cacheability when a deployment
/// scales out.
///
/// `ResponseCache::put_at` bounds `cache_entry_estimate`; `SharedStore::put_at`
/// bounds the SERIALISED length. The estimate charges every JSON number a flat
/// 16 bytes and a full-precision `f64` prints as more — so a float-heavy body
/// can estimate under the bound and serialise over it.
///
/// This test is the witness for the paragraph in `SharedStore::put_at` that
/// says so. It exists because that paragraph used to claim the opposite — "the
/// same 4 MiB the in-process cache applies, so an endpoint does not start or
/// stop being cacheable when a deployment scales out" — and prose is not an
/// instrument. Should someone make the two agree, this reddens and the
/// paragraph gets rewritten with it, which is the point.
///
/// It is not a defect being pinned open. The direction is the safe one: the
/// shared store refuses to hold what it cannot actually afford, the caller
/// misses, and a miss re-executes and re-shapes against the frozen contract.
#[tokio::test]
async fn a_float_heavy_body_caches_in_process_and_not_in_the_store() {
    let url = skip_without_store!();
    let shared = SharedStore::connect(&url).await.expect("store");
    let local = ResponseCache::new();
    let now = Instant::now();
    let key = unique("float-heavy");

    // 220 000 full-precision f64s: 16 bytes each to the estimate (3.4 MiB, under
    // the bound) and 24 characters each on the wire (5.0 MiB, over it).
    let data = Arc::new(serde_json::json!({
        "rows": (0..220_000)
            .map(|i| serde_json::json!(-1.234_567_890_123_456_7e-300 * (1.0 + i as f64)))
            .collect::<Vec<_>>()
    }));
    let estimate = egret_dataapi::serving::cache_entry_estimate(&data);
    let serialised = serde_json::to_vec(&*data).expect("serialise").len();
    const BOUND: usize = 4 * 1024 * 1024;
    assert!(
        estimate < BOUND && serialised > BOUND,
        "the witness must actually straddle the bound, or this test asserts nothing: \
         estimate {estimate}, serialised {serialised}, bound {BOUND}"
    );

    assert!(
        local.put_at(
            key.clone(),
            Arc::clone(&data),
            vec![220_000],
            Duration::from_secs(60),
            now
        ),
        "the in-process cache bounds the ESTIMATE ({estimate}), which is under {BOUND}"
    );
    assert!(
        local.get_at(&key, now).is_some(),
        "and it really is retrievable, or the `true` above means nothing"
    );

    assert!(
        !ResponseStore::put_at(
            &shared,
            key.clone(),
            Arc::clone(&data),
            vec![220_000],
            Duration::from_secs(60),
            now
        )
        .await,
        "the shared store bounds the SERIALISED length ({serialised}), which is over \
         {BOUND}, so it must refuse this body"
    );
    assert!(
        ResponseStore::get_at(&shared, &key, now).await.is_none(),
        "a refused put must leave nothing behind"
    );
}
