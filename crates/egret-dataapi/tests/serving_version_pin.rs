//! Package 7 — a consumer pins the version it built against, and pinning buys
//! it nothing else.
//!
//! # The shape every test in this file is built to avoid
//!
//! **A pinning test whose fixture has only ONE version cannot distinguish
//! "served the pin" from "served live".** It passes with the header read, with
//! the header ignored, and with the header misspelled. So every fixture here
//! publishes at least two versions whose answers DIFFER, and the difference is
//! observed in three independent places rather than one:
//!
//! * `meta.version` — which could be a mere echo of the header, so it is never
//!   the only assertion;
//! * the SQL the fake engine was actually asked to run (`FakePort::last`), which
//!   an echo cannot fake;
//! * the execution COUNT, which is what separates "answered from the pinned
//!   version" from "answered from a cache entry belonging to another version".
//!
//! [`two_versions`] builds that fixture once, and every test starts from it.
//!
//! # The property this package could get catastrophically wrong
//!
//! Pinning pins the CONTRACT, not the PERMISSIONS. A pinned old version stays
//! subject to everything evaluated live today — the invoke grant, the
//! governed-database predicate, the endpoint's state, the workspace's. Each of
//! those has a test below, and each is written in the same five steps:
//!
//! 1. two versions exist;
//! 2. **the pin to the old one succeeds** — without this the rest is vacuous,
//!    because a pin that never worked cannot resurrect anything;
//! 3. the withdrawal happens (revoke, govern, disable, trash);
//! 4. the pinned call is refused, with the code that names the real cause;
//! 5. the UNPINNED call is refused identically, so the pin is shown not to be
//!    special-cased in either direction.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run
//! with no `DATABASE_URL` FAILS — check the DURATION, not just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use egret_core::ids::UserId;
use serving_common::{fixture, Fixture, Res};
use uuid::Uuid;

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

fn cache_on() -> serde_json::Value {
    serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" })
}

fn no_limits() -> serde_json::Value {
    serde_json::json!({})
}

/// The SQL each version is published with. Two statements that are trivially
/// different, so the rendered text the engine receives says which contract ran.
const V1_SQL: &str = "SELECT 1 AS n /* version one */";
const V2_SQL: &str = "SELECT 2 AS n /* version two */";

/// The fixture every test starts from: one endpoint, TWO published versions,
/// whose SQL differs.
///
/// Returns `(endpoint id, url path, live version number)`. The live version is
/// asserted to be `2` rather than assumed: `republish` goes through the real
/// publish route, and a route that silently declined would leave a
/// one-version fixture behind — which is the exact failure this file exists to
/// not have.
///
/// The two versions differ only in `sql` on purpose. Package 6 classifies a
/// `sql` edit as reported-but-safe, so `republish` (which sends no
/// acknowledgement) is the honest way to reach version 2 — and if that
/// classification ever changed, this helper would fail loudly at the publish
/// rather than quietly produce one version.
async fn two_versions(f: &Fixture, path: &str, cache: serde_json::Value) -> (Uuid, String, i32) {
    let ep = f
        .publish_with_params(
            path,
            f.database_id,
            cache,
            no_limits(),
            V1_SQL,
            serde_json::json!([]),
        )
        .await;
    f.set_statements(ep, serde_json::json!([{ "name": "main", "sql": V2_SQL }]))
        .await;
    let live = f.republish(ep).await;
    assert_eq!(
        live, 2,
        "the fixture must really carry two versions; with one, every assertion \
         in this file passes whether the pin is read or ignored"
    );
    (ep, path.trim_start_matches('/').to_string(), live)
}

/// The `/* version … */` comment out of the SQL the engine was last asked to
/// run — i.e. which version's statement actually executed.
fn executed_version(f: &Fixture) -> i32 {
    let (sql, _) = f.port.last();
    if sql.contains("version one") {
        1
    } else if sql.contains("version two") {
        2
    } else {
        panic!("the engine ran a statement from neither published version: {sql:?}");
    }
}

