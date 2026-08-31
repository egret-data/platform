//! `POST …/endpoints/{id}/preview` — spec §5 of the P6 authoring-loop design.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like every other
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

/// The body every test below starts from: a draft with no parameters and no
/// transform.
///
/// `output` is spelled `{}` rather than omitted, and it is REQUIRED on the wire
/// (C2 Task 8) — the empty object is every `OutputSpec` field at its serde
/// default, which is an endpoint that declares no transform.
fn draft(f: &serving_common::Fixture, sql: &str) -> serde_json::Value {
    serde_json::json!({
        "sql": sql,
        "params": [],
        "values": {},
        "output": {},
        "database_id": f.database_id,
    })
}

/// An `output` carrying `script` as its transform, declaring `returns`.
///
/// A helper rather than a literal at each call site because the SHAPE is what
/// the tests below vary — a script that will not parse, a `returns` with
/// nothing in it — and spelling the rest out four times invites two of them to
/// drift.
fn transformed(script: &str, returns: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "transform": {
            "script": script,
            "returns": returns
                .iter()
                .map(|n| serde_json::json!({ "name": n, "ty": "bigint", "required": true }))
                .collect::<Vec<_>>(),
        }
    })
}

/// Spec §5: **preview passes the AUTHOR as the principal, not a service
/// account.**
///
/// `FakePort` folds the principal into the value it returns (`marker`), so this
/// pins which `UserId` the route hands the port. **It does not, and cannot, show
/// that the ENGINE runs the query as that user** — the real port only reaches
/// the author's identity when the connection is delegated or impersonating
/// (`egret-query/src/execution/mod.rs:43-88`); on a plain connection the engine
/// sees the service account whatever this argument says. The comment here used
/// to claim the stronger thing, and part of Package 1 was justified on top of
/// that claim. The rule that closes the gap on governed data is
/// [`preview_is_refused_on_a_governed_service_account_connection`] below.
#[tokio::test]
async fn preview_passes_the_author_as_the_principal() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0][0].as_i64(),
        Some(marker(UserId(f.author))),
        "preview must be given the session's own user as the principal, not a \
         service account"
    );
}

/// **Package 1: a governed database refuses the PREVIEW too, and this is where
/// the sentence that used to sit in `preview.rs` was made true.**
///
/// That comment justified leaving preview ungated with "a preview runs as the
/// author, under the author's own engine policy". It does not: `preview::run`
/// reaches `execute_as_with_params` -> `connect_as` -> `delegation_for`, and
/// with `delegated: false` and no `options.impersonate` there is no bearer,
/// Trino is sent the SERVICE user as `X-Trino-User` and StarRocks issues no
/// `EXECUTE AS`. So a `:manage` holder could press Run on a Ranger-masked
/// database and read unmasked rows while publish was refused and serving 503'd
/// — with the UI telling them the opposite. See `preview::run`'s doc for why
/// the answer chosen was to gate rather than to reword.
///
/// Every refusal below is bracketed by a case that must NOT refuse, one field
/// apart, because a fixture that could never preview would "prove" this rule
/// with the rule deleted.
#[tokio::test]
async fn preview_is_refused_on_a_governed_service_account_connection() {
    let Some(f) = fixture().await else { return };

    // Anti-triviality FIRST: this exact body previews while nothing is governed.
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the fixture's own draft must preview while the database is ungoverned: {body}"
    );

    // ONE column, on the database. Nothing about the draft changes.
    f.set_database_governed(f.database_id, true).await;

    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a preview on a governed database through a connection that runs every \
         query as the service account reads that data under the MACHINE's \
         policy — the exact thing publish and serving refuse: {body}"
    );
    let text = body.to_string();
    assert!(
        text.contains("governed"),
        "the refusal must name the flag that caused it: {body}"
    );
    assert!(
        text.contains("previewed"),
        "...and it must be a sentence about what the author just pressed, not \
         one about publishing only: {body}"
    );
    assert!(
        body.get("rows").is_none(),
        "a refused preview must not carry rows: {body}"
    );

    // One field apart, twice: a governed database whose connection DOES run per
    // principal previews normally -- first `delegated`...
    f.set_database_governed(f.delegated_database_id, true).await;
    let mut on_delegated = draft(&f, "SELECT 1 AS n");
    on_delegated["database_id"] = serde_json::json!(f.delegated_database_id);
    let (status, body) = f.mgmt_post(&preview_path(&f), Some(on_delegated)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "authoring on a governed database with a per-principal connection is \
         precisely what this package asks for and must not be blocked: {body}"
    );

    // ...then `options.impersonate` alone, which is the half a reader who knows
    // only `delegated` gets wrong.
    f.set_database_governed(f.impersonating_database_id, true)
        .await;
    let mut on_impersonating = draft(&f, "SELECT 1 AS n");
    on_impersonating["database_id"] = serde_json::json!(f.impersonating_database_id);
    let (status, body) = f.mgmt_post(&preview_path(&f), Some(on_impersonating)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`options.impersonate` makes the engine run as the caller independently \
         of `delegated`, so it satisfies the gate on its own: {body}"
    );

    // And clearing the flag restores the first case -- the rule reads the live
    // column rather than latching on the first refusal.
    f.set_database_governed(f.database_id, false).await;
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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

