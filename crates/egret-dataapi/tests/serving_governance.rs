//! Package 1 — the governance gate, through both real surfaces.
//!
//! The defect: a served request runs the SQL as the SERVICE ACCOUNT unless the
//! connection is delegated or impersonating, so on a `governed` database the
//! engine's row filters and column masks are evaluated at machine-identity
//! granularity and every consumer reads what the machine may read. Nothing
//! required a delegated connection before this package.
//!
//! Two halves, and they are in separate tests on purpose: with only one of them,
//! reverting the other check alone would leave a green suite and the gate would
//! look closed when half of it was. That is the same reason `serving_cache.rs`
//! splits its pair, and it is the failure mode this repo has actually shipped.
//!
//! # What each test does to keep itself from passing vacuously
//!
//! Every refusal here is bracketed by a case that must NOT be refused, differing
//! in exactly one field. A fixture that could never reach the unsafe state
//! proves nothing, and neither does one that can never leave it: the flip back
//! is what shows the rule reads the live column rather than latching.
//!
//! DB-gated; self-skips when `DATABASE_URL` is unset — check the DURATION, not
//! the `ok`.

mod serving_common;

use axum::http::StatusCode;
use serving_common::fixture;

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
    serde_json::json!({})
}

fn per_principal_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" })
}

fn no_limits() -> serde_json::Value {
    serde_json::json!({})
}

/// The publish half.
///
/// The connection is the ONLY thing wrong with this draft — the SQL is the same
/// `SELECT 1 AS n` every other publish in these suites uses — so the refusal
/// cannot be an artefact of some other rule answering first.
#[tokio::test]
async fn a_governed_database_cannot_carry_an_endpoint_on_a_service_account_connection() {
    let f = skip_without_db!();

    // Anti-triviality, FIRST: the same draft on the same connection publishes
    // while the database is ungoverned. Without this the assertion below would
    // hold with the rule deleted and something else refusing.
    let (status, body) = f
        .try_publish("/gov-before", f.database_id, no_cache())
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the fixture's own draft must publish while nothing is governed: {body}"
    );

    // One column, on the database, and nothing about the draft changes.
    f.set_database_governed(f.database_id, true).await;

    let (status, body) = f.try_publish("/gov-after", f.database_id, no_cache()).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a governed database on a connection that runs every query as the \
         service account must not publish — the engine would filter for the \
         machine identity, not for the consumer: {body}"
    );
    let text = body.to_string();
    // The message must name BOTH ways out, for the same reason the shared-cache
    // refusal does: an author told only "delegated" on a connection whose
    // `options.impersonate` is the thing they can actually change has been sent
    // to the wrong setting.
    assert!(
        text.contains("governed"),
        "the refusal must name the flag that caused it: {body}"
    );
    assert!(
        text.contains("delegated") && text.contains("impersonate"),
        "the refusal must name both ways to satisfy it: {body}"
    );

    // And the two escape hatches both work, one field apart. `delegated` first…
    f.set_database_governed(f.delegated_database_id, true).await;
    let (status, body) = f
        .try_publish("/gov-delegated", f.delegated_database_id, no_cache())
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a governed database with a delegated connection is exactly the \
         configuration this gate exists to require: {body}"
    );
    // …then `options.impersonate` alone, which is the half a reader who knows
    // only `delegated` gets wrong. `impersonating_database_id` is
    // `delegated: false, impersonate: true`.
    f.set_database_governed(f.impersonating_database_id, true)
        .await;
    let (status, body) = f
        .try_publish(
            "/gov-impersonating",
            f.impersonating_database_id,
            no_cache(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`options.impersonate` makes the engine run every query as the caller \
         independently of `delegated`, so it satisfies the gate on its own: {body}"
    );
}

