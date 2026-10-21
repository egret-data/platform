//! Data API P5 §5 — the generated OpenAPI document a consumer is handed.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no `DATABASE_URL` FAILS, so check the
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

/// KI-023: a draft whose path moved has not moved what is served, so neither
/// the document a consumer is handed nor the URL Try it calls may follow it.
/// Both must name the LIVE route until the move is published.
#[tokio::test]
async fn a_pending_draft_path_is_not_documented_or_tried() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("pending-path-sa").await;
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
    // What `PATCH …/endpoints/{id}` writes for a new path.
    sqlx::query("UPDATE dataapi.endpoints SET path = '/moved' WHERE id = $1")
        .bind(f.endpoint_id)
        .execute(&f.db.pool)
        .await
        .expect("move the draft");

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}&endpoint_id={}",
            f.ws_slug, f.endpoint_id
        ))
        .await;
    assert_eq!(status, 200, "doc: {doc}");
    let paths: Vec<&String> = doc["paths"].as_object().expect("paths").keys().collect();
    assert_eq!(
        paths,
        vec!["/probe"],
        "the document names the live path: {doc}"
    );

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
        "the trial reaches the live version: {try_body}"
    );
    let called = try_body["request"]["url"].as_str().expect("try-it url");
    assert!(
        called.ends_with("/probe"),
        "Try it calls the live URL: {called}"
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
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, database_id, statements, state) \
         SELECT $1, 'bound-' || i, '/bound-' || i, $2, \
                jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1 AS n')), 'live' \
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
        // The live route is set with the pointer, as publish sets it.
        "UPDATE dataapi.endpoints e SET live_version_id = ev.id, \
                live_method = e.method, live_path = e.path \
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

/// Helper: the operation object a consumer would read for `path`.
fn operation<'a>(doc: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    let op = &doc["paths"][path]["get"];
    assert!(op.is_object(), "no operation for {path} in {doc}");
    op
}

/// The keys of a JSON object, in order.
fn keys(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected an object, got {v}"))
        .keys()
        .cloned()
        .collect()
}

