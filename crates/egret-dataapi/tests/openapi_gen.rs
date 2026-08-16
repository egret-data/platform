//! Data API P5 §5 — the generated OpenAPI document a consumer is handed.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

mod serving_common;

use serving_common::fixture;

#[tokio::test]
async fn a_ws_admin_gets_cascade_reachable_paths() {
    let Some(f) = fixture().await else { return };

    // A REAL service account (through the real POST route, never a direct
    // insert), invited as a `ws_admin` member — plan §7.1 records that a
    // service account can be a workspace member today, and that `ws_admin`
    // then carries `data_api_endpoint:invoke` onto every endpoint through the
    // Cedar workspace->object cascade (migration 0124). It holds ZERO
    // object-scoped grants naming any endpoint — asserted below — which is
    // exactly what makes a `access_grants` scan answer "no" while the PDP
    // answers "yes". A generator that scanned the table would emit an EMPTY
    // `paths` object here and pass every other test in this file.
    let sa = f.create_service_account("ws-admin-sa").await;
    f.grant_ws_admin(sa).await;

    let object_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_id = $1 AND scope_type = 'object'",
    )
    .bind(sa)
    .fetch_one(&f.db.pool)
    .await
    .expect("count object grants");
    assert_eq!(
        object_grants, 0,
        "the whole point: cascade-reachable with no object grant"
    );

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);
    assert!(
        doc["paths"].as_object().is_some_and(|p| !p.is_empty()),
        "cascade-reachable endpoints must be documented"
    );
}

#[tokio::test]
async fn servers_and_token_url_share_one_origin() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("origin-check-sa").await;
    let (_, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;

    let server = doc["servers"][0]["url"].as_str().expect("servers url");
    let token_url = doc["components"]["securitySchemes"]["egret_oauth2"]["flows"]
        ["clientCredentials"]["tokenUrl"]
        .as_str()
        .expect("tokenUrl");

    // Fixture issuer is https://egret.test. Both halves must come from it, or a
    // consumer fetches a token from one origin and calls another.
    assert_eq!(server, "https://egret.test/data-api/v1");
    assert!(token_url.starts_with("https://egret.test/"));
}

#[tokio::test]
async fn a_draft_endpoint_is_not_documented() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("draft-check-sa").await;
    let draft = f.create_draft_endpoint("unpublished", "/unpublished").await;
    let (_, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert!(
        !doc["paths"]
            .as_object()
            .unwrap()
            .contains_key("/unpublished"),
        "a draft has no published contract to describe"
    );
    let _ = draft;
}

#[tokio::test]
async fn an_unknown_service_account_id_is_not_found() {
    let Some(f) = fixture().await else { return };
    // `f.admin` is a real principal (a `ws_admin` member) but NOT a row in
    // `dataapi.service_accounts` — exactly the shape `scoped_account` must
    // reject with 404 rather than silently answering 200 with whatever that
    // principal happens to be able to invoke. Without this check, the route
    // would be a content oracle: two different uuids get two different `paths`
    // sets, with no way to tell "no such account" from "not yours".
    let (status, _) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
        ))
        .await;
    assert_eq!(status, 404);
}