fn code(res: &Res) -> String {
    res.body["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("no error.code in {}", res.body))
        .to_string()
}

// ---------------------------------------------------------------------------
// The feature itself
// ---------------------------------------------------------------------------

/// The whole point, in one test: the same URL and the same token answer from two
/// different contracts depending on one header.
///
/// Both directions are asserted in one place because separately each half is
/// satisfiable by an implementation that ignores the header (the unpinned half)
/// or by one that pins everything to the oldest version (the pinned half). Only
/// the pair rules both out.
///
/// Mutation: drop the `pin` argument from `live_snapshot`'s join condition — so
/// `v` always binds `e.live_version_id` — and the pinned assertions fail on both
/// `meta.version` and `executed_version`.
#[tokio::test]
async fn a_pin_selects_the_frozen_version_and_its_absence_selects_the_live_one() {
    let f = skip_without_db!();
    let (_, path, live) = two_versions(&f, "/pin-basic", no_cache()).await;

    // Unpinned: the live version, which is what every caller got before this
    // header existed.
    let unpinned = f.call_as(f.principal_a, &path).await;
    assert_eq!(unpinned.status, StatusCode::OK, "{}", unpinned.body);
    assert_eq!(unpinned.body["meta"]["version"], serde_json::json!(live));
    assert_eq!(
        executed_version(&f),
        2,
        "the unpinned call must run the LIVE version's statement"
    );

    // Pinned to the superseded one.
    let pinned = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(pinned.status, StatusCode::OK, "{}", pinned.body);
    assert_eq!(
        pinned.body["meta"]["version"],
        serde_json::json!(1),
        "meta.version must report the version that ANSWERED: {}",
        pinned.body
    );
    // The assertion `meta.version` alone cannot make: an implementation that
    // echoed the header back and served the live snapshot would pass the line
    // above and fail this one.
    assert_eq!(
        executed_version(&f),
        1,
        "the pinned call must run VERSION ONE's frozen statement, not the live \
         one — meta.version agreeing is not evidence on its own"
    );

    // And pinning the version that IS live is not a special case.
    let pinned_live = f.call_pinned(f.principal_a, &path, "2").await;
    assert_eq!(pinned_live.status, StatusCode::OK, "{}", pinned_live.body);
    assert_eq!(pinned_live.body["meta"]["version"], serde_json::json!(2));
    assert_eq!(executed_version(&f), 2);
}

// ---------------------------------------------------------------------------
// Pinning pins the contract, NOT the permissions
// ---------------------------------------------------------------------------

/// A revoked invoke grant still refuses, pinned or not.
///
/// If the pin bypassed step 4, a consumer who had ever been granted a version
/// could keep calling it forever by naming it — which would turn a revocation
/// into a suggestion.
///
/// Mutation: move `invoke_allowed` below the pin resolution in `router::run` and
/// this still passes (the grant check still runs); move it into the
/// `pin.is_none()` path and this fails on the first assertion after the revoke.
#[tokio::test]
async fn a_revoked_grant_still_refuses_a_pinned_version() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-grant", no_cache()).await;

    // Step 2: the pin WORKS while the grant stands. Without this the refusals
    // below prove nothing — a pin that never served version 1 cannot be shown to
    // stop serving it.
    let before = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        before.status,
        StatusCode::OK,
        "the pinned call must succeed before the grant is taken away: {}",
        before.body
    );
    assert_eq!(executed_version(&f), 1);

    f.revoke_invoke(f.principal_a, ep).await;

    let pinned = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        pinned.status,
        StatusCode::FORBIDDEN,
        "a pin must not resurrect a revoked grant: {}",
        pinned.body
    );
    assert_eq!(code(&pinned), "not_granted", "{}", pinned.body);

    // And the unpinned call answers identically, so the pin is not special-cased
    // in either direction.
    let unpinned = f.call_as(f.principal_a, &path).await;
    assert_eq!(unpinned.status, StatusCode::FORBIDDEN, "{}", unpinned.body);
    assert_eq!(code(&unpinned), "not_granted", "{}", unpinned.body);

    // The other principal, still granted, still gets both — so the refusal above
    // is about the revoked grant and not about the endpoint having broken.
    let other = f.call_pinned(f.principal_b, &path, "1").await;
    assert_eq!(
        other.status,
        StatusCode::OK,
        "the endpoint must still serve a principal whose grant stands: {}",
        other.body
    );
}

/// Marking a database `governed` takes effect on a PINNED old version.
///
/// This is the one Package 1 deliberately built as a live predicate rather than
/// a frozen witness, "precisely so that marking a database governed takes effect
/// on already-published versions". A pin must not be a way back to the frozen
/// world — and the mechanism that makes it not one is that the routing query's
/// `LEFT JOIN query.databases` keys on the pinned snapshot's `database_id` and
/// reads the LIVE row.
///
/// The fixture's `database_id` is `delegated = false, impersonate = false`, so
/// flipping `governed` alone reaches `governed && !runs_per_principal(…)` — the
/// state the check refuses. That is the "can the fixture reach the dangerous
/// state" question, and the answer is checked by the assertion at step 2 being
/// `OK` and the one at step 4 being `503`.
#[tokio::test]
async fn a_database_that_becomes_governed_refuses_a_pinned_version_too() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-governed", no_cache()).await;

    let before = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        before.status,
        StatusCode::OK,
        "the pinned call must succeed before the database is governed: {}",
        before.body
    );
    assert_eq!(executed_version(&f), 1);

    f.set_database_governed(f.database_id, true).await;

    let pinned = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        pinned.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a pin must not serve governed rows under the service account: {}",
        pinned.body
    );
    assert_eq!(code(&pinned), "governance_stale", "{}", pinned.body);

    let unpinned = f.call_as(f.principal_a, &path).await;
    assert_eq!(
        unpinned.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        unpinned.body
    );
    assert_eq!(code(&unpinned), "governance_stale", "{}", unpinned.body);
}

