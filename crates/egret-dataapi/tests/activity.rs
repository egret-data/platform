//! Data API P5 §4 — Activity statistics and the call list.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no `DATABASE_URL` FAILS, so check the
//! DURATION, not just the `ok`.

mod serving_common;

use serving_common::fixture;

#[tokio::test]
async fn a_call_that_matches_no_endpoint_still_records_its_url() {
    let Some(f) = fixture().await else { return };

    // A path no endpoint claims: this 404s inside routing, so the row carries
    // no endpoint_id and the two new columns are the only thing identifying it.
    let res = f
        .call_as_client(f.principal_a, "client-a", "no/such/endpoint")
        .await;
    assert_eq!(res.status, 404);

    let row: (
        Option<uuid::Uuid>,
        Option<String>,
        Option<String>,
        Option<uuid::Uuid>,
    ) = sqlx::query_as(
        "SELECT endpoint_id, request_method, request_path, actor_user_id \
         FROM dataapi.access_log WHERE workspace_id = $1 ORDER BY ts DESC LIMIT 1",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("access_log row");

    assert_eq!(row.0, None, "no endpoint should have resolved");
    assert_eq!(row.1.as_deref(), Some("GET"));
    assert_eq!(
        row.2.as_deref(),
        Some("no/such/endpoint"),
        "without this the row is an uncountable 404 with nothing to act on"
    );
    assert_eq!(
        row.3, None,
        "a REAL consumer call must never carry an actor: every statistic, the \
         trial exclusion and the trial-vs-consumer distinction rest on this"
    );
}

/// **`actor_user_id` is a fact, not a claim — the negative half.**
///
/// Only an in-process handler can insert a `TrialActor` extension
/// (`try_it::TrialActor`), which is what makes "this row was a trial" something
/// the caller cannot assert about their own call. Every existing test proves the
/// POSITIVE half: a real trial gets an actor. Nothing proved the negative, and a
/// refactor that read the actor from a request header would keep all forty test
/// binaries green while inverting the property the code comments call the point
/// of the design.
///
/// So this drives a REAL consumer call carrying headers a caller could invent —
/// including the exact `x-egret-trial-actor` shape such a refactor would reach
/// for — and asserts the column is still NULL. It would go red the moment the
/// extension became a header.
#[tokio::test]
async fn a_consumer_cannot_forge_an_actor_with_a_header() {
    let Some(f) = fixture().await else { return };

    let forged = f.admin.to_string();
    let res = f
        .call_as_client_with_headers(
            f.principal_a,
            "client-a",
            "probe",
            &[
                ("x-egret-trial-actor", forged.as_str()),
                ("x-trial-actor", forged.as_str()),
                ("trial-actor", forged.as_str()),
                ("x-egret-actor-user-id", forged.as_str()),
            ],
        )
        .await;
    // Whatever the endpoint answered — the row is written either way, and it is
    // the row this test is about.
    assert!(
        res.status.as_u16() < 500,
        "the call must have been served, not crashed: {} {}",
        res.status,
        res.body
    );

    let actor: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT actor_user_id FROM dataapi.access_log \
         WHERE workspace_id = $1 ORDER BY ts DESC LIMIT 1",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("access_log row");

    assert_eq!(
        actor, None,
        "a header a consumer can send must never become actor_user_id — that is \
         the whole reason TrialActor is an extension and not a header"
    );

    // And the forgery is invisible to the statistics: the call counts as
    // consumer traffic, which it is.
    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["total"]["calls"], 1,
        "a call that claimed to be a trial must still be counted: {body}"
    );
}