/// **The document, checked against the body the server actually produces.**
///
/// This is the assertion the task exists for. A test that reads only the
/// generator's own output agrees with the generator's own bug — and that is
/// precisely what happened here: `openapi_gen` described `{data: array, meta}`
/// for every endpoint while Task 8's server answered `{status, message, data,
/// meta}` with `data` keyed by statement name. Every generator-only test in this
/// file passed throughout.
///
/// So both halves are measured in one test: the document is fetched from the
/// real management route, the SAME two endpoints are driven through the real
/// serving router, and the top-level keys, the `data` keys and the `meta` keys
/// are compared. Nothing is compared against a literal the generator could also
/// be wrong about.
#[tokio::test]
async fn the_documented_body_is_the_body_the_endpoint_answers_with() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("body-shape-sa").await;
    f.grant_ws_admin(sa).await;

    // Distinct columns per statement, installed BEFORE publish: this is the
    // table publish describes and freezes. First arm wins, so the fallback last.
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);
    f.publish_statements(
        "/doc-two",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
    )
    .await;
    f.publish_statements(
        "/doc-one",
        f.database_id,
        serde_json::json!([{ "name": "rows", "sql": "SELECT id, name FROM t" }]),
        serde_json::json!({}),
    )
    .await;

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200, "{doc}");

    // ---- several statements -------------------------------------------------
    let schema =
        &operation(&doc, "/doc-two")["responses"]["200"]["content"]["application/json"]["schema"];
    let res = f.call_as(f.principal_a, "doc-two").await;
    assert_eq!(res.status.as_u16(), 200, "{}", res.body);

    assert_eq!(
        schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("no required list in {schema}"))
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect::<Vec<_>>(),
        keys(&res.body),
        "every key the document requires is a key the response carries, and no \
         other: {schema} vs {}",
        res.body
    );
    assert_eq!(
        keys(&schema["properties"]["data"]["properties"]),
        keys(&res.body["data"]),
        "`data` is documented with exactly the statement names it answers under: \
         {schema} vs {}",
        res.body
    );
    assert_eq!(
        keys(&schema["properties"]["meta"]["properties"]["statements"]["properties"]),
        keys(&res.body["meta"]["statements"]),
        "`meta.statements` likewise: {schema} vs {}",
        res.body
    );
    assert_eq!(
        keys(&schema["properties"]["meta"]["properties"]),
        keys(&res.body["meta"]),
        "and `meta` itself carries no field the document omits: {schema} vs {}",
        res.body
    );
    assert_eq!(
        schema["properties"]["data"]["properties"]["total"]["items"]["properties"]
            .as_object()
            .map(|_| keys(
                &schema["properties"]["data"]["properties"]["total"]["items"]["properties"]
            ))
            .expect("the total statement's row schema"),
        keys(&res.body["data"]["total"][0]),
        "each statement's row schema is its OWN columns: {schema} vs {}",
        res.body
    );
    assert!(
        schema["properties"]["status"]["enum"]
            .as_array()
            .expect("status enum")
            .contains(&res.body["status"]),
        "the served `status` is one the document allows: {schema} vs {}",
        res.body
    );

    // ---- one statement ------------------------------------------------------
    let schema =
        &operation(&doc, "/doc-one")["responses"]["200"]["content"]["application/json"]["schema"];
    let res = f.call_as(f.principal_a, "doc-one").await;
    assert_eq!(res.status.as_u16(), 200, "{}", res.body);
    assert_eq!(
        schema["properties"]["data"]["type"], "array",
        "one statement answers with the rows array itself: {schema}"
    );
    assert!(res.body["data"].is_array(), "{}", res.body);
    assert_eq!(
        keys(&schema["properties"]["data"]["items"]["properties"]),
        keys(&res.body["data"][0]),
        "{schema} vs {}",
        res.body
    );
    assert_eq!(
        keys(&schema["properties"]["meta"]["properties"]),
        keys(&res.body["meta"]),
        "one statement's `meta` is the endpoint-wide contract: {schema} vs {}",
        res.body
    );
}

/// §4.5's sentence, on the real operation of a real several-statement endpoint,
/// and absent from the one-statement endpoint beside it.
///
/// The negative half is not decoration: a generator that appended the sentence
/// unconditionally would satisfy the positive half while telling every
/// single-statement consumer their one read is inconsistent with itself.
#[tokio::test]
async fn a_several_statement_operation_discloses_that_it_shares_no_transaction() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("consistency-sa").await;
    f.grant_ws_admin(sa).await;

    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);
    f.publish_statements(
        "/disclose-two",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
    )
    .await;
    f.publish_statements(
        "/disclose-one",
        f.database_id,
        serde_json::json!([{ "name": "rows", "sql": "SELECT id, name FROM t" }]),
        serde_json::json!({}),
    )
    .await;

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200, "{doc}");

    let several = operation(&doc, "/disclose-two")["description"]
        .as_str()
        .unwrap_or_else(|| panic!("no description in {doc}"))
        .to_string();
    assert!(
        several.contains("no transaction") && several.contains("no snapshot"),
        "the operation must say the statements share neither: {several}"
    );
    assert!(
        several.contains("may disagree"),
        "and what that costs the caller, in plain words: {several}"
    );

    let one = operation(&doc, "/disclose-one")["description"]
        .as_str()
        .unwrap_or_else(|| panic!("no description in {doc}"))
        .to_string();
    assert!(
        !one.contains("no transaction"),
        "a one-statement endpoint has nothing to disclaim: {one}"
    );
}