/// A disabled endpoint, and then a trashed one, stay 404 under a pin.
///
/// `disable` is what takes a URL off the internet (`trash` refuses a `live`
/// endpoint outright, so it is only ever reached from `disabled`), and the
/// serving query's `e.state = 'live'` is what enforces it. The pin changes which
/// `endpoint_versions` row is joined and nothing about that predicate — this is
/// the test that says so.
///
/// The refusal must be byte-identical to an unknown URL's, `request_id` aside:
/// a `version_unavailable` here would tell a prober that the endpoint exists and
/// merely has no such version.
#[tokio::test]
async fn a_disabled_or_trashed_endpoint_is_not_reachable_by_pinning_a_version_it_once_served() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-disabled", no_cache()).await;
    let mgmt = format!("/api/ws/{}/data-api/endpoints/{ep}", f.ws_slug);

    let before = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        before.status,
        StatusCode::OK,
        "the pinned call must succeed before the endpoint is disabled: {}",
        before.body
    );

    let (status, body) = f.mgmt_post(&format!("{mgmt}/disable"), None).await;
    assert_eq!(status, StatusCode::OK, "disable: {body}");

    let reference = f
        .call_pinned(f.principal_a, "no-such-endpoint-here", "1")
        .await;
    assert_eq!(
        reference.status,
        StatusCode::NOT_FOUND,
        "{}",
        reference.body
    );

    let disabled = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        disabled.status,
        StatusCode::NOT_FOUND,
        "a pin must not reach a disabled endpoint: {}",
        disabled.body
    );
    assert_eq!(
        code(&disabled),
        code(&reference),
        "a disabled endpoint must answer a pinned caller exactly as an unknown \
         URL does; a version-specific code here is a directory of what exists: {}",
        disabled.body
    );

    // And through the trash, which is where a disabled endpoint goes next.
    let (status, body) = f.mgmt_post(&format!("{mgmt}/trash"), None).await;
    assert_eq!(status, StatusCode::OK, "trash: {body}");
    let trashed = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(trashed.status, StatusCode::NOT_FOUND, "{}", trashed.body);
    assert_eq!(code(&trashed), code(&reference), "{}", trashed.body);
}

// ---------------------------------------------------------------------------
// A pin cannot name a version that is not this endpoint's
// ---------------------------------------------------------------------------

/// Version numbers are per endpoint, and a pin cannot cross that line.
///
/// # The first version of this test passed for the wrong reason, and the fixture
/// # is the whole difference
///
/// It gave the endpoint under test two versions, gave a sibling three, and
/// pinned to `3`. That looks like a scoping test and is not one: `3` is also
/// **above** the live version, so the `v.version <= lv.version` clause refuses
/// it. Deleting `v.endpoint_id = e.id` from the join left it green.
///
/// The two predicates overlap for every reachable state, because published
/// version numbers are CONTIGUOUS: any number at or below an endpoint's live
/// version is a number that endpoint itself has. So the only way to build the
/// state where the scoping is the sole thing standing between the caller and
/// another endpoint's contract is to put a **gap** in this endpoint's history —
/// which is exactly the state a version pruner would create, and this file's
/// retention test records that no such pruner exists yet. This test therefore
/// does double duty: it pins the scoping today, and it pins what a pruner must
/// not break tomorrow.
///
/// With the gap in place the join is unambiguous either way, so the outcome is
/// deterministic rather than a race between two matching rows:
///
/// * scoped — no row for `(this endpoint, 2)`, so `409 version_unavailable`;
/// * unscoped — the sibling's version 2 is the only match, and the caller
///   receives ANOTHER endpoint's SQL, `database_id` and frozen columns under a
///   `200`.
#[tokio::test]
async fn a_version_number_belonging_to_another_endpoint_is_not_servable_here() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-scope-a", no_cache()).await;
    // A third version, so version 2 becomes an INTERIOR one that can be removed
    // without disturbing the live pointer.
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT 1 AS n /* version one */" }]),
    )
    .await;
    assert_eq!(f.republish(ep).await, 3);

    // The sibling, in the same workspace, with a version 2 of its own carrying
    // SQL that is recognisably not this endpoint's.
    let sibling = f
        .publish_with_params(
            "/pin-scope-b",
            f.database_id,
            no_cache(),
            no_limits(),
            "SELECT 9 AS n /* sibling one */",
            serde_json::json!([]),
        )
        .await;
    f.set_statements(
        sibling,
        serde_json::json!([{ "name": "main", "sql": "SELECT 9 AS n /* sibling two */" }]),
    )
    .await;
    assert_eq!(f.republish(sibling).await, 2);

    // Remove THIS endpoint's version 2, leaving the sibling's as the only row in
    // the table carrying that number and a version this endpoint could serve.
    // `compared_against` on version 3 points at it (package 6), and that
    // reference has no `ON DELETE` action, so it is cleared first — a delete
    // that failed here would leave the gap unmade and the test vacuous, which is
    // why the row count is asserted afterwards.
    sqlx::query(
        "UPDATE dataapi.endpoint_versions SET compared_against = NULL \
         WHERE compared_against IN (SELECT id FROM dataapi.endpoint_versions \
                                    WHERE endpoint_id = $1 AND version = 2)",
    )
    .bind(ep)
    .execute(&f.db.pool)
    .await
    .expect("clear compared_against");
    let removed =
        sqlx::query("DELETE FROM dataapi.endpoint_versions WHERE endpoint_id = $1 AND version = 2")
            .bind(ep)
            .execute(&f.db.pool)
            .await
            .expect("remove the interior version")
            .rows_affected();
    assert_eq!(removed, 1, "the gap must actually have been made");

    let candidates: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dataapi.endpoint_versions WHERE version = 2 AND endpoint_id = $1",
    )
    .bind(sibling)
    .fetch_one(&f.db.pool)
    .await
    .expect("count sibling version 2");
    assert_eq!(
        candidates, 1,
        "the sibling must really hold a version 2, or this test refuses a number \
         that exists nowhere and says nothing about scoping"
    );

    let res = f.call_pinned(f.principal_a, &path, "2").await;
    assert_eq!(
        res.status,
        StatusCode::CONFLICT,
        "version 2 is not this endpoint's, and serving the sibling's would hand \
         the caller another endpoint's contract: {}",
        res.body
    );
    assert_eq!(code(&res), "version_unavailable", "{}", res.body);

    // The versions this endpoint really has are unaffected, so the refusal above
    // is about the gap and not about pinning having stopped working.
    for (pin, expect) in [("1", 1), ("3", 3)] {
        let ok = f.call_pinned(f.principal_a, &path, pin).await;
        assert_eq!(ok.status, StatusCode::OK, "pin {pin}: {}", ok.body);
        assert_eq!(ok.body["meta"]["version"], serde_json::json!(expect));
    }
}