/// M2 debt: a preview must judge a parameter's SHAPE from the whole endpoint.
///
/// Whether a parameter arrives as a scalar or a list is decided by the endpoint —
/// one `{% for %}` header anywhere makes it a list for every statement, and both
/// publish and serving read it that way. Preview saw only the statement on
/// screen, so `summary` (a scalar use) previewed GREEN beside a `by_region` that
/// loops the same parameter, and then publish refused it. It failed closed, so
/// nothing unsafe ran; what the author got was "fine", then "no".
///
/// Both halves in one test, because the property is the DIFFERENCE: the
/// identical body without `siblings` still returns 200, which is both the
/// old-client compatibility promise and the proof that the refusal below comes
/// from the new field rather than from anything else in the draft.
#[tokio::test]
async fn preview_judges_a_parameter_shape_across_its_siblings() {
    let Some(f) = fixture().await else { return };
    let body = |siblings: serde_json::Value| {
        serde_json::json!({
            "sql": "SELECT 1 AS n WHERE region = {{ region }}",
            "siblings": siblings,
            "params": [{
                "name": "region",
                "type": "string",
                "location": "query",
                "required": true,
            }],
            "values": { "region": ["EU"] },
            "output": {},
            "database_id": f.database_id,
        })
    };

    // A sibling that LOOPS over `region` makes it a list for the endpoint, and
    // this statement cannot bind a list — the same `SequenceBind` publish gives.
    let looping = serde_json::json!([
        "SELECT region AS n FROM t WHERE region IN (         {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})"
    ]);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body(looping))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "preview must reach the conclusion publish reaches — {out}"
    );
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");

    // The control, and the compatibility promise: no `siblings` key at all —
    // the body an older client sends — is the union of one, and green.
    let (status, out) = f
        .mgmt_post(&preview_path(&f), Some(draft_with_region(&f)))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// A body that omits `siblings` entirely, for the control above.
///
/// Spelled as its own function rather than `body(json!([]))` on purpose: an
/// EMPTY list and an ABSENT field are two different wire shapes, and it is the
/// absent one that an old client sends and `#[serde(default)]` has to accept on
/// a `deny_unknown_fields` struct.
fn draft_with_region(f: &serving_common::Fixture) -> serde_json::Value {
    serde_json::json!({
        "sql": "SELECT 1 AS n WHERE region = {{ region }}",
        "params": [{
            "name": "region",
            "type": "string",
            "location": "query",
            "required": true,
        }],
        "values": { "region": ["EU"] },
        "output": {},
        "database_id": f.database_id,
    })
}

