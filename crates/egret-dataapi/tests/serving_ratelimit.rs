//! P4 Task 4 — rate limiting, through the serving surface.
//!
//! The bucket's own semantics (refill, burst, `Retry-After`, the two keys) are
//! unit-tested with an INJECTED clock in `serving/ratelimit.rs`, where two time
//! points are constructed explicitly and can never coincide. This file asserts
//! the other half, which a unit test cannot see: that the handler applies both
//! published limits, keys them on the identities it claims to, and answers 429
//! with a `Retry-After`.
//!
//! **The clock is frozen.** The handler reads `Instant::now()` itself, so an
//! earlier version of this file raced the wall clock: at `per_key_qps = 1` a
//! call that must be refused becomes a 200 the moment more than a second passes
//! between two `oneshot`s — a loaded CI box, or one slow query, and the suite
//! flakes. A rate-limit test that flakes gets `#[ignore]`d, and an ignored
//! rate-limit test is no rate limit at all. `serving_common::TestClock` wraps
//! the REAL `TokenBucket` and substitutes only the clock, so what is measured is
//! still the shipped implementation, and `f.clock.advance(…)` buys the
//! assertion a wall clock cannot make without sleeping: the refill.
//!
//! DB-gated; self-skips when `DATABASE_URL` is unset — check the DURATION.

mod serving_common;

use axum::http::StatusCode;
use serving_common::fixture;
use std::time::Duration;

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

#[tokio::test]
async fn a_client_over_its_published_limit_gets_429_with_a_retry_after() {
    // 429 rather than a silent slowdown, and `Retry-After` rather than a bare
    // status: a client that is refused without being told when to come back
    // guesses, and a retry loop's guess is "now".
    let f = skip_without_db!();
    f.publish(
        "/rl",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1, "burst": 1 }),
    )
    .await;

    let first = f.call_as(f.principal_a, "rl").await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);

    let second = f.call_as(f.principal_a, "rl").await;
    assert_eq!(
        second.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the second call within the same second must be refused: {}",
        second.body
    );
    let retry = second
        .retry_after
        .as_deref()
        .expect("a 429 must carry Retry-After");
    let secs: u64 = retry.parse().expect("Retry-After is delta-seconds");
    assert!(
        secs >= 1,
        "Retry-After: 0 invites an immediate retry that is refused again, got {secs}"
    );

    // The stated wait is honoured: after it, the SAME client is admitted again.
    // This is what separates a refilling bucket from `egret-auth`'s lockout, and
    // it is unassertable on a wall clock without sleeping for it.
    f.clock.advance(Duration::from_secs(secs));
    let third = f.call_as(f.principal_a, "rl").await;
    assert_eq!(
        third.status,
        StatusCode::OK,
        "the client must be admitted once the bucket has refilled: {}",
        third.body
    );
}

#[tokio::test]
async fn a_refused_client_is_not_locked_out_of_a_later_window() {
    // `egret-auth`'s limiter LOCKS a key for a fixed period once tripped, so a
    // caller that keeps knocking never gets in. Copying its shape without its
    // semantics is the mistake this module exists to avoid, and this is the
    // assertion that would catch a regression to it: knock while refused, then
    // wait, then be admitted.
    let f = skip_without_db!();
    f.publish(
        "/lock",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1, "burst": 1 }),
    )
    .await;

    assert_eq!(
        f.call_as(f.principal_a, "lock").await.status,
        StatusCode::OK
    );
    for _ in 0..3 {
        assert_eq!(
            f.call_as(f.principal_a, "lock").await.status,
            StatusCode::TOO_MANY_REQUESTS,
            "still inside the same second"
        );
    }

    f.clock.advance(Duration::from_secs(1));
    assert_eq!(
        f.call_as(f.principal_a, "lock").await.status,
        StatusCode::OK,
        "three refused attempts must not have extended the wait"
    );
}