/// A pin ABOVE the live version is refused, because the only way to get there is
/// a rollback.
///
/// `rollback` is deliberately not gated (it is the one act that must work during
/// an outage). If a pin could name the version it moved consumers off, a caller
/// could undo the operator's rollback with a header, and the endpoint's own
/// author would have no way to stop them.
///
/// The fixture reaches the state for real: version 2 is published, is live, is
/// served, and only THEN is rolled back — so the version being refused is one
/// this very test watched succeed.
#[tokio::test]
async fn a_version_a_rollback_moved_consumers_off_cannot_be_pinned_back_to() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-rollback", no_cache()).await;

    let before = f.call_pinned(f.principal_a, &path, "2").await;
    assert_eq!(
        before.status,
        StatusCode::OK,
        "version 2 must be servable before the rollback, or the refusal below \
         says nothing about the rollback: {}",
        before.body
    );

    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/rollback", f.ws_slug),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "rollback: {body}");

    // The rollback took effect for the unpinned caller...
    let unpinned = f.call_as(f.principal_a, &path).await;
    assert_eq!(unpinned.status, StatusCode::OK, "{}", unpinned.body);
    assert_eq!(unpinned.body["meta"]["version"], serde_json::json!(1));
    assert_eq!(executed_version(&f), 1);

    // ...and a header cannot undo it.
    let pinned = f.call_pinned(f.principal_a, &path, "2").await;
    assert_eq!(
        pinned.status,
        StatusCode::CONFLICT,
        "a pin must not put back the version a rollback took away: {}",
        pinned.body
    );
    assert_eq!(code(&pinned), "version_unavailable", "{}", pinned.body);

    // The version the rollback landed on is still pinnable, so the refusal above
    // is about being ABOVE live and not about pinning having stopped working.
    let still = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(still.status, StatusCode::OK, "{}", still.body);
    assert_eq!(still.body["meta"]["version"], serde_json::json!(1));
}

/// A version this endpoint never published is a clear refusal, never a silent
/// fall-through to live.
///
/// The silent fall-through is the failure mode worth naming: a consumer pinned
/// to a version that has gone would otherwise receive the live one, with a
/// `meta.version` they are not reading, and discover the contract change as
/// wrong data rather than as an error.
#[tokio::test]
async fn a_version_that_was_never_published_is_refused_and_not_answered_from_live() {
    let f = skip_without_db!();
    let (_, path, live) = two_versions(&f, "/pin-unknown", no_cache()).await;
    let before = f.port.executions();

    let res = f.call_pinned(f.principal_a, &path, "99").await;
    assert_eq!(res.status, StatusCode::CONFLICT, "{}", res.body);
    assert_eq!(code(&res), "version_unavailable", "{}", res.body);
    assert!(
        res.body.get("meta").is_none(),
        "a refusal must not carry a meta block, and certainly not one reporting \
         the live version: {}",
        res.body
    );
    assert_eq!(
        f.port.executions(),
        before,
        "an unknown pin must not have run the live version's query"
    );
    // The live version is still there, so the refusal is about the pin.
    let unpinned = f.call_as(f.principal_a, &path).await;
    assert_eq!(unpinned.body["meta"]["version"], serde_json::json!(live));
}