/// The siblings inform the shape and nothing else — they never reach the engine.
///
/// The risk the field creates is precisely this one: a `data_api_endpoint:manage`
/// route that accepts N SQL texts and runs the one on screen would become an
/// arbitrary multi-statement run if any of them were ever executed. Asserted
/// three ways, because one alone is weak — exactly ONE execution, and the SQL it
/// carried is the statement on screen, and the sibling's own marker is nowhere in
/// it.
///
/// A sibling that would be REFUSED as a definition (a `;`, a `DROP`) is used on
/// purpose: it is both the dangerous case and the ordinary one, since a sibling
/// is routinely mid-edit in another tab of the same form while the author
/// previews this statement.
#[tokio::test]
async fn preview_executes_only_the_statement_on_screen() {
    let Some(f) = fixture().await else { return };
    let mut body = draft(&f, "SELECT 1 AS n");
    body["siblings"] = serde_json::json!(["SELECT 2 AS sibling_marker", "DROP TABLE t; -- typing"]);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(f.port.executions(), 1, "one statement runs, not three");
    let (sql, _) = f.port.last();
    assert_eq!(sql, "SELECT 1 AS n");
    assert!(
        !sql.contains("sibling_marker") && !sql.contains("DROP"),
        "no sibling text may reach the engine, got {sql:?}"
    );
}

/// The bound on the new field, at the route.
///
/// `siblings` is an unbounded list of texts from a request body until something
/// says otherwise, and each one costs a template scan. The bound is the one
/// publish already has — `1 + siblings.len()` may not exceed `MAX_STATEMENTS`,
/// so a preview body cannot describe an endpoint that could not exist — and it
/// answers before the statement is scanned or anything is rendered.
#[tokio::test]
async fn preview_refuses_more_siblings_than_an_endpoint_may_have_statements() {
    let Some(f) = fixture().await else { return };
    let sqls: Vec<String> = (0..egret_dataapi_api::MAX_STATEMENTS)
        .map(|i| format!("SELECT {i} AS n"))
        .collect();

    let mut ok = draft(&f, "SELECT 1 AS n");
    ok["siblings"] = serde_json::json!(sqls[1..]);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(ok)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one statement plus its siblings is exactly an endpoint — {out}"
    );

    let mut too_many = draft(&f, "SELECT 1 AS n");
    too_many["siblings"] = serde_json::json!(sqls);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(too_many)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(
        out.to_string()
            .contains(&egret_dataapi_api::MAX_STATEMENTS.to_string()),
        "the refusal must name the limit, got {out}"
    );
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
            "output": {},
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

