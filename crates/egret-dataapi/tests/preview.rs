//! `POST …/endpoints/{id}/preview` — spec §5 of the P6 authoring-loop design.
//!
//! DB-gated. Self-skips (passes) when DATABASE_URL is unset, like every other
//! suite here — so check the DURATION, not just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use egret_core::UserId;
use serving_common::{bind_signature, fixture, marker};

fn preview_path(f: &serving_common::Fixture) -> String {
    format!(
        "/api/ws/{}/data-api/endpoints/{}/preview",
        f.ws_slug, f.endpoint_id
    )
}

/// The body every test below starts from: a draft with no parameters.
fn draft(f: &serving_common::Fixture, sql: &str) -> serde_json::Value {
    serde_json::json!({
        "sql": sql,
        "params": [],
        "values": {},
        "database_id": f.database_id,
    })
}

/// Spec §5: **preview runs as the author, not the service account.**
///
/// `FakePort` folds the principal into the value it returns (`marker`), the way
/// Ranger filters per principal. A preview that ran as a service account would
/// be `Try it` wearing the wrong label — and the author would be told their SQL
/// works against rows they cannot actually see.
#[tokio::test]
async fn preview_runs_as_the_author() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0][0].as_i64(),
        Some(marker(UserId(f.author))),
        "preview must run as the session's own user, not a service account"
    );
}

/// Spec §5: **preview reflects unsaved SQL.**
///
/// The property the request body exists for. It fails the moment anyone
/// "simplifies" the handler into reading the stored draft — at which point the
/// feature is gone and every other test here still passes.
#[tokio::test]
async fn preview_runs_the_body_not_the_stored_draft() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(
            &preview_path(&f),
            Some(draft(&f, "SELECT 42 AS unsaved_marker")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (sql, _) = f.port.last();
    assert!(
        sql.contains("unsaved_marker"),
        "the engine was sent {sql:?}, not the body's SQL"
    );
}

/// The row cap is PREVIEW's, not the endpoint's.
///
/// The published draft carries `limits.max_rows = 10000` (the default nothing in
/// this fixture overrides), so reading 100 here says the preview cap is fixed
/// rather than inherited — the difference between "the shape of the data" and a
/// million rows in a browser. The timeout is asserted in the same breath because
/// it is the other half of the same fixed budget, and a route that took its
/// `limits` from the snapshot would get both wrong at once.
#[tokio::test]
async fn preview_caps_at_a_hundred_rows_regardless_of_the_endpoints_limits() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (max_rows, timeout) = f.port.last_limits();
    assert_eq!(max_rows, 100, "the preview cap is fixed at 100 rows");
    assert_eq!(timeout, std::time::Duration::from_secs(30));
}