/// A `bare` endpoint is documented as the body it answers with — `data`'s value
/// alone — while its REFUSALS keep the error envelope, which is what the server
/// does (`Served::into_response` writes `self.response.data`; the error path is
/// untouched by `envelope`).
///
/// Both halves are read off real responses.
/// **Every status the taxonomy can answer with is declared in the document a
/// consumer generates a client from.**
///
/// # The finding
///
/// There are TWO OpenAPI documents. The root `utoipa` one, guarded by the
/// `openapi.json` byte gate, and this one — `openapi_gen::build`, the
/// per-service-account contract a consumer downloads. Package 3b made `502
/// engine_unavailable` and `504 timeout` live on the serving surface and
/// corrected only the first. This document still declared
/// `400,401,403,404,429,500,503`, so a generated client had no case for the
/// status it receives the moment the Trino coordinator goes down: the very
/// "the caller cannot act on it" defect the package exists to remove, in the
/// more consumer-facing of the two artefacts.
///
/// The byte gate cannot see this. It compares the ROOT document to the
/// committed copy of itself and passes precisely because nothing changed.
///
/// # Why this test cannot go stale the way the list did
///
/// The expectation is not written here. `error::wire_answers()` derives it by
/// applying `to_wire` to the taxonomy census, which is itself checked against
/// `ServingError`'s own source — so a variant added later reaches this
/// assertion without anyone editing it, and fails until the document declares
/// its status. `error_responses()` is built from the same derivation, which is
/// what makes the two agree by construction rather than by memory; this test is
/// what fails if someone ever hand-writes the list again.
///
/// Asserted on a REAL built document (through the management route, on a real
/// live endpoint), not on `error_responses()` in isolation: the responses map
/// being right and the operation carrying it are two different facts, and the
/// second one is the one a consumer reads.
#[tokio::test]
async fn the_consumer_document_declares_every_status_the_surface_can_answer() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("status-list-sa").await;
    f.grant_ws_admin(sa).await;
    f.publish(
        "/status-list",
        f.database_id,
        serde_json::json!({}),
        serde_json::json!({}),
    )
    .await;

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200, "{doc}");
    let op = operation(&doc, "/status-list");
    let responses = &op["responses"];

    let answers = egret_dataapi::serving::error::wire_answers();
    // A floor, not a count of today's taxonomy: its only job is to fail loudly
    // if the derivation ever returns nothing, because an empty expectation
    // agrees with every document there is.
    assert!(
        answers.len() >= 7,
        "the taxonomy derivation found only {} statuses, which means it stopped \
         working rather than that the surface shrank: {answers:?}",
        answers.len()
    );

    for (http_status, codes) in &answers {
        let declared = &responses[&http_status.to_string()];
        let description = declared["description"].as_str().unwrap_or_else(|| {
            panic!(
                "the serving surface answers {http_status} with {codes:?}, and the \
                 document a consumer generates a client from has no {http_status} \
                 response at all. A client generated from this file has no case \
                 for a status it will receive. Responses declared: {:?}",
                responses.as_object().map(|o| o.keys().collect::<Vec<_>>())
            )
        });
        for code in codes {
            assert!(
                description.contains(code),
                "{http_status} is declared but its description does not name \
                 `{code}` — `error.code` is the stable thing a consumer branches \
                 on, and two of these statuses carry two codes each. Got: \
                 {description:?}"
            );
        }
        assert!(
            declared["content"]["application/json"]["schema"]["$ref"]
                .as_str()
                .is_some_and(|r| r.ends_with("ServingErrorBody")),
            "{http_status} must carry the shared error envelope: {declared}"
        );
    }

    // And the success answer is still there beside them — a responses map built
    // purely from the taxonomy would pass everything above.
    assert!(
        responses["200"].is_object(),
        "the success answer must survive: {op}"
    );

    // The other control, in the opposite direction: a status the taxonomy
    // cannot produce must be absent. Without it, a builder that declared every
    // status there is would satisfy the loop above as easily as a derived one.
    assert!(
        responses.get("418").is_none(),
        "418 is not in the taxonomy, so the document must not declare it — a \
         document that listed everything would pass the loop above: {responses}"
    );
}