/// The browser finding, pinned at the route: a draft whose SQL references NONE
/// of its declared parameters previews anyway.
///
/// Measured against a real StarRocks engine, a stored draft declaring `region`
/// and `d` refused with "parameter `region` is declared but never referenced as
/// `{{region}}` in the query" the moment the author replaced the SQL — so the
/// single commonest intermediate state of the authoring loop could not be
/// previewed at all, in either edit order. That rule guards the OpenAPI contract
/// a PUBLISHED endpoint offers, so publish still refuses this pair; preview does
/// not (`lifecycle::validate_preview_accepts_sql_that_references_no_declared_parameter`).
#[tokio::test]
async fn preview_runs_sql_that_references_none_of_the_declared_parameters() {
    let Some(f) = fixture().await else { return };
    let body = serde_json::json!({
        // The parameters the stored draft declares, untouched…
        "params": [
            { "name": "region", "type": "string", "location": "query", "required": true },
            { "name": "d", "type": "date", "location": "query", "required": false },
        ],
        "values": { "region": ["EU"] },
        // …and SQL that reads neither of them, the way it looks mid-edit.
        "sql": "SELECT 7 AS mid_edit",
        "output": {},
        "database_id": f.database_id,
    });
    let (status, body) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an author must be able to preview the SQL on screen with the declarations mid-edit: {body}"
    );
    assert!(
        body["rows"].as_array().is_some_and(|r| !r.is_empty()),
        "the preview must come back with rows, not an empty result: {body}"
    );
    let (sql, binds) = f.port.last();
    assert!(
        sql.contains("mid_edit"),
        "the engine must have run the on-screen SQL, got {sql:?}"
    );
    assert!(
        binds.is_empty(),
        "no placeholder was referenced, so nothing may be bound: {binds:?}"
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
        "output": {},
        "database_id": f.database_id,
    });
    let mistyped = serde_json::json!({
        "sql": "SELECT 1 AS n WHERE day = {{ day }}",
        "params": [{ "name": "day", "type": "date", "location": "query", "required": true }],
        "values": { "day": ["not-a-date"] },
        "output": {},
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
        "output": {},
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

/// Spec §5: **preview judges a supplied value the way serving will.**
///
/// Preview reaches `serving::params::coerce` through `resolve_supplied_with` —
/// the same function serving resolves through — so every constraint Tasks 3-5
/// put inside `coerce` is inherited here for free. "For free" is a property to
/// TEST, not to assume: this seam has already failed twice in this programme,
/// most recently when preview judged a parameter's SHAPE from the one statement
/// on screen while publish judged it across the endpoint, so an author was told
/// "fine" and then told "no".
///
/// Both directions are measured, because only one of them is a gate: `5` against
/// `minimum: 10` must be refused **without reaching the engine**, and `10` must
/// go through. Without the second half this test passes just as well against a
/// preview route that refuses everything.
#[tokio::test]
async fn preview_refuses_a_value_its_endpoint_would_refuse() {
    let Some(f) = fixture().await else { return };
    let body = |v: &str| {
        serde_json::json!({
            "sql": "SELECT 1 AS n WHERE n >= {{ n }}",
            "params": [{
                "name": "n",
                "type": "number",
                "location": "query",
                "required": true,
                "minimum": "10",
            }],
            "values": { "n": [v] },
            "output": {},
            "database_id": f.database_id,
        })
    };

    let before = f.port.executions();
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body("5"))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a value the published endpoint would refuse must be refused in preview too — {out}"
    );
    assert_eq!(
        f.port.executions(),
        before,
        "a value that fails a constraint must not reach the engine"
    );

    // The refusal has to send the author to the right field. `5` IS a valid
    // number, so the pre-C1 sentence — "the test value for `n` is not a valid
    // number" — is a type failure that did not happen, and it points at the
    // `type` column instead of at the `minimum` the author just typed. This
    // route is `:manage`-gated and the author owns the declarations, which is
    // why the §2.1 payload-free rule that governs a consumer's 400 does not
    // govern this message; `preview::param_error`'s own doc comment is the
    // authority on that inversion.
    let msg = out["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains('n') && msg.contains("minimum") && msg.contains("10"),
        "the refusal must name the parameter, the constraint it broke and the bound: {msg}"
    );
    assert!(
        !msg.contains("not a valid"),
        "a value that is well-typed and out of range is not a type failure: {msg}"
    );

    // The other side of the gate, on the same declaration.
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body("10"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a value that satisfies the constraint must preview — {out}"
    );
    assert_eq!(
        f.port.executions(),
        before + 1,
        "the satisfying value must have reached the engine"
    );
}

/// The same inheritance for a constraint that is not a number, so the test above
/// cannot be passing because one `if` happens to sit on the numeric branch.
///
/// A `pattern` is the constraint most likely to be enforced in only one place:
/// it is the only one that has to COMPILE, and publish compiles it separately
/// (`lifecycle::compile_pattern`). Anchoring is what is actually pinned here —
/// `"EU-1x"` matches `^EU-\d$` nowhere, but an UNANCHORED `EU-\d` finds `EU-1`
/// inside it and passes.
#[tokio::test]
async fn preview_enforces_a_pattern_the_way_serving_does() {
    let Some(f) = fixture().await else { return };
    let body = |v: &str| {
        serde_json::json!({
            "sql": "SELECT 1 AS n WHERE region = {{ region }}",
            "params": [{
                "name": "region",
                "type": "string",
                "location": "query",
                "required": true,
                "pattern": "EU-\\d",
            }],
            "values": { "region": [v] },
            "output": {},
            "database_id": f.database_id,
        })
    };

    let before = f.port.executions();
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body("EU-1x"))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a value that only matches the pattern as a substring must be refused — {out}"
    );
    assert_eq!(
        f.port.executions(),
        before,
        "a value that fails a pattern must not reach the engine"
    );
    let msg = out["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("region") && msg.contains("pattern"),
        "the refusal must name the parameter and the constraint it broke: {msg}"
    );

    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body("EU-1"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a matching value must preview — {out}"
    );
    assert_eq!(
        f.port.executions(),
        before + 1,
        "the matching value must have reached the engine"
    );
}