#[tokio::test]
async fn one_clients_burst_does_not_exhaust_another_clients_allowance() {
    // The reason `per_key_qps` is keyed on the CLIENT and not folded into one
    // combined bucket. A single combined key makes one noisy client the reason
    // every other client is refused.
    //
    // The two calls run as the same principal on purpose: the only thing that
    // differs is the client id, so nothing but the rate-limit key can explain
    // the outcome.
    let f = skip_without_db!();
    f.publish(
        "/rl",
        f.database_id,
        no_cache(),
        // Generous endpoint-wide allowance, so only the per-client limit can
        // refuse anything here.
        serde_json::json!({ "per_key_qps": 1, "burst": 1, "per_endpoint_qps": 1000 }),
    )
    .await;

    assert_eq!(
        f.call_as_client(f.principal_a, "noisy", "rl").await.status,
        StatusCode::OK
    );
    assert_eq!(
        f.call_as_client(f.principal_a, "noisy", "rl").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    let quiet = f.call_as_client(f.principal_a, "quiet", "rl").await;
    assert_eq!(
        quiet.status,
        StatusCode::OK,
        "a second client must have its own allowance: {}",
        quiet.body
    );
}

#[tokio::test]
async fn the_endpoint_wide_limit_bounds_clients_that_are_each_within_theirs() {
    // The other key. Every client here is comfortably inside its own per-client
    // limit; what runs out is the endpoint's shared allowance. Without this
    // second bucket an endpoint has no ceiling at all as clients are added — and
    // the engine behind it has no `query_max_run_time` equivalent on StarRocks
    // to fall back on.
    let f = skip_without_db!();
    f.publish(
        "/ep",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1000, "burst": 1, "per_endpoint_qps": 1 }),
    )
    .await;

    assert_eq!(
        f.call_as_client(f.principal_a, "one", "ep").await.status,
        StatusCode::OK
    );
    let second = f.call_as_client(f.principal_b, "two", "ep").await;
    assert_eq!(
        second.status,
        StatusCode::TOO_MANY_REQUESTS,
        "a DIFFERENT client, within its own limit, must still hit the \
         endpoint-wide bucket: {}",
        second.body
    );
}

#[tokio::test]
async fn an_endpoint_with_no_published_limit_is_not_limited() {
    // `RateLimitSpec` defaults every field to 0 and nothing validates it, so 0
    // must mean "unconfigured", never "zero requests per second" — the latter
    // would make every endpoint an author never thought about unreachable.
    //
    // This is also the anti-triviality for the file: if the limiter refused
    // everything, every 429 assertion above would pass for the wrong reason.
    let f = skip_without_db!();
    f.publish("/free", f.database_id, no_cache(), serde_json::json!({}))
        .await;

    for i in 0..10 {
        let res = f.call_as_client(f.principal_a, "same-client", "free").await;
        assert_eq!(
            res.status,
            StatusCode::OK,
            "call {i} must not be limited: {}",
            res.body
        );
    }
}

#[tokio::test]
async fn two_endpoints_do_not_share_a_bucket() {
    // Both buckets are keyed on `endpoint_id`, so one endpoint's exhaustion must
    // leave every other endpoint alone — including one published with the same
    // limits by the same client.
    let f = skip_without_db!();
    f.publish(
        "/a",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1, "burst": 1, "per_endpoint_qps": 1 }),
    )
    .await;
    f.publish(
        "/b",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1, "burst": 1, "per_endpoint_qps": 1 }),
    )
    .await;

    assert_eq!(
        f.call_as_client(f.principal_a, "c", "a").await.status,
        StatusCode::OK
    );
    assert_eq!(
        f.call_as_client(f.principal_a, "c", "a").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    let other = f.call_as_client(f.principal_a, "c", "b").await;
    assert_eq!(
        other.status,
        StatusCode::OK,
        "a second endpoint must have its own buckets: {}",
        other.body
    );
}

#[tokio::test]
async fn the_limit_is_not_doubled_by_re_spelling_the_path() {
    // Keying on the raw wildcard capture would give one endpoint two buckets:
    // axum percent-decodes the capture, so `%2F` and `/` reach the same endpoint
    // by two spellings. `endpoint_id` is what closes it, and this is the
    // assertion that would go red if a later change reached for the path.
    let f = skip_without_db!();
    f.publish(
        "/reports/daily",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1, "burst": 1 }),
    )
    .await;

    assert_eq!(
        f.call_as_client(f.principal_a, "c", "reports/daily")
            .await
            .status,
        StatusCode::OK
    );
    let encoded = f
        .call_as_client(f.principal_a, "c", "reports%2Fdaily")
        .await;
    assert_eq!(
        encoded.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the same endpoint spelled differently must share one bucket: {}",
        encoded.body
    );
}