/// The refusal names nothing about the endpoint.
///
/// Serving errors leak nothing — not a parameter name, a bound, or a constraint
/// kind — and this one must not leak the version numbers either. What it can
/// honestly contain is the name of the header the caller sent and what to do
/// next; those are the caller's own facts.
#[tokio::test]
async fn the_version_refusals_name_no_endpoint_fact() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-quiet", no_cache()).await;

    for pin in ["99", "3"] {
        let res = f.call_pinned(f.principal_a, &path, pin).await;
        let text = res.body.to_string();
        assert_eq!(code(&res), "version_unavailable", "{text}");
        for leak in [
            "\"version\":1",
            "\"version\":2",
            "\"live\"",
            V1_SQL,
            V2_SQL,
            "pin-quiet",
        ] {
            assert!(
                !text.contains(leak),
                "the refusal for pin {pin} leaked {leak:?}: {text}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A malformed pin
// ---------------------------------------------------------------------------

/// Every value the parser refuses, refused end to end with a 400 that names the
/// caller's header and nothing else.
///
/// The unit tests in `serving::pin` cover the parse; this covers the wiring —
/// that the parse is reached at all, that its error becomes
/// `version_pin_invalid` rather than a 500, and that no query runs.
///
/// `+1` is in the list for the reason it is in the unit test: `i32::from_str`
/// accepts it, so a router built on a bare `parse()` passes every other case
/// here.
#[tokio::test]
async fn a_malformed_pin_is_a_400_that_runs_no_query() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-malformed", no_cache()).await;
    let before = f.port.executions();

    for bad in ["", " ", "abc", "+1", "-1", "0", "1 ", "1.0", "1234567890"] {
        let res = f.call_pinned(f.principal_a, &path, bad).await;
        assert_eq!(
            res.status,
            StatusCode::BAD_REQUEST,
            "pin {bad:?} must be a 400: {}",
            res.body
        );
        assert_eq!(
            code(&res),
            "version_pin_invalid",
            "pin {bad:?}: {}",
            res.body
        );
    }
    assert_eq!(
        f.port.executions(),
        before,
        "a malformed pin must not reach the engine"
    );
}

/// Two pins on one request are refused, not resolved by header order.
///
/// The fixture has to be able to reach the dangerous state, and here that means
/// the two values must name two DIFFERENT and individually servable versions —
/// `1` and `2`. An implementation using `HeaderMap::get` returns the first and
/// would answer `200` with version 1, which is a perfectly plausible-looking
/// result; a fixture sending `1` twice, or sending one servable and one
/// nonsense value, could not tell that apart from the refusal.
#[tokio::test]
async fn two_pins_on_one_request_are_refused_rather_than_answered_by_order() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-twice", no_cache()).await;

    // Both are individually servable, so an implementation that picked either
    // one would answer 200.
    for v in ["1", "2"] {
        assert_eq!(
            f.call_pinned(f.principal_a, &path, v).await.status,
            StatusCode::OK,
            "version {v} must be servable on its own for this test to mean anything"
        );
    }

    let res = f
        .call_as_client_with_headers(
            f.principal_a,
            "client-double-pin",
            &path,
            &[("x-egret-version", "1"), ("x-egret-version", "2")],
        )
        .await;
    assert_eq!(
        res.status,
        StatusCode::BAD_REQUEST,
        "a request with two pins has two answers and must be given none: {}",
        res.body
    );
    assert_eq!(code(&res), "version_pin_invalid", "{}", res.body);
}

/// A malformed pin from a caller who has proved nothing is still a 401.
///
/// Authentication answers first. A `400` reachable without a credential would be
/// a free "this host speaks the Data API" oracle, which is the same reason there
/// is one `key_invalid` for every authentication failure.
#[tokio::test]
async fn an_unauthenticated_caller_with_a_malformed_pin_still_gets_401() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-unauth", no_cache()).await;

    let res = f
        .call_with_raw_authorization(&path, "Bearer not-a-token", &[("x-egret-version", "abc")])
        .await;
    assert_eq!(
        res.status,
        StatusCode::UNAUTHORIZED,
        "the pin must be parsed AFTER authentication: {}",
        res.body
    );
    assert_eq!(code(&res), "key_invalid", "{}", res.body);
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// A pinned request cannot read a live-version entry, and a live request cannot
/// read a pinned one.
///
/// The key already carries the version (`cache_key`'s
/// `endpoint(16) ‖ version(4) ‖ params ‖ principal(16)`), so this changes
/// nothing about its shape — which is exactly why it needs a witness rather than
/// an argument. The witness is the EXECUTION COUNT: two versions returning the
/// same single column would produce identical bodies whether the cache leaked or
/// not.
///
/// Mutation: pass a constant in place of `version` to `cache_key` and the
/// "warmed live, then pinned" step below serves version 2's body under
/// `meta.version: 1` with no execution.
#[tokio::test]
async fn a_pinned_request_and_a_live_one_do_not_share_a_cache_entry() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-cache", cache_on()).await;

    // Warm the LIVE entry.
    let live_miss = f.call_as(f.principal_a, &path).await;
    assert_eq!(live_miss.status, StatusCode::OK, "{}", live_miss.body);
    assert!(!live_miss.cached(), "the first call is a miss");
    let after_live = f.port.executions();
    assert_eq!(executed_version(&f), 2);

    // A pinned call must NOT be answered from it.
    let pinned_miss = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(pinned_miss.status, StatusCode::OK, "{}", pinned_miss.body);
    assert!(
        !pinned_miss.cached(),
        "a pinned call must not hit the live version's entry: {}",
        pinned_miss.body
    );
    assert_eq!(
        f.port.executions(),
        after_live + 1,
        "a pinned call that hit the live entry would have executed nothing"
    );
    assert_eq!(executed_version(&f), 1);
    assert_eq!(pinned_miss.body["meta"]["version"], serde_json::json!(1));

    // The pinned entry is its own, and hits.
    let pinned_hit = f.call_pinned(f.principal_a, &path, "1").await;
    assert!(pinned_hit.cached(), "{}", pinned_hit.body);
    assert_eq!(pinned_hit.body["meta"]["version"], serde_json::json!(1));
    let after_pinned = f.port.executions();
    assert_eq!(after_pinned, after_live + 1, "a hit executes nothing");

    // And the live entry is untouched by any of it: still warm, still version 2.
    let live_hit = f.call_as(f.principal_a, &path).await;
    assert!(live_hit.cached(), "{}", live_hit.body);
    assert_eq!(live_hit.body["meta"]["version"], serde_json::json!(2));
    assert_eq!(f.port.executions(), after_pinned, "a hit executes nothing");
}