// ---------------------------------------------------------------------------
// C2 §6 — the response transform, at the preview seam
// ---------------------------------------------------------------------------

/// **Preview refuses the transform publish refuses**, with publish's own words.
///
/// This is the seam, and it had been open in the one direction that costs an
/// author the most: `PreviewEndpointInput` carried no `output` at all, so a
/// draft whose script does not parse — or that declares nothing for serving to
/// check against — previewed GREEN and was then refused at publish. Told
/// "fine", then told "no".
///
/// Two refusals, chosen for two different reasons:
///
/// * the **unparseable script** is the one that goes through
///   `transform::eval_probe`, i.e. the single evaluator. A preview that reached
///   its own parser could accept a script publish rejects, which is the
///   second-implementation failure this whole package is built to avoid;
/// * the **empty `returns`** is the one whose sentence is short enough to pin
///   whole, so this asserts publish's message character for character rather
///   than merely asserting a 400. A preview-shaped imitation of the rule would
///   pass a status assertion and fail this one.
#[tokio::test]
async fn preview_refuses_the_transform_publish_refuses() {
    let Some(f) = fixture().await else { return };

    let mut unparseable = draft(&f, "SELECT 1 AS n");
    unparseable["output"] = transformed("def transform(params, statements:\n", &["total"]);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(unparseable)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("the transform script is not valid"),
        "a script that does not parse must be refused in publish's words: {out}"
    );
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");

    let mut no_returns = draft(&f, "SELECT 1 AS n");
    no_returns["output"] = transformed("def transform(params, statements):\n  return {}", &[]);
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(no_returns)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    // `ValidationError` reaches a body through `Error::InvalidInput`, which
    // prefixes it — the same wrapping publish's own refusal gets, so this is
    // still publish's sentence and not a paraphrase of it.
    assert_eq!(
        out["error"].as_str(),
        Some(
            format!(
                "invalid input: {}",
                egret_dataapi::lifecycle::ValidationError::TransformReturnsEmpty
            )
            .as_str()
        ),
        "preview must answer with the sentence publish answers with, not one of its own"
    );
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");

    // The control that makes both refusals mean something: the SAME script,
    // parseable, with a `returns` — green. Without this, a route that refused
    // every transform outright would pass everything above.
    let mut good = draft(&f, "SELECT 1 AS n");
    good["output"] = transformed(
        "def transform(params, statements):\n  return {\"total\": 1}",
        &["total"],
    );
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(good)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a valid transform must preview — {out}"
    );
}

/// **Preview may not read a default `OutputSpec` for a body that omits one.**
///
/// The guard against the edit this seam invites. `output` looks exactly like
/// the kind of field that should carry `#[serde(default)]` — `siblings`,
/// `params` and `values` all do, and one of them documents at length why an
/// older client must keep working. It must not, and the direction is what
/// decides it: a defaulted `OutputSpec` has `transform: None`, so an omitted
/// `output` makes every refusal in the test above vanish and preview goes green
/// on a draft publish refuses. That is the failure, not a compatibility cost.
///
/// Asserted at the WIRE rather than through a Rust call, because the edit to
/// catch is a serde attribute: `#[serde(default)]` on the field, or an
/// `OutputSpec::default()` threaded in at a call site, both leave every other
/// test in this file green and turn this one red.
#[tokio::test]
async fn preview_will_not_read_a_default_output_for_a_body_that_omits_one() {
    let Some(f) = fixture().await else { return };
    let mut body = draft(&f, "SELECT 1 AS n");
    body.as_object_mut().expect("an object").remove("output");
    assert!(
        body.get("output").is_none(),
        "the field must really be absent"
    );

    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    // The EXACT status, not merely "not 200". `assert_ne!` here passed on any
    // refusal at all, including a 500 from something unrelated — so the test
    // could have gone on reporting success while measuring a different failure.
    // 422 is what axum's `Json` extractor answers for a missing required field,
    // and it is the only answer that says the field is required rather than
    // that the draft was bad.
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a body with no `output` must be refused by the extractor rather than silently \
         read as `OutputSpec::default()` — a default carries no transform, so preview \
         would go green on every draft publish refuses: {out}"
    );
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");
}