#[tokio::test]
async fn ws_admin_can_invoke_without_any_object_grant() {
    let Some(f) = fixture().await else { return };

    // `f.admin` is a ws_admin membership and holds NO object-scoped grant on the
    // endpoint. Migration 0124 puts data_api_endpoint:invoke on that role, and
    // the Cedar workspace->object cascade applies it to every endpoint — so a
    // grant-table scan says "no" here and the PDP says "yes". This test is the
    // difference between those two answers.
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants \
         WHERE principal_id = $1 AND scope_type = 'object'",
    )
    .bind(f.admin)
    .fetch_one(&f.db.pool)
    .await
    .expect("count grants");
    assert_eq!(grants, 0, "fixture must not give the admin an object grant");

    let allowed = egret_dataapi::can_invoke_for_test(
        &f.state,
        egret_core::ids::UserId(f.admin),
        egret_core::WorkspaceId(f.ws_id),
        f.endpoint_id,
    )
    .await
    .expect("pdp answered");

    assert!(
        allowed,
        "ws_admin reaches invoke through the workspace cascade"
    );
}

#[tokio::test]
async fn latency_percentiles_ignore_error_responses() {
    let Some(f) = fixture().await else { return };

    // Three successes at 100ms and seven failures at 1ms. Including the failures
    // drags p50 to 1ms and makes latency look BETTER the more the system fails.
    for _ in 0..3 {
        f.log_row(200, 100, false, None).await;
    }
    for _ in 0..7 {
        f.log_row(401, 1, false, Some("unauthorized")).await;
    }

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);

    assert_eq!(body["total"]["calls"], 10);
    assert_eq!(body["total"]["errors"], 7);
    assert_eq!(
        body["total"]["p50_ms"].as_f64().unwrap().round(),
        100.0,
        "p50 must come from the 2xx population only"
    );
}

#[tokio::test]
async fn trial_runs_are_excluded_from_statistics() {
    let Some(f) = fixture().await else { return };

    f.log_row(200, 10, false, None).await;
    f.log_row_as_trial(200, 10, f.admin).await;

    let (_, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;

    assert_eq!(body["total"]["calls"], 1, "the trial must not be counted");
}

#[tokio::test]
async fn the_unmatched_bucket_is_not_conflated_with_the_grand_total() {
    let Some(f) = fixture().await else { return };

    // Two calls that matched the fixture's real endpoint...
    f.log_row(200, 10, false, None).await;
    f.log_row(200, 10, false, None).await;
    // ...and three calls that never resolved ANY endpoint — endpoint_id NULL,
    // exactly the shape `a_call_that_matches_no_endpoint_still_records_its_url`
    // proves the serving path writes for a genuine 404. Both this bucket and
    // the grand total carry a NULL endpoint_id; only `grouping()` tells them
    // apart — see `ActivityStore::summary`'s doc comment.
    for _ in 0..3 {
        f.log_row_for(None, None, 404, 5, false, Some("not_found"))
            .await;
    }

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);

    // The grand total must count every row, unmatched included — reading NULL
    // as "this is the total" would instead make the total EQUAL the unmatched
    // bucket, or drop one of the two NULL-endpoint_id rows outright.
    assert_eq!(body["total"]["calls"], 5);
    assert_eq!(body["total"]["errors"], 3);

    let endpoints = body["endpoints"].as_array().expect("endpoints array");
    let matched = endpoints
        .iter()
        .find(|r| r["endpoint_id"] == serde_json::json!(f.endpoint_id))
        .expect("the matched endpoint's own row is present");
    assert_eq!(
        matched["calls"], 2,
        "the unmatched rows must not be absorbed into the matched endpoint's row"
    );

    let unmatched = endpoints
        .iter()
        .find(|r| r["endpoint_id"].is_null())
        .expect(
            "the unmatched bucket must appear as its OWN entry in `endpoints`, \
             distinct from `total`",
        );
    assert_eq!(unmatched["calls"], 3);
    assert_eq!(unmatched["errors"], 3);
}

#[tokio::test]
async fn a_request_id_lookup_cannot_read_another_workspace() {
    let Some(f) = fixture().await else { return };

    // A row in ANOTHER workspace, with a request_id the caller knows.
    let other_ws = f.make_workspace("other").await;
    let rid = "01JOTHERWORKSPACEREQUESTID";
    f.log_row_in(other_ws, 500, 5, Some(rid)).await;

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/calls?range=24h&request_id={rid}",
            f.ws_slug
        ))
        .await;

    assert_eq!(status, 200);
    assert_eq!(
        body.as_array().map(|a| a.len()),
        Some(0),
        "access_log_request_id_idx is global on purpose; the QUERY is what scopes it"
    );
}