/// Two principals still do not share a pinned entry.
///
/// The principal is the key's trailing field and the version is in front of the
/// parameters; adding a version to the request must not have moved either. The
/// fake engine returns a row derived from the CALLING principal, so a leak is
/// visible in the body rather than only in a counter.
#[tokio::test]
async fn a_pin_does_not_make_a_cache_entry_shared_between_principals() {
    let f = skip_without_db!();
    let (_, path, _) = two_versions(&f, "/pin-cache-principal", cache_on()).await;

    let a = f.call_pinned(f.principal_a, &path, "1").await;
    let b = f.call_pinned(f.principal_b, &path, "1").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(b.status, StatusCode::OK, "{}", b.body);
    assert!(!b.cached(), "B must not hit A's entry: {}", b.body);
    assert_ne!(
        a.ns(),
        b.ns(),
        "two principals received the same rows from a per-principal endpoint"
    );
}

// ---------------------------------------------------------------------------
// The rate limit follows the LIVE version, never the pin
// ---------------------------------------------------------------------------

/// A pin does not carry its own rate limit.
///
/// The buckets are keyed on `endpoint_id` alone (`ratelimit::client_key`,
/// `endpoint_key`) — no version — so one endpoint has one bucket. If the rate
/// came from the pinned snapshot, a caller could pick the refill rate of a
/// shared counter with a header: `TokenBucket::take` tops the stored tokens up to
/// `rate.burst` on every call, so alternating a pin to a generous old version
/// with an unpinned call would refill the bucket at the old rate and spend it at
/// the new one. That is Package 4's "a shared counter driven by a caller-supplied
/// value" defect wearing different clothes.
///
/// **The fixture has to be able to reach it.** Version 1 is published with a
/// generous limit and version 2 with a tight one — a TIGHTENING, which package 6
/// classifies as breaking, so the republish goes through the acknowledgement
/// route. Publishing them the other way round would have produced a test that
/// passes under both implementations.
///
/// The clock is frozen (`TestClock`), so no bucket refills on its own during the
/// test and every admission is a token that was really there.
#[tokio::test]
async fn a_pinned_call_is_limited_by_the_live_versions_rate_limit() {
    let f = skip_without_db!();
    // Version 1: two requests' worth of allowance.
    let ep = f
        .publish_with_params(
            "/pin-ratelimit",
            f.database_id,
            no_cache(),
            serde_json::json!({ "per_key_qps": 2, "burst": 2, "per_endpoint_qps": 0 }),
            V1_SQL,
            serde_json::json!([]),
        )
        .await;
    // Version 2: one. A tightening, hence the acknowledgement.
    f.set_rate_limit(
        ep,
        serde_json::json!({ "per_key_qps": 1, "burst": 1, "per_endpoint_qps": 0 }),
    )
    .await;
    f.set_statements(ep, serde_json::json!([{ "name": "main", "sql": V2_SQL }]))
        .await;
    assert_eq!(f.republish_acknowledging(ep).await, 2);

    let path = "pin-ratelimit";
    let client = "client-pin-rl";

    // One token, spent by a PINNED call to the generous version.
    let first = f
        .call_as_client_with_headers(f.principal_a, client, path, &[("x-egret-version", "1")])
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!(first.body["meta"]["version"], serde_json::json!(1));

    // The live version's burst of 1 is now spent. Under version 1's own limit
    // there would still be a token here.
    let second = f
        .call_as_client_with_headers(f.principal_a, client, path, &[("x-egret-version", "1")])
        .await;
    assert_eq!(
        second.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the LIVE version's tightened limit must bound a pinned caller; \
         version 1's burst of 2 would have admitted this one: {}",
        second.body
    );
    assert_eq!(code(&second), "rate_limited", "{}", second.body);

    // And the unpinned caller shares that one bucket rather than getting a
    // second allowance — the pin bought no extra capacity at all.
    let unpinned = f.call_as_client(f.principal_a, client, path).await;
    assert_eq!(
        unpinned.status,
        StatusCode::TOO_MANY_REQUESTS,
        "one endpoint has one bucket: {}",
        unpinned.body
    );
}

