//! Data API P5 §5 — the generated OpenAPI document a consumer is handed.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

mod serving_common;

use serving_common::fixture;

#[tokio::test]
async fn a_ws_admin_gets_cascade_reachable_paths() {
    let Some(f) = fixture().await else { return };
    // The admin holds no object grant (asserted in Task 2's test); its invoke
    // path is the ws_admin membership and the Cedar cascade. A generator that
    // scanned access_grants would emit an EMPTY paths object here and pass every
    // other test in this file.
    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
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
    let (_, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
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
    let draft = f.create_draft_endpoint("unpublished", "/unpublished").await;
    let (_, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
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