/// **A transform is DECLARED to preview, and preview says it did not apply it.**
///
/// The decision this task settles, asserted rather than described. Preview runs
/// exactly ONE statement of an endpoint that may have sixteen, and
/// `run_transform` takes every statement's rows keyed by NAME — which this
/// route is not even told. Handing the script the one statement's rows with the
/// others empty would show the author a refusal for an endpoint that serves
/// fine, or worse a GREEN answer computed from `[]`. So the rows below are the
/// engine's own, and `transform_declared` is what tells the UI to say so.
///
/// Both sides in one test, because the property is the DIFFERENCE: the same
/// draft without a transform must read `false`. A constant — the flag hardcoded
/// either way, or dropped from the DTO — cannot satisfy both halves.
#[tokio::test]
async fn preview_shows_the_statements_own_rows_and_says_a_transform_is_declared() {
    let Some(f) = fixture().await else { return };

    let (status, plain) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    assert_eq!(
        plain["transform_declared"].as_bool(),
        Some(false),
        "a draft with no transform must not claim one: {plain}"
    );

    // A script that returns something no raw row could be mistaken for. If
    // preview ever ran it, `columns` would be `total` and `rows` would carry
    // 99 — so the two assertions below are what say the script did not run.
    let mut body = draft(&f, "SELECT 1 AS n");
    body["output"] = transformed(
        "def transform(params, statements):\n  return {\"total\": 99}",
        &["total"],
    );
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        out["transform_declared"].as_bool(),
        Some(true),
        "the answer must tell the author the endpoint declares a transform: {out}"
    );
    assert_eq!(
        out["columns"][0]["name"].as_str(),
        Some("n"),
        "preview answers with the statement's own columns, not the transform's `returns`: {out}"
    );
    assert_eq!(
        out["rows"][0][0].as_i64(),
        Some(marker(UserId(f.author))),
        "preview answers with the statement's own rows, not the script's output: {out}"
    );
}

// ---------------------------------------------------------------------------
// Package S Task 7 — a statement whose SQL a SCRIPT builds.
// ---------------------------------------------------------------------------

/// A script that emits a DIFFERENT template for `region = "US"`.
///
/// Branching is what makes the assertions below non-vacuous. Publish generates
/// with `lifecycle::representative_value` (`"x"` for a string) and probes with a
/// sentinel, so **no publish-time evaluation ever takes the `US` arm** — a
/// preview that showed the author the baseline template would show it for every
/// test value they typed, and the two-value comparison below is what catches
/// that.
const BRANCHING: &str = "def build_sql(params):\n    \
     if params[\"region\"] == \"US\":\n        \
     return \"SELECT n FROM t WHERE country = {{ region }}\"\n    \
     return \"SELECT n FROM t WHERE region = {{ region }}\"\n";

/// A preview body whose statement is `script` rather than `sql`, with one
/// required string parameter `region` at `value`.
fn scripted(f: &serving_common::Fixture, script: &str, value: &str) -> serde_json::Value {
    serde_json::json!({
        "script": script,
        "params": [{
            "name": "region",
            "type": "string",
            "location": "query",
            "required": true,
        }],
        "values": { "region": [value] },
        "output": {},
        "database_id": f.database_id,
    })
}

