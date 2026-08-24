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

/// **The two audiences, told apart.**
///
/// `data_api.public_url` set: every URL a CONSUMER is handed carries it, while
/// the token `iss` — a claim, not an address — stays on the machine-IdP issuer.
/// Measured on kind 2026-08-20, the whole document advertised
/// `http://egret.egret-system.svc.cluster.local:8080`, which is the Service DNS
/// name engine pods resolve and nothing outside the cluster can.
///
/// The fixture gives the two DIFFERENT hosts on purpose: with them equal (the
/// default, i.e. the key unset) a generator that still read `issuer` would emit
/// `https://egret.test` and pass every assertion below.
///
/// Three separate assertions, because they can fail separately:
/// * `servers[0].url` — where the call goes.
/// * `tokenUrl` — where the credential is exchanged. A test checking only the
///   first passes against a change that moves one and not the other, which is
///   the exact split `openapi_gen.rs`'s own comment warns about: one file
///   sending a consumer to fetch a token from one host and call another.
/// * `token_endpoint`, from the REAL issue-client route in another module,
///   which is the same promise made at a different moment and must not drift
///   from `tokenUrl`.
///
/// And the `iss` of a token this deployment actually mints (`try_it`'s
/// `mint_trial_token`, through the test shim, since a trial token is never
/// echoed to anyone). It must NOT follow: StarRocks byte-compares
/// `required_issuer`, and every already-provisioned engine has the old value
/// baked into its catalog ConfigMap.
#[tokio::test]
async fn consumer_facing_urls_move_to_the_public_url_and_the_iss_claim_does_not() {
    let Some(f) = fixture().await else { return };
    const CONSUMER: &str = "https://api.egret.example";
    let f = f.with_consumer_base_url(CONSUMER);
    let sa = f.create_service_account("consumer-origin-sa").await;

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200, "doc: {doc}");

    let server = doc["servers"][0]["url"].as_str().expect("servers url");
    let token_url = doc["components"]["securitySchemes"]["egret_oauth2"]["flows"]
        ["clientCredentials"]["tokenUrl"]
        .as_str()
        .expect("tokenUrl");
    assert_eq!(
        server,
        format!("{CONSUMER}/data-api/v1/{}", f.ws_slug),
        "the documented call URL must be reachable by the consumer reading it"
    );
    assert_eq!(
        token_url,
        format!("{CONSUMER}/oauth/token"),
        "both halves of the origin come from one value, or the document sends a \
         consumer to two different hosts"
    );

    // The same promise, made when the credential is issued rather than when the
    // document is generated.
    let (issued, client) = f
        .mgmt_post(
            &format!(
                "/api/ws/{}/data-api/service-accounts/{sa}/clients",
                f.ws_slug
            ),
            Some(serde_json::json!({ "name": "consumer-origin-client" })),
        )
        .await;
    assert_eq!(issued, 201, "issue client failed: {client}");
    assert_eq!(
        client["token_endpoint"], token_url,
        "the endpoint handed out with a credential must be the one the document \
         documents: {client}"
    );

    // The claim, which is a different kind of thing and must not have moved.
    let token = egret_dataapi::mint_trial_token_for_test(&f.state, f.ws_id, sa)
        .await
        .expect("mint trial token");
    let claims = claims_of(&token);
    assert_eq!(
        claims["iss"], "https://egret.test",
        "`iss` is byte-compared by StarRocks and baked into provisioned engines; \
         it stays on the machine-IdP issuer: {claims}"
    );
}

/// The payload of a JWT, unverified — the signature is not what is under test.
fn claims_of(token: &str) -> serde_json::Value {
    use base64::Engine as _;
    let payload = token.split('.').nth(1).expect("a JWT has three parts");
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("base64url payload");
    serde_json::from_slice(&raw).expect("claims are JSON")
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

/// **The document is gated on `data_api_endpoint:manage`, not `:read`.**
///
/// It takes a `service_account_id` and answers with exactly the endpoints that
/// account may invoke — the grant roster, per account and iterable over the
/// workspace's service accounts. `/grants` LIST is on `:manage` for precisely
/// that reason (see `routes.rs`'s first module invariant), and this route
/// answers a strictly more powerful version of the same question, so a `:read`
/// holder must not reach it.
///
/// The rest of this file's calls go through `f.mgmt_get`, whose session belongs
/// to a principal holding BOTH `:read` and `:manage` — which is why they keep
/// passing and why none of them could have noticed the gate. This one needs its
/// own principal.
///
/// The mutation that fails this test: put `EP_READ` back on `get_openapi`.
#[tokio::test]
async fn a_read_only_principal_cannot_read_the_generated_document() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("gate-check-sa").await;

    let reader = f.caller_with(&["data_api_endpoint:read"]).await;
    let path = format!(
        "/api/ws/{}/data-api/openapi?service_account_id={sa}",
        f.ws_slug
    );
    let (status, body) = reader.get(&path).await;
    assert_eq!(
        status, 403,
        "`:read` must not enumerate what a service account may invoke: {body}"
    );

    // And the SAME request succeeds for a principal that holds `:manage`, so
    // the 403 above is the gate and not a broken route, a bad session, or a
    // service account this fixture never really created. Without this half, a
    // handler that answered 403 unconditionally would pass.
    let manager = f
        .caller_with(&["data_api_endpoint:read", "data_api_endpoint:manage"])
        .await;
    let (status, body) = manager.get(&path).await;
    assert_eq!(status, 200, "`:manage` must still be served: {body}");
    assert!(body["paths"].is_object(), "{body}");
}

