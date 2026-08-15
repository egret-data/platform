//! P4 Task 4 — the response cache, through the serving surface.
//!
//! Two properties, and the rest of the file exists to keep them from passing
//! vacuously:
//!
//! * **The key carries the principal.** Ranger filters per principal, so the
//!   same endpoint and the same parameters can legitimately return different
//!   rows; a key without the principal serves one identity's rows to another
//!   through the exact control the design leans on.
//! * **`shared` is re-checked against the LIVE `delegated` column.** The frozen
//!   `EndpointSnapshot.delegated` is a witness, and a witness nobody questions
//!   is decoration.
//!
//! The fake engine returns a row derived from the CALLING principal
//! (`serving_common::marker`), which is what makes the first assertion a
//! statement about the key rather than about a constant, and it counts its own
//! executions, which is what tells a cache HIT from a re-execution that happened
//! to agree.
//!
//! DB-gated; self-skips when `DATABASE_URL` is unset — check the DURATION.

mod serving_common;

use axum::http::StatusCode;
use serving_common::{fixture, marker};

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

/// `cache.enabled` with the default `per_principal` scope.
fn per_principal_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" })
}

fn shared_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "shared" })
}

fn no_limits() -> serde_json::Value {
    serde_json::json!({})
}

#[tokio::test]
async fn two_principals_do_not_share_a_cached_response() {
    // Ranger filters per principal, so the same endpoint and the same params can
    // legitimately return different rows. A cache key without the principal
    // serves one identity's rows to another.
    let f = skip_without_db!();
    f.publish("/x", f.database_id, per_principal_cache(), no_limits())
        .await;

    let a = f.call_as(f.principal_a, "x").await;
    let b = f.call_as(f.principal_b, "x").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(b.status, StatusCode::OK, "{}", b.body);

    // Anti-triviality: A really did get A's own rows, so the assertion below has
    // something to fail on. It also names which of the two failures happened —
    // dropping the principal from the key makes B's call a HIT on A's entry, and
    // the message must not then read as "the fixture is broken".
    assert_eq!(a.ns(), vec![marker(f.principal_a)]);
    assert_ne!(
        a.ns(),
        b.ns(),
        "B was served A's rows. Either the fixture stopped granting them \
         different rows (then A's row above would not be A's), or the cache key \
         no longer separates the two principals — which is the leak this test \
         exists for."
    );

    let b_again = f.call_as(f.principal_b, "x").await;
    assert_eq!(
        b_again.ns(),
        b.ns(),
        "B's second call must still be B's rows"
    );
    assert!(
        b_again.cached(),
        "…and it must have come from the cache — otherwise this test would pass \
         with no cache at all"
    );
}

#[tokio::test]
async fn a_repeat_call_is_served_from_the_cache_without_re_executing() {
    // The anti-triviality for the whole file: if the cache were never consulted,
    // every "must not share" assertion above would hold for free. Counted at the
    // ENGINE, because the body of a hit and of a re-execution are identical by
    // construction.
    let f = skip_without_db!();
    f.publish("/x", f.database_id, per_principal_cache(), no_limits())
        .await;

    let first = f.call_as(f.principal_a, "x").await;
    let after_first = f.port.executions();
    assert!(!first.cached(), "the first call cannot be a hit");

    let second = f.call_as(f.principal_a, "x").await;
    assert!(second.cached(), "the second call must be a hit");
    assert_eq!(second.ns(), first.ns());
    assert_eq!(
        f.port.executions(),
        after_first,
        "a cache hit must not reach the engine"
    );
}

#[tokio::test]
async fn a_disabled_cache_executes_every_call() {
    // `cache.enabled = false` is the default, and it must mean what it says:
    // nothing is stored and nothing is served from store. Without this, a cache
    // that ignored the flag would still pass every test above.
    let f = skip_without_db!();
    f.publish(
        "/off",
        f.database_id,
        serde_json::json!({ "enabled": false }),
        no_limits(),
    )
    .await;

    let before = f.port.executions();
    let one = f.call_as(f.principal_a, "off").await;
    let two = f.call_as(f.principal_a, "off").await;
    assert!(!one.cached() && !two.cached());
    assert_eq!(
        f.port.executions(),
        before + 2,
        "a disabled cache must execute every call"
    );
}