/// **Package S Task 7: preview shows the SQL the script produced, and runs it.**
///
/// Three claims in one test because they are one property — the artefact on
/// screen has to be the artefact that ran:
///
/// * the response carries the template `build_sql` returned, `{{ }}` intact,
///   which is the thing an author debugging a branching script cannot otherwise
///   see;
/// * it is generated from the author's OWN test values, so the two calls below
///   disagree. Generating from publish's representative values would make both
///   answers the baseline branch and is the defect this asserts against;
/// * the engine ran the rendered form of exactly that text, with the value
///   BOUND rather than interpolated — the same guarantee written SQL has.
#[tokio::test]
async fn preview_shows_the_generated_sql_and_runs_it() {
    let Some(f) = fixture().await else { return };

    let (status, us) = f
        .mgmt_post(&preview_path(&f), Some(scripted(&f, BRANCHING, "US")))
        .await;
    assert_eq!(status, StatusCode::OK, "{us}");
    assert_eq!(
        us["generated_sql"].as_str(),
        Some("SELECT n FROM t WHERE country = {{ region }}"),
        "the answer must carry the template the script built, braces intact: {us}"
    );
    // What the engine was actually asked to run is the RENDERED form of that
    // template — same statement, placeholder replaced by a bind marker.
    let (sql, binds) = f.port.last();
    assert!(
        sql.contains("country"),
        "the engine must have been sent the branch the author's value takes, got {sql:?}"
    );
    assert!(
        !sql.contains("US"),
        "the value must not be interpolated into the generated statement, got {sql:?}"
    );
    assert_eq!(
        bind_signature(&binds),
        bind_signature(&[egret_query_api::Param::Str("US".into())]),
        "a generated statement must bind the author's value exactly as a written one does"
    );
    assert_eq!(
        us["rows"][0][0].as_i64(),
        Some(marker(UserId(f.author)) + bind_signature(&binds)),
        "the rows must be the ones that SQL returned, for the author's own principal: {us}"
    );

    // The other branch, from the same script and a different test value.
    let (status, eu) = f
        .mgmt_post(&preview_path(&f), Some(scripted(&f, BRANCHING, "EU")))
        .await;
    assert_eq!(status, StatusCode::OK, "{eu}");
    assert_eq!(
        eu["generated_sql"].as_str(),
        Some("SELECT n FROM t WHERE region = {{ region }}"),
        "a different test value must show the branch it takes, not the one the last \
         call took and not publish's representative branch: {eu}"
    );
}

/// A statement the author TYPED generates nothing, and the answer says so.
///
/// `null` rather than an echo of the body: the author is looking at that text in
/// the editor, and a field that always carries something gives the UI no way to
/// tell "this is what your script built" from "this is what you typed".
#[tokio::test]
async fn a_written_statement_generates_nothing_to_show() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The key must be THERE and null. `body["generated_sql"].is_null()` alone is
    // also true of a key that does not exist, so it would pass with the field
    // deleted from the DTO — and it did, before this line, while the field was
    // still unimplemented.
    assert_eq!(
        body.get("generated_sql"),
        Some(&serde_json::Value::Null),
        "a written statement has no generated SQL to show, and the answer must say so \
         rather than omit the field: {body}"
    );
}

/// **The branch publish's probes never take is gated HERE too.**
///
/// `serving::router::generate_one` re-applies two publish rules to text
/// generated per request, because a script may branch on a value no probe
/// supplies. Preview generates from the author's test values, which is the same
/// situation and the same risk in both directions: the statement below is not a
/// read at all, and preview reaches a real engine — on an ordinary connection
/// under the SERVICE account's credentials, which is a stronger reason to refuse
/// a generated `DELETE` than "the author is running it", not a weaker one.
///
/// Both halves are asserted. The refusal, and that **nothing reached the
/// engine** — "a 400" and "a 400 after running DML" are the same status code and
/// very different events.
#[tokio::test]
async fn preview_refuses_a_generated_branch_that_is_not_a_read() {
    let Some(f) = fixture().await else { return };
    let script = "def build_sql(params):\n    \
         if params[\"region\"] == \"XX\":\n        \
         return \"DELETE FROM t\"\n    \
         return \"SELECT n FROM t WHERE region = {{ region }}\"\n";

    // The control: the baseline branch previews green, so the refusal below is
    // about the branch and not about the script.
    let (status, ok) = f
        .mgmt_post(&preview_path(&f), Some(scripted(&f, script, "EU")))
        .await;
    assert_eq!(status, StatusCode::OK, "{ok}");

    let before = f.port.executions();
    let (status, out) = f
        .mgmt_post(&preview_path(&f), Some(scripted(&f, script, "XX")))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a generated branch that is not a single read must be refused: {out}"
    );
    assert_eq!(
        f.port.executions(),
        before,
        "nothing may reach the engine once generation is refused"
    );
}

