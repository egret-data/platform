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

/// **The leak this pair of checks exists for, and the one `delegated` alone
/// missed.**
///
/// `options.impersonate` makes the engine run every query as the CALLER
/// independently of `delegated` — `delegation_for`'s non-delegated arm still
/// passes the caller as the session user, Trino sends `X-Trino-User: <caller>`
/// and StarRocks issues `EXECUTE AS <caller> WITH NO REVERT`. So
/// `{delegated: false, impersonate: true}` is a fully per-principal connection
/// that reads as shared-safe to anything consulting `delegated`.
///
/// The serving-side witness is reached by flipping the flag AFTER publish, which
/// is now the ONLY way to get there — publish itself refuses the combination
/// (see the test below). That is the same no-re-validation window the
/// `delegated` flip opens, and the reason the witness is a PAIR.
///
/// **Assert on the ROWS, not only the status.** A 503 that still handed B a body
/// would satisfy a status-only assertion, and "B received A's rows" is the
/// actual claim.
#[tokio::test]
async fn b_is_not_served_as_rows_when_impersonate_is_switched_on_after_publish() {
    let f = skip_without_db!();
    f.publish("/imp", f.database_id, shared_cache(), no_limits())
        .await;

    // Anti-triviality: on a genuinely shared-safe connection this IS a shared
    // entry, so the refusal below is caused by the flip and not by the endpoint
    // being broken all along.
    let warm = f.call_as(f.principal_a, "imp").await;
    assert_eq!(warm.status, StatusCode::OK, "{}", warm.body);
    let a_rows = warm.ns();
    assert_eq!(
        a_rows,
        vec![marker(f.principal_a)],
        "the fake engine must be filtering per principal, or this test cannot \
         tell one identity's rows from another's"
    );

    // The flip. `delegated` is untouched and still false — which is exactly why
    // reading it alone could never notice.
    f.set_database_impersonate(f.database_id, true).await;

    let b = f.call_as(f.principal_b, "imp").await;
    assert_ne!(
        b.status,
        StatusCode::OK,
        "B was served from a principal-less cache entry on a connection that \
         runs queries as the caller: {}",
        b.body
    );
    assert_eq!(
        b.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "503 cache_scope_stale — the published configuration has drifted and a \
         re-publish (or putting the connection back) clears it: {}",
        b.body
    );
    assert!(
        !b.body.to_string().contains(&a_rows[0].to_string()),
        "the refusal still carried A's rows in its body: {}",
        b.body
    );

    // And it tracks the live value rather than latching once.
    f.set_database_impersonate(f.database_id, false).await;
    assert_eq!(f.call_as(f.principal_b, "imp").await.status, StatusCode::OK);
}