/// The serving half, and the drift case the design turns on.
///
/// This endpoint was published legitimately: at publish time the database was
/// ungoverned, so no rule had anything to say. Marking the database governed
/// afterwards is the edit that makes it unsafe, and **nothing re-validates a
/// published version**.
///
/// Note which columns move: only `governed`. `delegated` and
/// `options.impersonate` are untouched, so the frozen `(delegated, impersonate)`
/// witness pair that `shared_cache_allowed` compares still AGREES. A
/// witness-shaped check would stay silent here — which is why this one evaluates
/// the live predicate instead.
#[tokio::test]
async fn marking_a_database_governed_stops_an_already_published_endpoint() {
    let f = skip_without_db!();
    f.publish("/drift", f.database_id, no_cache(), no_limits())
        .await;

    // Anti-triviality: it serves. Otherwise the 503 below could be the endpoint
    // having been broken all along.
    let before = f.call_as(f.principal_a, "drift").await;
    assert_eq!(before.status, StatusCode::OK, "{}", before.body);

    f.set_database_governed(f.database_id, true).await;

    let after = f.call_as(f.principal_a, "drift").await;
    assert_eq!(
        after.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "governed data must not be read under the service account's policy just \
         because the version predates the flag: {}",
        after.body
    );
    assert_eq!(
        after.body["error"]["code"], "governance_stale",
        "the code is the consumer's branching key and must not collapse into \
         `cache_scope_stale`, whose fix (re-publish) does not clear this: {}",
        after.body
    );
    assert!(
        after.body.get("data").is_none(),
        "a refusal must not carry rows: {}",
        after.body
    );
    assert!(
        after.retry_after.is_some(),
        "a config-drift 503 states a floor so clients do not hammer a state only \
         a human clears: {:?}",
        after.retry_after
    );

    // It reads the live column rather than latching: clearing the flag restores
    // service with no re-publish. A check that had frozen anything would not.
    f.set_database_governed(f.database_id, false).await;
    assert_eq!(
        f.call_as(f.principal_a, "drift").await.status,
        StatusCode::OK,
        "the rule tracks the live column"
    );
}

/// The other direction of the same drift, and the one a witness pair WOULD
/// catch — kept because the two edits have different fixes and an operator must
/// be able to reach either from the same message.
#[tokio::test]
async fn turning_delegation_off_under_a_governed_database_stops_serving() {
    let f = skip_without_db!();
    f.set_database_governed(f.delegated_database_id, true).await;
    f.publish(
        "/undelegated",
        f.delegated_database_id,
        no_cache(),
        no_limits(),
    )
    .await;

    let before = f.call_as(f.principal_a, "undelegated").await;
    assert_eq!(before.status, StatusCode::OK, "{}", before.body);

    f.set_database_delegated(f.delegated_database_id, false)
        .await;

    let after = f.call_as(f.principal_a, "undelegated").await;
    assert_eq!(
        after.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "the connection stopped running queries as the caller while the data \
         stayed governed: {}",
        after.body
    );
    assert_eq!(after.body["error"]["code"], "governance_stale");
}

/// **The check is before the cache lookup, and a warm entry must not answer for
/// it.**
///
/// Without this ordering the flip would take effect only when the TTL expired,
/// so an endpoint would keep serving governed rows read under the service
/// account for as long as its author configured — the failure being cached is
/// worse than the failure, because it is invisible and it has a duration.
#[tokio::test]
async fn a_warm_cache_entry_does_not_outlive_the_flip() {
    let f = skip_without_db!();
    f.publish("/cached", f.database_id, per_principal_cache(), no_limits())
        .await;

    assert_eq!(
        f.call_as(f.principal_a, "cached").await.status,
        StatusCode::OK
    );
    // The entry is warm — asserted, not assumed. If it were not, the 503 below
    // would prove nothing about ordering.
    let warm = f.call_as(f.principal_a, "cached").await;
    assert_eq!(warm.status, StatusCode::OK, "{}", warm.body);
    assert!(
        warm.cached(),
        "the second call must be a cache HIT, or this test is not about the \
         cache at all: {}",
        warm.body
    );

    f.set_database_governed(f.database_id, true).await;

    let after = f.call_as(f.principal_a, "cached").await;
    assert_eq!(
        after.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a warm entry answered after the database became governed: {}",
        after.body
    );
    assert_eq!(after.body["error"]["code"], "governance_stale");
}

/// The message's own claim, tested.
///
/// `governance_stale` deliberately does NOT tell an operator to re-publish,
/// unlike every other config-drift 503 — because re-publishing is refused under
/// exactly the condition that produced it. If the publish gate and the serving
/// gate ever disagreed about that, the message would be sending someone to do
/// something that cannot work, which is the one failure a drift message must not
/// have. This pins the two together.
#[tokio::test]
async fn republishing_does_not_clear_a_governance_refusal() {
    let f = skip_without_db!();
    let ep = f
        .publish("/norepub", f.database_id, no_cache(), no_limits())
        .await;
    f.set_database_governed(f.database_id, true).await;
    assert_eq!(
        f.call_as(f.principal_a, "norepub").await.body["error"]["code"],
        "governance_stale"
    );

    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", f.ws_slug),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "re-publishing must be refused by the same rule, so the serving message \
         is right not to offer it as a fix: {body}"
    );
    assert!(
        body.to_string().contains("governed"),
        "and it must be refused for the SAME reason, not incidentally: {body}"
    );
}