#[tokio::test]
async fn a_shared_cache_serves_one_entry_to_every_principal() {
    // `scope = shared` on a NON-delegated connection is the configuration the
    // spec allows, and it must actually do the thing: the engine ran once, as
    // whoever missed first, and both identities are served that entry.
    //
    // Kept as its own test because it is what proves the principal-free branch
    // is reachable at all — without it, "the principal is in the key" could be
    // true because the shared branch is dead code, and the mutation that drops
    // the principal would be indistinguishable from this configuration.
    let f = skip_without_db!();
    f.publish("/shared", f.database_id, shared_cache(), no_limits())
        .await;

    let before = f.port.executions();
    let a = f.call_as(f.principal_a, "shared").await;
    let b = f.call_as(f.principal_b, "shared").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(b.status, StatusCode::OK, "{}", b.body);
    assert!(b.cached(), "B must be served A's entry: {}", b.body);
    assert_eq!(a.ns(), b.ns());
    assert_eq!(
        f.port.executions(),
        before + 1,
        "a shared cache must execute once for both principals"
    );
}

#[tokio::test]
async fn a_shared_cache_is_refused_when_delegated_flipped_after_publish() {
    // The one snapshot-external input. Publish `shared` against a non-delegated
    // database (allowed), then flip the database to delegated — nothing
    // re-validates when that happens, and every already-published `shared`
    // version becomes a cross-principal leak.
    //
    // Refused, not downgraded to a per-principal key: silently serving the
    // endpoint a different way hides the drift from the operator who has to fix
    // it.
    let f = skip_without_db!();
    f.publish("/x", f.database_id, shared_cache(), no_limits())
        .await;
    // Anti-triviality: it serves before the flip, so the refusal below is caused
    // by the flip and not by the endpoint being broken all along.
    assert_eq!(f.call_as(f.principal_a, "x").await.status, StatusCode::OK);

    f.set_database_delegated(f.database_id, true).await;

    let res = f.call_as(f.principal_a, "x").await;
    assert_ne!(
        res.status,
        StatusCode::OK,
        "the frozen witness disagrees with the live column — refuse rather than leak"
    );
    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "503: the configuration has drifted and a re-publish clears it — the \
         endpoint is not permanently broken and nothing on egret's side failed"
    );

    // And the refusal survives the flip back — i.e. it tracks the live column
    // rather than latching once.
    f.set_database_delegated(f.database_id, false).await;
    assert_eq!(f.call_as(f.principal_a, "x").await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_per_principal_endpoint_is_unaffected_by_the_delegated_column() {
    // The re-check is scoped to `shared`. A `per_principal` endpoint on a
    // flipped connection is still safe — its key carries the principal — and
    // must keep serving, or every delegation change would take out every
    // endpoint on that database.
    let f = skip_without_db!();
    f.publish(
        "/pp",
        f.delegated_database_id,
        per_principal_cache(),
        no_limits(),
    )
    .await;
    assert_eq!(f.call_as(f.principal_a, "pp").await.status, StatusCode::OK);

    f.set_database_delegated(f.delegated_database_id, false)
        .await;
    let res = f.call_as(f.principal_a, "pp").await;
    assert_eq!(
        res.status,
        StatusCode::OK,
        "a per-principal endpoint must not be refused for a delegation change: {}",
        res.body
    );
    f.set_database_delegated(f.delegated_database_id, true)
        .await;
}

#[tokio::test]
async fn different_query_strings_do_not_share_an_entry() {
    // Params do not reach the SQL until step 7, so this cannot be asserted on
    // the ROWS yet — what it pins is that the two calls are two entries, which
    // is the property step 7 will depend on. Measured at the engine: the second
    // call must miss.
    let f = skip_without_db!();
    f.publish("/q", f.database_id, per_principal_cache(), no_limits())
        .await;

    let before = f.port.executions();
    f.call_as(f.principal_a, "q?day=2026-01-01").await;
    f.call_as(f.principal_a, "q?day=2026-01-02").await;
    assert_eq!(
        f.port.executions(),
        before + 2,
        "two different query strings must be two cache entries"
    );

    let repeat = f.call_as(f.principal_a, "q?day=2026-01-01").await;
    assert!(
        repeat.cached(),
        "…and repeating one of them must still hit: {}",
        repeat.body
    );
}