/// A caller looping on a stale pin is rate limited rather than refused for free.
///
/// The version refusal sits AFTER the limiter deliberately, which is only
/// possible because the limiter's rate comes from the live version and therefore
/// needs no resolved snapshot.
#[tokio::test]
async fn a_stale_pin_is_bounded_by_the_endpoints_published_allowance() {
    let f = skip_without_db!();
    // ONE version, deliberately: this test is about a pin that names nothing,
    // so a second version would only add a way for the pin to succeed.
    f.publish_with_params(
        "/pin-stale-flood",
        f.database_id,
        no_cache(),
        serde_json::json!({ "per_key_qps": 1, "burst": 1, "per_endpoint_qps": 0 }),
        V1_SQL,
        serde_json::json!([]),
    )
    .await;
    let path = "pin-stale-flood";
    let client = "client-stale-flood";

    let first = f
        .call_as_client_with_headers(f.principal_a, client, path, &[("x-egret-version", "77")])
        .await;
    assert_eq!(first.status, StatusCode::CONFLICT, "{}", first.body);
    assert_eq!(code(&first), "version_unavailable", "{}", first.body);

    let second = f
        .call_as_client_with_headers(f.principal_a, client, path, &[("x-egret-version", "77")])
        .await;
    assert_eq!(
        second.status,
        StatusCode::TOO_MANY_REQUESTS,
        "a stale pin must spend the endpoint's allowance like any other call, \
         or it is an unbounded refusal loop: {}",
        second.body
    );
}

// ---------------------------------------------------------------------------
// The access log
// ---------------------------------------------------------------------------

/// The recorded version is the one that ANSWERED, which is what makes "who is
/// still on an old contract" answerable before an operator retires a version.
///
/// A refused pin records no version, because nothing answered.
#[tokio::test]
async fn the_access_log_records_the_version_that_answered() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-log", no_cache()).await;

    f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        last_logged_version(&f, ep).await,
        Some(1),
        "a pinned call must record the pinned version, not the live one"
    );

    f.call_as(f.principal_a, &path).await;
    assert_eq!(last_logged_version(&f, ep).await, Some(2));

    let refused = f.call_pinned(f.principal_a, &path, "88").await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert_eq!(
        last_logged_version(&f, ep).await,
        None,
        "a refused pin answered from no version and must record none"
    );
}

async fn last_logged_version(f: &Fixture, ep: Uuid) -> Option<i32> {
    sqlx::query_scalar::<_, Option<i32>>(
        "SELECT version FROM dataapi.access_log WHERE endpoint_id = $1 \
         ORDER BY ts DESC, request_id DESC LIMIT 1",
    )
    .bind(ep)
    .fetch_one(&f.db.pool)
    .await
    .expect("read access log")
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Nothing prunes `dataapi.endpoint_versions`, and this is the assertion that
/// says so rather than the comment that hopes so.
///
/// Retention is unbounded by design (see `router::live_snapshot` and the design
/// doc's Package 7 section): a version lives exactly as long as its endpoint, and
/// deleting the endpoint takes the endpoint too — so a pin against it answers
/// `404 not_found`, never a fall-through to live. `version_unavailable` therefore
/// has exactly two causes today, and both are tested above.
///
/// If a pruner is ever added, this test is what will fail, and the refusal it
/// would need already exists.
#[tokio::test]
async fn republishing_does_not_retire_the_versions_a_consumer_may_be_pinned_to() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-retention", no_cache()).await;

    // Five more publishes, so "the old one survived" is not an accident of there
    // having been only two.
    for i in 3..=7 {
        f.set_statements(
            ep,
            serde_json::json!([{ "name": "main", "sql": format!("SELECT {i} AS n /* version one */") }]),
        )
        .await;
        assert_eq!(f.republish(ep).await, i);
    }

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoint_versions WHERE endpoint_id = $1")
            .bind(ep)
            .fetch_one(&f.db.pool)
            .await
            .expect("count versions");
    assert_eq!(rows, 7, "every published version must still be there");

    let res = f.call_pinned(f.principal_a, &path, "1").await;
    assert_eq!(
        res.status,
        StatusCode::OK,
        "the first version must still be servable six publishes later: {}",
        res.body
    );
    assert_eq!(res.body["meta"]["version"], serde_json::json!(1));
    assert_eq!(executed_version(&f), 1);
}