/// **The one failure in this crate whose blast radius is the whole workspace.**
///
/// `data_schema` refuses to document a version whose `output.naming` maps two
/// frozen columns onto one key, because serving answers `503
/// output_naming_collision` for it and a document describing a call that never
/// returns 200 is worse than none. But the refusal is `OpenApiGenError`, which
/// the route turns into a **409 for the entire document** — so one broken
/// endpoint takes every unrelated endpoint's documentation with it. That is the
/// only failure mode here that is not confined to its own endpoint, and it had
/// no test at all.
///
/// # Why the subject is built this way
///
/// `validate()` now refuses a colliding union at publish, so this shape can no
/// longer be created through the route (`tests/lifecycle.rs`). It can still
/// ARRIVE, and that is the whole reason both later refusals must stay: a version
/// frozen BEFORE that rule existed. Reproduced by publishing under `as_is` and
/// rewriting the frozen snapshot in place — the same fixture
/// `serving_routing`'s sibling test uses, and for the same reason.
///
/// # Anti-triviality
///
/// The narrowed request must still be 200. Without it, a generator that refused
/// every document — or a fixture that simply broke the route — satisfies the
/// 409 above, and the test would say nothing about WHICH endpoint caused it.
#[tokio::test]
async fn one_endpoints_naming_collision_fails_the_whole_document() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("collision-sa").await;
    // `ws_admin`, so every endpoint is cascade-reachable and the document really
    // does contain both this endpoint and the fixture's own.
    f.grant_ws_admin(sa).await;

    // Two columns that are distinct under `as_is` and identical under `camel`.
    f.port
        .set_columns_per_sql(&[("", &["total_revenue", "totalRevenue"][..])]);
    let ep = f
        .publish(
            "/collide-doc",
            f.database_id,
            serde_json::json!({ "enabled": false }),
            serde_json::json!({}),
        )
        .await;
    // The refusal names the endpoint by its `name` column, not by its path
    // (`data_schema` is handed `name`), so give it one a human wrote.
    sqlx::query("UPDATE dataapi.endpoints SET name = 'collide-doc' WHERE id = $1")
        .bind(ep)
        .execute(&f.db.pool)
        .await
        .expect("rename");

    // Before the rewrite the document is fine — so the 409 below is caused by
    // `naming`, not by the column pair.
    let (ok, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(ok, 200, "doc: {doc}");
    assert!(
        doc["paths"]
            .as_object()
            .is_some_and(|p| p.contains_key("/collide-doc")),
        "the endpoint must be IN the document before it can break it: {doc}"
    );

    // The pre-rule version, written straight into the frozen snapshot.
    let rewritten = sqlx::query(
        "UPDATE dataapi.endpoint_versions \
            SET snapshot = jsonb_set(snapshot, '{output,naming}', '\"camel\"') \
          WHERE endpoint_id = $1",
    )
    .bind(ep)
    .execute(&f.db.pool)
    .await
    .expect("rewrite the frozen naming")
    .rows_affected();
    assert_eq!(
        rewritten, 1,
        "the rewrite must have hit exactly one version"
    );

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(
        status, 409,
        "one endpoint's collision must fail the whole document, loudly: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("collide-doc") && e.contains("re-publish")),
        "the refusal must name WHICH endpoint and what to do about it — with \
         every other endpoint's document gone, that name is the only lead: \
         {body}"
    );

    // Narrowing to an UNRELATED endpoint still works. This is what makes the
    // 409 a statement about the document rather than about the route, and it is
    // the operator's way to keep reading the rest while the author re-publishes.
    let (one, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}&endpoint_id={}",
            f.ws_slug, f.endpoint_id
        ))
        .await;
    assert_eq!(one, 200, "doc: {doc}");
}