#[tokio::test]
async fn a_bare_endpoint_is_documented_as_the_body_it_answers_with() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("bare-doc-sa").await;
    f.grant_ws_admin(sa).await;

    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);
    f.publish_statements_with_output(
        "/bare-doc",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
        serde_json::json!({ "envelope": "bare" }),
    )
    .await;

    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200, "{doc}");
    let op = operation(&doc, "/bare-doc");
    let schema = &op["responses"]["200"]["content"]["application/json"]["schema"];

    let res = f.call_as(f.principal_a, "bare-doc").await;
    assert_eq!(res.status.as_u16(), 200, "{}", res.body);
    // The served body IS the keyed object — no wrapper at all.
    assert!(res.body.get("status").is_none(), "{}", res.body);
    assert_eq!(
        keys(&schema["properties"]),
        keys(&res.body),
        "a bare endpoint's 200 schema is `data`'s value: {schema} vs {}",
        res.body
    );
    assert!(
        schema["properties"].get("meta").is_none(),
        "nothing is wrapped around it: {schema}"
    );

    // The refusal is still the error envelope, under its real status — and the
    // document's shared `ServingErrorBody` must be that body, not the detail
    // object it nests. Driven as a principal with no invoke grant.
    let refused = f.call_as(f.principal_b, "no-such-path-at-all").await;
    assert_eq!(refused.status.as_u16(), 404, "{}", refused.body);
    let err = &doc["components"]["schemas"]["ServingErrorBody"];
    assert_eq!(
        keys(&err["properties"]),
        keys(&refused.body),
        "the documented error body is the error body served: {err} vs {}",
        refused.body
    );
    // `error` is a `$ref`, so its own properties live in the component it names —
    // reading `err["properties"]["error"]["properties"]` finds null, not the detail.
    assert_eq!(
        err["properties"]["error"]["$ref"].as_str(),
        Some("#/components/schemas/ServingErrorDetail"),
        "the detail is referenced, not inlined: {err}"
    );
    let detail = &doc["components"]["schemas"]["ServingErrorDetail"];
    assert_eq!(
        keys(&detail["properties"]),
        keys(&refused.body["error"]),
        "including the nested detail: {detail} vs {}",
        refused.body
    );
    assert!(
        op["responses"]["404"]["content"]["application/json"]["schema"]["$ref"]
            .as_str()
            .is_some_and(|r| r.ends_with("ServingErrorBody")),
        "a bare endpoint's refusals still reference it: {op}"
    );
}

/// **The generated document is never stored, on this route either.**
///
/// The bearer-reachable contract (`serving::contract`) carries
/// `Cache-Control: no-store` and states why: the document varies by service
/// account, so a shared cache hands one consumer another's. This route is the
/// more sensitive of the two — it is `:manage`-gated precisely because it
/// answers that question **per account and iterable** — and it returned no
/// cache header at all.
///
/// Being behind a session is not a substitute. `Cache-Control` governs
/// intermediaries and the browser's own disk cache, both downstream of
/// authorization and neither of which knows the response is per account.
///
/// Mutation: drop the header tuple from `get_openapi`'s `Ok(...)` and this is
/// `None`.
#[tokio::test]
async fn the_management_document_is_never_stored_either() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("no-store-sa").await;

    let (status, cache_control) = f
        .mgmt_get_cache_control(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={sa}",
            f.ws_slug
        ))
        .await;

    // The 200 first: a 403 or a 404 would carry no header either, and would
    // make the assertion below pass for the wrong reason.
    assert_eq!(status, 200);
    assert_eq!(
        cache_control.as_deref(),
        Some("no-store"),
        "a per-service-account document must not be stored by anything between \
         egret and the admin reading it"
    );
}
