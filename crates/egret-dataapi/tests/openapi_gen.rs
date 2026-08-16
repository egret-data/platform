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
    //
    // The slug is part of the base URL because it is part of the serving ROUTE
    // (`/data-api/v1/{ws}/{*path}`), not of the mount — see
    // `the_documented_url_is_the_url_try_it_calls` below, which is the test that
    // can actually catch its absence. This one deliberately does not: it looks at
    // one half of a URL, and half a URL is what shipped.
    assert_eq!(
        server,
        format!("https://egret.test/data-api/v1/{}", f.ws_slug)
    );
    assert!(token_url.starts_with("https://egret.test/"));
}

/// **The two halves, composed.**
///
/// `servers[0].url` and a `paths` key are never used apart by a consumer: a
/// generated client concatenates them and calls the result. So the assertion has
/// to concatenate them too, and compare against a URL something else independently
/// believes is right — here the one `try_it` builds and drives the REAL serving
/// router with (`try_it.rs`'s `try_endpoint`), for the same endpoint, in the same
/// workspace.
///
/// Neither half alone can see the defect this pins. The sibling above asserts the
/// base; `tests/try_it.rs`'s `the_token_never_reaches_the_caller` asserts the full
/// Try-it URL. Both passed while `servers[0].url` omitted the workspace slug and
/// every documented operation therefore bound `ws` to the first path segment and
/// answered 404.
#[tokio::test]
async fn the_documented_url_is_the_url_try_it_calls() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("url-compose-sa").await;
    // The trial must reach the endpoint for its echo to be the URL of a real
    // call, so the account needs the same invoke grant a consumer would.
    let (granted, gbody) = f
        .mgmt_post(
            &format!(
                "/api/ws/{}/data-api/endpoints/{}/grants",
                f.ws_slug, f.endpoint_id
            ),
            Some(serde_json::json!({ "service_account_id": sa })),
        )
        .await;
    assert_eq!(granted, 200, "grant failed: {gbody}");

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}&endpoint_id={}",
            f.ws_slug, f.endpoint_id
        ))
        .await;
    assert_eq!(status, 200, "doc: {doc}");

    let server = doc["servers"][0]["url"].as_str().expect("servers url");
    let paths = doc["paths"].as_object().expect("paths");
    assert_eq!(paths.len(), 1, "endpoint_id narrows to one path: {doc}");
    let path_key = paths.keys().next().expect("one path key");
    // Exactly what a generated client does with the two halves.
    let documented = format!("{server}{path_key}");

    let (try_status, try_body) = f
        .mgmt_post(
            &format!(
                "/api/ws/{}/data-api/endpoints/{}/try",
                f.ws_slug, f.endpoint_id
            ),
            Some(serde_json::json!({ "service_account_id": sa, "params": {} })),
        )
        .await;
    assert_eq!(try_status, 200, "trial failed: {try_body}");
    assert_eq!(
        try_body["status"], 200,
        "the trial must have really reached the endpoint, or its echoed URL \
         proves nothing: {try_body}"
    );
    let called = try_body["request"]["url"].as_str().expect("try-it url");

    assert_eq!(
        documented, called,
        "a consumer joining servers[0].url to a paths key must land on the URL \
         Try-it just proved serves this endpoint"
    );
}

/// The bound on how many endpoints one document may describe is an ERROR, never
/// a short document — a consumer cannot tell a truncated file from a complete
/// one, and `?endpoint_id=` is the documented way through.
///
/// Asserted against the CONSTANT rather than a literal, so raising the bound does
/// not silently disarm the test; the loop publishes one past it.
#[tokio::test]
async fn more_live_endpoints_than_the_bound_is_refused_not_truncated() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("bound-sa").await;

    // Padded by direct INSERT rather than by `MAX + 1` real publishes: the bound
    // is decided from the row COUNT before any snapshot is parsed or any Cedar
    // call is made, so what matters is that the rows exist, are `live`, and have
    // a live version. The snapshot is a copy of the fixture's own published one,
    // so removing the bound leaves a document that really would be built rather
    // than a decode error standing in for the assertion.
    let snapshot: serde_json::Value = sqlx::query_scalar(
        "SELECT ev.snapshot FROM dataapi.endpoint_versions ev \
         JOIN dataapi.endpoints e ON e.live_version_id = ev.id WHERE e.id = $1",
    )
    .bind(f.endpoint_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("fixture snapshot");

    // The fixture already publishes `/probe`, so `MAX` more reaches `MAX + 1`.
    let pad = egret_dataapi::openapi_gen::MAX_DOCUMENTED_ENDPOINTS as i32;
    sqlx::query(
        "INSERT INTO dataapi.endpoints (workspace_id, name, path, database_id, sql, state) \
         SELECT $1, 'bound-' || i, '/bound-' || i, $2, 'SELECT 1 AS n', 'live' \
         FROM generate_series(1, $3::int) AS i",
    )
    .bind(f.ws_id)
    .bind(f.database_id)
    .bind(pad)
    .execute(&f.db.pool)
    .await
    .expect("pad endpoints");
    sqlx::query(
        "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot) \
         SELECT id, 1, $2 FROM dataapi.endpoints \
         WHERE workspace_id = $1 AND live_version_id IS NULL",
    )
    .bind(f.ws_id)
    .bind(&snapshot)
    .execute(&f.db.pool)
    .await
    .expect("pad versions");
    sqlx::query(
        "UPDATE dataapi.endpoints e SET live_version_id = ev.id \
         FROM dataapi.endpoint_versions ev \
         WHERE ev.endpoint_id = e.id AND e.workspace_id = $1 AND e.live_version_id IS NULL",
    )
    .bind(f.ws_id)
    .execute(&f.db.pool)
    .await
    .expect("link live versions");

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 400, "body: {body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("endpoint_id")),
        "the refusal must name the way through: {body}"
    );

    // Narrowing to one endpoint still works — the bound is on the aggregate.
    let (one, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}&endpoint_id={}",
            f.ws_slug, f.endpoint_id
        ))
        .await;
    assert_eq!(one, 200, "doc: {doc}");
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