/// The publish-time half. Kept separate from the serving-time test above
/// deliberately: with only one of them, reverting either check alone would leave
/// the other test green and the leak would look closed when half of it was.
#[tokio::test]
async fn a_shared_cache_cannot_be_published_against_an_impersonating_connection() {
    let f = skip_without_db!();
    let (status, body) = f
        .try_publish("/imp-pub", f.impersonating_database_id, shared_cache())
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "`delegated: false, impersonate: true` runs every query as the caller; \
         a shared cache on it serves one caller's rows to another: {body}"
    );
    // The message must name the field an author can actually check. Told only
    // "delegated", they would look at a column that genuinely reads false.
    assert!(
        body.to_string().contains("impersonate"),
        "the refusal must name options.impersonate, not just `delegated`: {body}"
    );

    // The same connection with a per-principal scope publishes fine — so the
    // refusal is about the SCOPE, not about the connection being unusable.
    let (status, body) = f
        .try_publish(
            "/imp-ok",
            f.impersonating_database_id,
            per_principal_cache(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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

/// An endpoint declaring one query parameter, `day`.
///
/// P3 refuses to publish a parameter no placeholder references, so the SQL has
/// to use it. Since P4.5 the value really is rendered and bound; what is under
/// test in this file is still the KEY, and `serving_params.rs` owns the
/// rendering itself.
async fn publish_with_day_param(f: &serving_common::Fixture, path: &str) {
    f.publish_with_params(
        path,
        f.database_id,
        per_principal_cache(),
        no_limits(),
        "SELECT 1 AS n WHERE d = {{day}}",
        serde_json::json!([{ "name": "day", "type": "string", "location": "query" }]),
    )
    .await;
}

#[tokio::test]
async fn different_values_of_a_declared_param_do_not_share_an_entry() {
    // Two values are two entries. Written before step 7 existed, when it could
    // only be measured at the engine (the second call must MISS) because no
    // value reached the SQL; that measurement is still the right one, and
    // `serving_params.rs` now also asserts the rows themselves differ.
    let f = skip_without_db!();
    publish_with_day_param(&f, "/q").await;

    let before = f.port.executions();
    f.call_as(f.principal_a, "q?day=2026-01-01").await;
    f.call_as(f.principal_a, "q?day=2026-01-02").await;
    assert_eq!(
        f.port.executions(),
        before + 2,
        "two values of a declared parameter must be two cache entries"
    );

    let repeat = f.call_as(f.principal_a, "q?day=2026-01-01").await;
    assert!(
        repeat.cached(),
        "…and repeating one of them must still hit: {}",
        repeat.body
    );
}

#[tokio::test]
async fn an_undeclared_query_param_cannot_mint_cache_entries() {
    // The key is built from the DECLARED parameters only. Hashing the whole
    // query string instead let one authenticated client mint unbounded distinct
    // entries with `?junk=1`, `?junk=2`, … in a cache shared by every endpoint
    // in every workspace in the process — a cross-tenant availability effect
    // from a single client, with correctness untouched (which is what made it
    // easy to miss).
    //
    // An undeclared parameter cannot affect the response, so it must not be able
    // to affect the entry either.
    let f = skip_without_db!();
    publish_with_day_param(&f, "/j").await;

    let before = f.port.executions();
    let first = f.call_as(f.principal_a, "j?day=2026-01-01&junk=1").await;
    assert!(!first.cached());
    for i in 2..12 {
        let res = f
            .call_as(f.principal_a, &format!("j?day=2026-01-01&junk={i}"))
            .await;
        assert!(
            res.cached(),
            "junk={i} must hit the entry the declared params identify: {}",
            res.body
        );
    }
    assert_eq!(
        f.port.executions(),
        before + 1,
        "ten spellings of one undeclared parameter must be ONE entry and one execution"
    );
}

/// **Task 10.** The obvious guess, pinned wrong: since this package gave an
/// endpoint several statements, the cache key must now carry the statement
/// list. It must not — the cached value is the WHOLE response, and
/// `cache_key`'s `endpoint_id` already makes two endpoints unable to share an
/// entry, whatever their statement lists say. Two endpoints, otherwise
/// identical (same database, same cache scope, same params, same principal),
/// differing only in the SQL their one statement runs.
///
/// Mutation-verified: dropping `h.update(endpoint_id.as_bytes())` from
/// `cache_key` reddens this test — B collapses onto A's entry, comes back
/// `cached: true`, and `f.port.executions()` stops at `before + 1`.
#[tokio::test]
async fn two_endpoints_differing_only_in_statement_list_do_not_collide() {
    let f = skip_without_db!();
    f.publish_with_params(
        "/a",
        f.database_id,
        per_principal_cache(),
        no_limits(),
        "SELECT 1 AS n",
        serde_json::json!([]),
    )
    .await;
    f.publish_with_params(
        "/b",
        f.database_id,
        per_principal_cache(),
        no_limits(),
        "SELECT 2 AS n",
        serde_json::json!([]),
    )
    .await;

    let before = f.port.executions();
    let a = f.call_as(f.principal_a, "a").await;
    assert!(!a.cached(), "A's first call cannot be a hit: {}", a.body);
    let b = f.call_as(f.principal_a, "b").await;
    assert!(
        !b.cached(),
        "B has never been called; a hit here means the key collapsed two \
         DIFFERENT endpoints — whose statement lists are the only thing that \
         differs — onto one entry: {}",
        b.body
    );
    assert_eq!(
        f.port.executions(),
        before + 2,
        "two endpoints must be two cache entries and two executions, never one"
    );
}

/// **Task 10, the other half.** The statement list is not IN the key, but it
/// does not need to be: a changed statement list only ever reaches serving
/// through a republish, and a republish bumps `version`, which IS in the key.
/// So the old entry is orphaned, never served stale.
///
/// Mutation-verified: dropping `h.update(version.to_le_bytes())` from
/// `cache_key` reddens this test — the version-2 call collides with the
/// version-1 entry, comes back `cached: true`, and `f.port.executions()`
/// stops at `before`.
#[tokio::test]
async fn a_republished_endpoint_misses_the_cache() {
    let f = skip_without_db!();
    let ep = f
        .publish_with_params(
            "/rep",
            f.database_id,
            per_principal_cache(),
            no_limits(),
            "SELECT 1 AS n",
            serde_json::json!([]),
        )
        .await;

    let first = f.call_as(f.principal_a, "rep").await;
    assert!(
        !first.cached(),
        "the first call cannot be a hit: {}",
        first.body
    );
    let warm = f.call_as(f.principal_a, "rep").await;
    assert!(
        warm.cached(),
        "a repeat call against the SAME version must hit, or this test cannot \
         tell a real republish-miss from a cache that never hits at all: {}",
        warm.body
    );

    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT 2 AS n" }]),
    )
    .await;
    let version = f.republish(ep).await;
    assert_eq!(
        version, 2,
        "the republish must actually have produced version 2, or the miss \
         asserted below would prove nothing about a version bump"
    );

    let before = f.port.executions();
    let after_republish = f.call_as(f.principal_a, "rep").await;
    assert!(
        !after_republish.cached(),
        "a republished endpoint must MISS — the warm entry is keyed under \
         version 1, and version 2 has never been executed: {}",
        after_republish.body
    );
    assert_eq!(
        f.port.executions(),
        before + 1,
        "the miss must have reached the engine rather than being served from \
         the stale version-1 entry"
    );
}

#[tokio::test]
async fn a_zero_ttl_does_not_become_a_one_second_cache() {
    // `ttl_seconds: 0` with `enabled: true` is an author saying "do not cache".
    // Clamping it into `1..=CEILING` — which the first version did — invents a
    // caching policy they declined. It is the one place a published 0 could
    // silently become non-zero behaviour.
    let f = skip_without_db!();
    f.publish(
        "/zero",
        f.database_id,
        serde_json::json!({ "enabled": true, "ttl_seconds": 0 }),
        no_limits(),
    )
    .await;

    let before = f.port.executions();
    let one = f.call_as(f.principal_a, "zero").await;
    let two = f.call_as(f.principal_a, "zero").await;
    assert!(!one.cached() && !two.cached(), "{} {}", one.body, two.body);
    assert_eq!(
        f.port.executions(),
        before + 2,
        "a zero TTL must execute every call"
    );
}