/// Spec §5: **preview writes no `access_log` row.**
///
/// Activity measures real traffic. A preview row would inflate call counts and
/// skew every percentile on that page.
#[tokio::test]
async fn preview_writes_no_access_log_row() {
    let Some(f) = fixture().await else { return };
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE workspace_id = $1")
            .bind(f.ws_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE workspace_id = $1")
            .bind(f.ws_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(before, after, "a preview must not appear in Activity");
}

/// Spec §5: **preview does not populate the cache.**
///
/// A cached preview would poison what consumers read with a result produced
/// under a different principal — the author's, not the service account's.
///
/// # Why this endpoint and not the fixture's own `/probe`
///
/// `/probe` publishes `cache.enabled = false`, and `serving/router.rs` skips the
/// lookup entirely for a snapshot that does not cache — so a preview could write
/// whatever it liked and `meta.cached` would still read `false`. The assertion
/// would pass for a reason that has nothing to do with preview.
///
/// So this publishes a **cache-enabled, `shared`-scope** endpoint instead. Shared
/// is the scope whose serving key carries no principal (`cache_key`'s `None`
/// arm), which is what makes an entry a preview wrote reachable by a consumer's
/// key at all: a `per_principal` entry written under the AUTHOR could never be
/// hit by a call made as `principal_a`, so the test could not fail however wrong
/// the route was. `shared` is publishable here because `database_id` is neither
/// delegated nor impersonating.
#[tokio::test]
async fn preview_does_not_populate_the_cache() {
    let Some(f) = fixture().await else { return };
    let ep = f
        .publish(
            "/cached-probe",
            f.database_id,
            serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "shared" }),
            serde_json::json!({}),
        )
        .await;
    let path = format!("/api/ws/{}/data-api/endpoints/{ep}/preview", f.ws_slug);
    let (status, body) = f.mgmt_post(&path, Some(draft(&f, "SELECT 1 AS n"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let res = f.call_as(f.principal_a, "cached-probe").await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(
        res.body["meta"]["cached"].as_bool(),
        Some(false),
        "a consumer's first call must be a MISS — a preview must not seed the cache"
    );
}

/// Spec §5: **preview refuses what publish refuses**, and with the same error —
/// which is what proves it did not get a render path of its own.
#[tokio::test]
async fn preview_refuses_a_definition_publish_refuses() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(
            &preview_path(&f),
            Some(draft(&f, "SELECT 1 AS n; DROP TABLE t")),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a second statement must be refused: `:manage` is not engine write access — {body}"
    );
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");
}

/// The other half of that rule, and the half nothing else re-runs: a single
/// statement that is not a SELECT.
///
/// `check_params` re-runs the separator scan on purpose, so the test above stays
/// green even with `check_single_select` out of the bundle. A bare `DELETE`
/// carries no separator, so this is the assertion that says `:manage` is not
/// engine WRITE access.
#[tokio::test]
async fn preview_refuses_a_statement_that_is_not_a_select() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(
            &preview_path(&f),
            Some(draft(&f, "DELETE FROM t WHERE 1 = 1")),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");
}

/// Spec §5: **preview surfaces the engine's message**, not `data_access_denied`.
///
/// The one place serving's sanitising rule is deliberately inverted. An author
/// owns the SQL, runs as themselves, and cannot fix a syntax error from a
/// scrubbed message.
#[tokio::test]
async fn preview_surfaces_the_engines_own_message() {
    let Some(f) = fixture().await else { return };
    f.port.fail_next_with("line 1:8: mismatched input 'FRUM'");
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    assert!(
        text.contains("mismatched input 'FRUM'"),
        "the author must see the engine's own text, got {text}"
    );
    assert!(
        !text.contains("data_access_denied"),
        "serving's sanitising must NOT apply to a preview"
    );
}

/// Not in spec §5, and it belongs here anyway: a `database_id` from a request
/// body is exactly the shape that can name another workspace's connection.
///
/// # Why the database must really EXIST, in a real other workspace
///
/// This test used to send `Uuid::new_v4()`. A random uuid 404s from an
/// implementation with NO workspace scoping whatsoever — the row is absent from
/// `query.databases` entirely, so the answer is the same whether the read filters
/// by workspace or not, and dropping `AND workspace_id = $2` from
/// `EndpointStore::connection_per_principal_pair` left it green. A row that exists
/// somewhere else is the only version that distinguishes the two.
#[tokio::test]
async fn preview_refuses_a_foreign_database_id() {
    let Some(f) = fixture().await else { return };
    let foreign_db = f.database_in_another_workspace().await;
    let mut body = draft(&f, "SELECT 1 AS n");
    body["database_id"] = serde_json::json!(foreign_db);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a connection this workspace was never granted must be invisible, not merely forbidden — {out}"
    );
    assert_eq!(f.port.executions(), 0);
}

/// Spec §2.4.1: **`output.naming` is NOT applied to a preview's columns.**
///
/// An author debugging SQL needs the names they wrote; the published contract —
/// naming transform included — is what the `Response` section is for, and the UI's
/// copy says which is which. Nothing asserted the code half of that, so a mutation
/// that ran the columns through `apply_naming` before returning them failed
/// nothing.
///
/// # Why the column is `total_revenue` and not the fixture's `n`
///
/// `apply_naming` maps `n` to itself under every mode, so with the default fake
/// column this assertion would hold whether the transform ran or not. `camel`
/// turns `total_revenue` into `totalRevenue` — pinned as a unit test in
/// `serving/router.rs` — so this pair is the one that can tell the difference.
#[tokio::test]
async fn preview_returns_the_engines_column_names_not_the_endpoints_naming() {
    let Some(f) = fixture().await else { return };
    // The endpoint really does carry a renaming contract…
    sqlx::query("UPDATE dataapi.endpoints SET output = $1 WHERE id = $2")
        .bind(serde_json::json!({ "naming": "camel" }))
        .bind(f.endpoint_id)
        .execute(&f.db.pool)
        .await
        .expect("set output.naming");
    let (status, ep) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/endpoints/{}",
            f.ws_slug, f.endpoint_id
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{ep}");
    assert_eq!(
        ep["output"]["naming"].as_str(),
        Some("camel"),
        "the fixture must really declare a renaming contract, or this test is vacuous: {ep}"
    );

    // …and the engine really does return a name that contract would change.
    f.port.set_columns(&["total_revenue"]);
    let (status, body) = f
        .mgmt_post(
            &preview_path(&f),
            Some(draft(&f, "SELECT 1 AS total_revenue")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["columns"][0]["name"].as_str(),
        Some("total_revenue"),
        "preview must return the ENGINE's own column names, not the published contract's: {body}"
    );
}

/// A parameterised preview binds the author's values — the assertion that makes
/// every other one here non-vacuous. `bind_signature` folds the bound values
/// into the returned row, so two different test values cannot look the same.
#[tokio::test]
async fn preview_binds_the_authors_test_values() {
    let Some(f) = fixture().await else { return };
    let body = |v: &str| {
        serde_json::json!({
            "sql": "SELECT 1 AS n WHERE region = {{ region }}",
            "params": [{
                "name": "region",
                "type": "string",
                "location": "query",
                "required": true,
            }],
            "values": { "region": [v] },
            "database_id": f.database_id,
        })
    };
    let (s1, b1) = f.mgmt_post(&preview_path(&f), Some(body("EU"))).await;
    let (s2, b2) = f.mgmt_post(&preview_path(&f), Some(body("US"))).await;
    assert_eq!(s1, StatusCode::OK, "{b1}");
    assert_eq!(s2, StatusCode::OK, "{b2}");
    assert_ne!(
        b1["rows"][0][0], b2["rows"][0][0],
        "two different test values must produce different rows"
    );
    let (sql, binds) = f.port.last();
    assert!(
        !sql.contains("US"),
        "the value must not be interpolated into the statement, got {sql:?}"
    );
    assert_eq!(
        bind_signature(&binds),
        bind_signature(&[egret_query_api::Param::Str("US".into())]),
        "the author's value must reach the engine as a BIND, not as SQL text"
    );
}

/// Three different author mistakes, three different sentences — and none of them
/// may claim a type failure that did not happen.
///
/// A blank required field, a value the declared type cannot read, and two values
/// for an enum have three different fixes: fill the field in, correct the value,
/// remove the extra one. Serving answers all three with one payload-free sentence
/// because naming a parameter would hand a CONSUMER a directory of what an
/// endpoint declares — a reason that does not survive on a route whose caller
/// wrote the declarations and is looking at them. Task 3 renders this message
/// verbatim in the editor, so a wrong sentence here is a wrong instruction there.
#[tokio::test]
async fn preview_names_which_test_value_is_wrong() {
    let Some(f) = fixture().await else { return };
    let missing = serde_json::json!({
        "sql": "SELECT 1 AS n WHERE region = {{ region }}",
        "params": [{ "name": "region", "type": "string", "location": "query", "required": true }],
        "values": {},
        "database_id": f.database_id,
    });
    let mistyped = serde_json::json!({
        "sql": "SELECT 1 AS n WHERE day = {{ day }}",
        "params": [{ "name": "day", "type": "date", "location": "query", "required": true }],
        "values": { "day": ["not-a-date"] },
        "database_id": f.database_id,
    });
    let repeated_enum = serde_json::json!({
        "sql": "SELECT 1 AS n WHERE sort_dir = {{ sort_dir }}",
        "params": [{
            "name": "sort_dir",
            "type": "enum",
            "location": "query",
            "required": true,
            "allowed": ["asc", "desc"],
        }],
        "values": { "sort_dir": ["asc", "desc"] },
        "database_id": f.database_id,
    });

    let mut messages = Vec::new();
    for body in [missing, mistyped, repeated_enum] {
        let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
        messages.push(out["error"].as_str().unwrap_or_default().to_string());
    }
    let [blank, mistyped, repeated] = <[String; 3]>::try_from(messages).expect("three answers");

    assert!(
        blank.contains("region") && blank.contains("required"),
        "a blank required field must name itself and say what it is: {blank}"
    );
    assert!(
        !blank.contains("type"),
        "a blank required field is not a type failure: {blank}"
    );
    assert!(
        mistyped.contains("day") && mistyped.contains("date"),
        "a wrong-typed value must name the parameter and the type it must satisfy: {mistyped}"
    );
    assert!(
        repeated.contains("sort_dir") && repeated.contains("single"),
        "an enum given two values must say it takes one: {repeated}"
    );
    assert_ne!(blank, mistyped, "these are two different mistakes");
    assert_ne!(mistyped, repeated, "these are two different mistakes");
    assert_ne!(blank, repeated, "these are two different mistakes");
    assert_eq!(f.port.executions(), 0, "none of these reaches the engine");
}

/// `:read` cannot preview. egret's permissions are flat, so nothing but the
/// explicit `require` stands between a reader and running SQL on the engine.
///
/// The caller is a brand-new human holding EXACTLY `data_api_endpoint:read`,
/// measured against a PDP built after its grant exists — see
/// `Fixture::caller_with`, whose comment explains why a reused PDP would deny
/// everything and look exactly like a gate that works. It shares the fixture's
/// `FakePort`, so `executions()` still answers for it.
#[tokio::test]
async fn read_permission_cannot_preview() {
    let Some(f) = fixture().await else { return };
    let reader = f.caller_with(&["data_api_endpoint:read"]).await;
    let before = f.port.executions();
    let (status, body) = reader
        .post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`:read` must not reach the preview route — {body}"
    );
    assert_eq!(
        f.port.executions(),
        before,
        "a refused preview must not reach the engine"
    );
}

/// The other side of that measurement: the SAME request from a caller holding
/// exactly `:manage` succeeds.
///
/// Without this, `read_permission_cannot_preview` could be passing because the
/// request is malformed, the route is unmounted, or `caller_with` builds a
/// session that fails every gate — three ways to get a green 403 that says
/// nothing about the permission.
#[tokio::test]
async fn manage_permission_can_preview() {
    let Some(f) = fixture().await else { return };
    let author = f.caller_with(&["data_api_endpoint:manage"]).await;
    let (status, body) = author
        .post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0][0].as_i64(),
        Some(marker(UserId(author.user))),
        "the preview runs as whoever asked for it"
    );
}