/// A pin against an endpoint that has been destroyed is a 404, not a
/// `version_unavailable` — the endpoint is what is gone.
///
/// `dataapi.endpoint_versions` cascades from `dataapi.endpoints`, so a hard
/// delete takes the versions with it; there is nothing left to be pinned to and
/// nothing to disclose.
#[tokio::test]
async fn a_pin_against_a_destroyed_endpoint_answers_as_an_unknown_url_does() {
    let f = skip_without_db!();
    let (ep, path, _) = two_versions(&f, "/pin-destroyed", no_cache()).await;
    assert_eq!(
        f.call_pinned(f.principal_a, &path, "1").await.status,
        StatusCode::OK
    );

    sqlx::query("DELETE FROM dataapi.endpoints WHERE id = $1")
        .bind(ep)
        .execute(&f.db.pool)
        .await
        .expect("delete endpoint");

    let gone = f.call_pinned(f.principal_a, &path, "1").await;
    let reference = f
        .call_pinned(f.principal_a, "no-such-endpoint-here", "1")
        .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.body);
    assert_eq!(code(&gone), code(&reference), "{}", gone.body);
}

// ---------------------------------------------------------------------------
// The published contract
// ---------------------------------------------------------------------------

/// The per-service-account document declares the header, so a consumer can
/// generate a client that sends it.
///
/// It is `in: header`, which is what makes it safe to add beside an author's own
/// parameters: OpenAPI keys a parameter on `(name, in)`, and an author's names
/// all live in `in: query`. The test publishes an endpoint that declares a query
/// parameter LITERALLY NAMED `version` — publishable on `main` today — so the
/// collision a `?version=` pin would have had is present in the fixture rather
/// than imagined.
#[tokio::test]
async fn the_generated_contract_declares_the_pin_header_beside_an_authors_own_version_parameter() {
    let f = skip_without_db!();
    let sa = f.create_service_account("pin-doc").await;
    let ep = f
        .publish_with_params(
            "/pin-doc",
            f.database_id,
            no_cache(),
            no_limits(),
            "SELECT 1 AS n WHERE tag = {{ version }}",
            serde_json::json!([{ "name": "version", "type": "string", "required": true,
                                 "location": "query", "pattern": "[a-z]+" }]),
        )
        .await;
    serving_common::grant_invoke(&f.db, UserId(sa), ep).await;

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{doc}");

    let params = doc["paths"]["/pin-doc"]["get"]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("no parameters in {doc}"));

    // The author's own `version`, in the query — unchanged by any of this.
    let authors = params
        .iter()
        .find(|p| p["name"] == "version" && p["in"] == "query")
        .unwrap_or_else(|| panic!("the author's own `version` parameter is gone: {doc}"));
    assert_eq!(authors["required"], serde_json::json!(true), "{doc}");

    // And the pin, in the header, alongside it.
    let pin = params
        .iter()
        .find(|p| p["in"] == "header")
        .unwrap_or_else(|| panic!("the pin header is not declared: {doc}"));
    assert_eq!(pin["name"], serde_json::json!("X-Egret-Version"), "{doc}");
    assert_eq!(pin["required"], serde_json::json!(false), "{doc}");
    assert_eq!(pin["schema"]["type"], serde_json::json!("integer"), "{doc}");
    assert_eq!(pin["schema"]["minimum"], serde_json::json!(1), "{doc}");
    assert!(
        pin.get("default").is_none(),
        "a default would pin every generated client forever to whatever was live \
         on the day it was generated: {doc}"
    );
    // The example is this operation's own version, so copying it gets the
    // contract the reader is reading.
    assert_eq!(
        pin["example"], doc["paths"]["/pin-doc"]["get"]["x-egret-version"],
        "{doc}"
    );

    // And the refusal a pinned call can receive is described, derived from
    // `to_wire` rather than hand-listed.
    let responses = &doc["paths"]["/pin-doc"]["get"]["responses"];
    let conflict = responses["409"]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("no 409 response: {doc}"));
    assert!(
        conflict.contains("version_unavailable"),
        "the 409 must name the code a client branches on: {conflict}"
    );
    let bad_request = responses["400"]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("no 400 response: {doc}"));
    assert!(
        bad_request.contains("version_pin_invalid"),
        "the 400 must name the new code too: {bad_request}"
    );
}
