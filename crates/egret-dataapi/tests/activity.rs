//! Data API P5 §4 — Activity statistics and the call list.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

mod serving_common;

use serving_common::fixture;

#[tokio::test]
async fn a_call_that_matches_no_endpoint_still_records_its_url() {
    let Some(f) = fixture().await else { return };

    // A path no endpoint claims: this 404s inside routing, so the row carries
    // no endpoint_id and the two new columns are the only thing identifying it.
    let res = f.call_as_client(f.principal_a, "client-a", "no/such/endpoint").await;
    assert_eq!(res.status, 404);

    let row: (Option<uuid::Uuid>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT endpoint_id, request_method, request_path \
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

    assert!(allowed, "ws_admin reaches invoke through the workspace cascade");
}