#[tokio::test]
async fn trial_runs_appear_only_when_asked_for() {
    let Some(f) = fixture().await else { return };
    f.log_row(200, 10, false, None).await;
    f.log_row_as_trial(200, 10, f.admin).await;

    let (_, without) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/calls?range=24h",
            f.ws_slug
        ))
        .await;
    assert_eq!(without.as_array().unwrap().len(), 1);

    let (_, with) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/calls?range=24h&include_trials=true",
            f.ws_slug
        ))
        .await;
    assert_eq!(with.as_array().unwrap().len(), 2);
    let trial = with
        .as_array()
        .unwrap()
        .iter()
        .find(|r| !r["actor_user_id"].is_null())
        .expect("the trial row is present and identifies its actor");
    assert_eq!(trial["actor_user_id"], f.admin.to_string());
}

#[tokio::test]
async fn cacheable_calls_reads_the_versions_own_snapshot_not_the_live_config() {
    let Some(f) = fixture().await else { return };

    // Publish once with caching OFF — this becomes version 1.
    let ep = f
        .publish(
            "/versioned",
            f.database_id,
            serde_json::json!({ "enabled": false }),
            serde_json::json!({}),
        )
        .await;

    // Flip the DRAFT's cache on and republish the SAME endpoint id. Publish
    // freezes whatever `dataapi.endpoints.cache` holds at that moment into the
    // NEW version's snapshot (lifecycle.rs), so this produces version 2 with
    // caching ON while version 1's frozen snapshot still says OFF.
    sqlx::query("UPDATE dataapi.endpoints SET cache = $1 WHERE id = $2")
        .bind(serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" }))
        .bind(ep)
        .execute(&f.db.pool)
        .await
        .expect("flip cache on");
    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", f.ws_slug),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, 200, "republish failed: {body}");

    // One successful call against EACH version. Same endpoint, same status,
    // same duration — the only thing that differs is which version's snapshot
    // the row is attributed to.
    f.log_row_for(Some(ep), Some(1), 200, 10, false, None).await;
    f.log_row_for(Some(ep), Some(2), 200, 10, false, None).await;

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);

    assert_eq!(
        body["total"]["calls"], 2,
        "a join fanning out across an endpoint's several versions would \
         double-count this row and everything else in the total"
    );
    assert_eq!(
        body["total"]["cacheable_calls"], 1,
        "only the version-2 call ran on a cache-enabled snapshot — the version-1 \
         call must NOT count just because the endpoint is cache-on NOW"
    );
}

/// KI-296: a trial row names its actor. The browser used to resolve the id
/// through the platform's user list, which only a platform admin may read, so
/// everyone else saw a shortened id.
#[tokio::test]
async fn a_trial_row_names_its_actor() {
    let Some(f) = fixture().await else { return };
    sqlx::query("UPDATE users SET name = 'Tia Trial' WHERE id = $1")
        .bind(f.admin)
        .execute(&f.db.pool)
        .await
        .unwrap();
    f.log_row(200, 10, false, None).await;
    f.log_row_as_trial(200, 10, f.admin).await;

    let (_, with) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/calls?range=24h&include_trials=true",
            f.ws_slug
        ))
        .await;
    let rows = with.as_array().unwrap();
    let trial = rows
        .iter()
        .find(|r| !r["actor_user_id"].is_null())
        .expect("the trial row is present");
    assert_eq!(trial["actor_name"], "Tia Trial", "{with}");
    let consumer = rows
        .iter()
        .find(|r| r["actor_user_id"].is_null())
        .expect("the consumer row is present");
    assert!(consumer["actor_name"].is_null(), "{with}");
}