/// **Rollback re-asks the two rules whose inputs are LIVE, so a version that
/// `publish` would refuse cannot be made live by the back door.**
///
/// `rollback` writes `live_version_id` and nothing else, and it used to
/// re-validate nothing at all. So the sequence below — publish twice while the
/// database is ordinary, mark it governed, roll back — put a version live that
/// `publish` refuses, whereupon every consumer got `503 governance_stale`
/// forever and the explanation appeared nowhere near the act that caused it.
/// The operator's next move is a DIFFERENT page (the database's), and nothing
/// told them so.
///
/// Bracketed on both sides: the same rollback succeeds before the flip and
/// again after it is cleared, so the 409 is about the flag and not about the
/// rollback route being broken.
#[tokio::test]
async fn rollback_refuses_a_version_publish_would_refuse() {
    let f = skip_without_db!();
    let ep = f
        .publish("/rollback-gov", f.database_id, no_cache(), no_limits())
        .await;
    // A second version of the SAME endpoint, so there is something to roll back
    // FROM. `publish` on an existing endpoint id freezes v2.
    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", f.ws_slug),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "second publish: {body}");
    assert_eq!(body["version"].as_i64(), Some(2), "{body}");

    let roll_back_to_v1 = || async {
        f.mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/rollback", f.ws_slug),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await
    };

    // Anti-triviality FIRST: the rollback itself works.
    let (status, body) = roll_back_to_v1().await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the rollback must work while nothing is governed, or the refusal below \
         proves nothing: {body}"
    );
    assert_eq!(body["version"].as_i64(), Some(1), "{body}");

    // ONE column, on the database.
    f.set_database_governed(f.database_id, true).await;

    let (status, body) = roll_back_to_v1().await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a version that `publish` refuses must not be reachable by rollback — \
         it would 503 on every request with the explanation arriving nowhere \
         near the act: {body}"
    );
    let text = body.to_string();
    assert!(
        text.contains("governed"),
        "the refusal must name the flag that caused it, at the point of action: {body}"
    );
    assert!(
        text.contains("delegated") && text.contains("impersonate"),
        "and both ways out, exactly as publish names them: {body}"
    );

    // It reads the live column rather than latching: clear the flag and the same
    // rollback goes through.
    f.set_database_governed(f.database_id, false).await;
    let (status, body) = roll_back_to_v1().await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The same gap for the OTHER live-input rule, which the reviewer asked be
/// closed together or explained: a `shared`-cache version published on a
/// connection that has since become per-principal.
///
/// `check_cache_scope` refuses that at publish; serving refuses it with
/// `cache_scope_stale`. Rollback used to make it live regardless, which is the
/// same "explanation nowhere near the action" shape as the test above — and
/// leaving one of the pair open would have taught the next reader that rollback
/// re-validates, which would then be half true.
#[tokio::test]
async fn rollback_refuses_a_shared_cache_version_on_a_now_per_principal_connection() {
    let f = skip_without_db!();
    let shared_cache = serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "shared" });
    let ep = f
        .publish("/rollback-cache", f.database_id, shared_cache, no_limits())
        .await;
    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", f.ws_slug),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "second publish: {body}");

    let roll_back_to_v1 = || async {
        f.mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/rollback", f.ws_slug),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await
    };

    let (status, body) = roll_back_to_v1().await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the rollback works to begin with: {body}"
    );

    // The connection starts running queries as the caller. The frozen `shared`
    // scope is now a cross-principal cache.
    f.set_database_delegated(f.database_id, true).await;

    let (status, body) = roll_back_to_v1().await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a `shared` cache on a connection the engine now filters per principal \
         is one principal's rows served to another; publish refuses it and \
         rollback must not be the way around: {body}"
    );
    assert!(
        body.to_string().contains("shared"),
        "the refusal must name the scope that caused it: {body}"
    );

    f.set_database_delegated(f.database_id, false).await;
    let (status, body) = roll_back_to_v1().await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