/// **Every template the script can emit is scanned, not only the one the
/// author's values reach.**
///
/// `lifecycle::GeneratedSql::probe` runs the script again with a sentinel and
/// then with the parameter ABSENT, and publish scans everything those runs
/// emitted — a branch that is not a read is refused there even when the
/// representative values do not take it. Preview has to reach the same verdict
/// or the author is told "fine" and then told "no" at publish, which is the one
/// thing this route exists not to do.
///
/// The test value below takes the SELECT branch, so `generate_one`'s per-value
/// gate is satisfied and cannot be what refuses this. Only the scan over the
/// absent-parameter template can.
#[tokio::test]
async fn preview_scans_every_template_the_script_can_emit() {
    let Some(f) = fixture().await else { return };
    let script = "def build_sql(params):\n    \
         if params[\"region\"] == None:\n        \
         return \"DELETE FROM t\"\n    \
         return \"SELECT n FROM t WHERE region = {{ region }}\"\n";

    let before = f.port.executions();
    let (status, out) = f
        .mgmt_post(&preview_path(&f), Some(scripted(&f, script, "EU")))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a branch that is not a read must be refused even when the author's own test \
         value does not take it — publish refuses it, so preview must: {out}"
    );
    assert_eq!(
        f.port.executions(),
        before,
        "nothing may reach the engine once a template is refused"
    );
}

/// **A misspelt field is a refusal at the wire, not an ignored key.**
///
/// The unit tests beside the `Deserialize` impl pin the rule; this pins what it
/// is FOR, end to end. `sibling` for `siblings` reads as an endpoint with no
/// other statements, so the shape decision becomes the union of one — and a
/// draft whose real sibling loops the same parameter previews GREEN and is then
/// refused at publish for `SequenceBind`. Told "fine", then told "no".
///
/// The body below is exactly that draft: `region` is a scalar in the statement
/// on screen and iterated by the sibling the author meant to send.
#[tokio::test]
async fn preview_refuses_a_misspelt_field_rather_than_ignoring_it() {
    let Some(f) = fixture().await else { return };
    let looping = "SELECT n FROM t WHERE region IN (\
         {% for x in region %}{{ x }}{% if not loop.last %},{% endif %}{% endfor %})";
    let mut body = serde_json::json!({
        "sql": "SELECT n FROM t WHERE region = {{ region }}",
        "params": [{ "name": "region", "type": "string", "location": "query", "required": true }],
        "values": { "region": ["EU"] },
        "output": {},
        "database_id": f.database_id,
    });

    // Spelled correctly, the sibling is heard and the draft is refused for the
    // shape — which is publish's answer, and the control that says the typo
    // below really does change the verdict.
    body["siblings"] = serde_json::json!([looping]);
    let (heard, out) = f.mgmt_post(&preview_path(&f), Some(body.clone())).await;
    assert_eq!(
        heard,
        StatusCode::BAD_REQUEST,
        "a sibling that iterates the parameter must reach the shape decision: {out}"
    );

    let before = f.port.executions();
    let obj = body.as_object_mut().expect("an object");
    obj.remove("siblings");
    obj.insert("sibling".into(), serde_json::json!([looping]));
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a misspelt field must be refused by the extractor; ignoring it previews GREEN a \
         draft publish refuses: {out}"
    );
    assert_eq!(
        f.port.executions(),
        before,
        "nothing may reach the engine for a body that was not understood"
    );
}
