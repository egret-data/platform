//! M2 Task 11 — the feature, driven end to end, and the upgrade path it must not break.
//!
//! Tasks 1-10 each tested their own half: the snapshot reader has unit tests, the
//! renderer has unit tests, `meta_contract` has unit tests, and the serving suites
//! each pin one clause of one response. **None of them publishes the canonical
//! two-statement endpoint through the real publish route, calls it through the real
//! serving route, and reads the whole body back.** That is what this file is, and it
//! is deliberately the only file in the crate whose tests assert a WHOLE envelope
//! rather than a field of one — a suite where every assertion is a single key can be
//! entirely green while the document as a shipped object is wrong, which is exactly
//! what happened to `openapi_gen.rs` earlier in this package.
//!
//! Two tests, because M2 makes two promises to two different audiences:
//!
//! * **The new promise** — an author writes `rows` and `total`, and a consumer gets
//!   both result sets keyed by those names with a per-statement contract beside each.
//! * **The old promise** — an endpoint published BEFORE this branch keeps serving,
//!   byte for byte, with nobody re-publishing anything. A published version is
//!   immutable and a deploy is not allowed to require an author's attention, so this
//!   is the promise with consequences outside this branch: breaking it breaks live
//!   consumers of endpoints nobody has touched in months.
//!
//! Driven through `serving_common`, which owns the REAL management router and the
//! REAL serving router, so nothing here is a store call in disguise.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset — check the DURATION,
//! not just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use serving_common::{fixture, marker};

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

/// The keys of a JSON object, in wire order.
///
/// Order rather than membership because `serde_json`'s `preserve_order` is on
/// across this workspace, so the wire order IS stable and is part of what a
/// consumer reads.
fn keys(v: &serde_json::Value) -> Vec<&str> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {v}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Column NAMES from a `meta` column list, in order.
fn column_names(v: &serde_json::Value) -> Vec<&str> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected a column array, got {v}"))
        .iter()
        .map(|c| {
            c["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a column with no name: {c}"))
        })
        .collect()
}

/// The canonical pair — the two statements the whole feature was specified around.
const ROWS_SQL: &str = "SELECT id, name FROM t";
const TOTAL_SQL: &str = "SELECT count(*) AS total FROM t";

// ---------------------------------------------------------------------------
// 1. The canonical pair, whole
// ---------------------------------------------------------------------------

/// **Publish `rows` + `total` through the real publish route, call it through the
/// real serving route, and assert the entire body.**
///
/// The assertions below are ordered as a consumer meets them: the envelope, then
/// `data`, then `meta`. Each is asserted as a WHOLE — the four top-level keys in
/// order, the full `data` object compared against a literal, the full key set of
/// `meta` — because a field-at-a-time suite cannot see a body that grew a key, lost
/// one, or reordered them, and all three are breaking changes to a client that
/// generated its types from the document.
///
/// The row VALUES are asserted against `marker(principal)` rather than against
/// "some number": the fake engine folds the calling principal into every cell, so a
/// literal match is evidence that both statements actually reached the engine as
/// this caller. A serving path that returned the frozen columns with zeroed cells,
/// or that ran the first statement twice, produces a different literal.
#[tokio::test]
async fn the_canonical_pair_serves_both_result_sets_keyed_by_name() {
    let f = skip_without_db!();
    // Installed BEFORE publish, so these are the columns publish DESCRIBES and
    // freezes. First arm wins, so the catch-all goes last.
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);

    f.publish_statements(
        "/canonical",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": ROWS_SQL },
            { "name": "total", "sql": TOTAL_SQL },
        ]),
        serde_json::json!({}),
    )
    .await;

    let res = f.call_as(f.principal_a, "canonical").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);

    // --- the envelope ---
    assert_eq!(
        keys(&res.body),
        vec!["status", "message", "data", "meta"],
        "the envelope is these four keys in this order and no others: {}",
        res.body
    );
    assert_eq!(res.body["status"], "ok", "{}", res.body);
    assert!(
        res.body["message"].is_null(),
        "`message` is the human-readable half of an ERROR, null on success: {}",
        res.body
    );

    // --- data, whole ---
    let m = marker(f.principal_a);
    assert_eq!(
        res.body["data"],
        serde_json::json!({
            "rows":  [{ "id": m, "name": m }],
            "total": [{ "total": m }],
        }),
        "each statement answers under its own name, with its own columns, and \
         with cells this caller's execution produced: {}",
        res.body
    );

    // --- meta, whole ---
    assert_eq!(
        keys(&res.body["meta"]),
        vec!["version", "cached", "request_id", "statements"],
        "`meta` carries the call-wide fields and the keyed contract, and — the \
         half that matters — NOT `row_count` or `columns` beside them: a sum \
         across independent reads is the arity of nothing, and a flat union of \
         columns is a contract no statement has: {}",
        res.body
    );
    assert_eq!(res.body["meta"]["version"], 1, "{}", res.body);
    assert_eq!(res.body["meta"]["cached"], false, "{}", res.body);
    assert!(res.body["meta"]["request_id"].is_string(), "{}", res.body);

    assert_eq!(
        keys(&res.body["meta"]["statements"]),
        vec!["rows", "total"],
        "the contract is keyed by statement name, in the author's order: {}",
        res.body
    );
    assert_eq!(
        res.body["meta"]["statements"]["rows"]["row_count"], 1,
        "{}",
        res.body
    );
    assert_eq!(
        column_names(&res.body["meta"]["statements"]["rows"]["columns"]),
        vec!["id", "name"],
        "the `rows` statement's own frozen columns: {}",
        res.body
    );
    assert_eq!(
        res.body["meta"]["statements"]["total"]["row_count"], 1,
        "{}",
        res.body
    );
    assert_eq!(
        column_names(&res.body["meta"]["statements"]["total"]["columns"]),
        vec!["total"],
        "the `total` statement's own frozen column — not the `rows` \
         statement's, and not the concatenation of both: {}",
        res.body
    );

    // Two executions, not one and not three: the endpoint ran each statement
    // once. A `data` that looked right could still have been built by running
    // one statement and reshaping it.
    assert_eq!(
        f.port.executions(),
        2,
        "one execution per statement: {}",
        res.body
    );

    // The keys a consumer writes into their own code come from the AUTHOR's
    // statement names, which is why publish validates them as identifiers. Read
    // back off the wire so the claim is measured rather than restated.
    let names: Vec<&str> = keys(&res.body["data"]);
    assert_eq!(
        names,
        keys(&res.body["meta"]["statements"]),
        "`data` and `meta.statements` are keyed identically — a client that \
         iterates one and indexes the other must never miss: {}",
        res.body
    );
}

// ---------------------------------------------------------------------------
// 2. The upgrade path
// ---------------------------------------------------------------------------

/// **An endpoint published before this branch serves `200` without being
/// re-published, and answers with the body it always answered with.**
///
/// This is the promise with consequences outside the branch. `endpoint_versions.
/// snapshot` is immutable by design — it is the contract a live consumer is already
/// coding against — so M2 migrates nothing there and the compatibility lives in the
/// READER (`EndpointSnapshotRepr`). The unit tests in `egret-dataapi-api` pin that
/// the reader folds `sql` + `columns` into one statement; **this pins that the route
/// reaches the reader at all**, which no unit test can, and that everything
/// downstream of it — rendering, execution, the drift check, shaping, `meta` — still
/// agrees on the one-statement shape.
///
/// # Why the snapshot is forged rather than published
///
/// "Published weeks ago on `main`" is the situation under test, and the only honest
/// way to have such a row is to write one. It is forged from a REAL published
/// snapshot rather than assembled from a literal, so every field this test does not
/// care about is exactly what publish freezes and the diff between the two rows is
/// only the shape change:
///
/// * `statements` is DELETED and the pre-M2 `sql` + `columns` pair is lifted out of
///   its first element — the shape every row published before this branch carries;
/// * `output.envelope` is DELETED too, because it is also new in M2, and a forge
///   that left it in place would be testing a snapshot no deployment has.
///
/// `rows_affected` is asserted because a `jsonb` path that does not exist updates the
/// row to an unchanged value and still reports success — this test would then run
/// against an untouched M2 snapshot and pass while measuring nothing. The forged
/// snapshot is then read BACK and its shape asserted, which is the stronger check of
/// the two: `rows_affected` proves a row was written, not that it was written into
/// the shape the test is named after.
///
/// # Why a second endpoint
///
/// The promise is not "still 200"; it is "unchanged". A control endpoint published
/// natively, on the same SQL, gives the test a body to compare against, so the
/// assertion is byte equality (less `request_id`, which is per-request by design)
/// rather than a hand-written expectation that could drift from the real one.
#[tokio::test]
async fn an_endpoint_published_before_this_branch_still_serves_unchanged() {
    let f = skip_without_db!();

    // Two endpoints, identical in every way except which shape their frozen
    // snapshot is in.
    let old = f
        .publish_statements(
            "/legacy",
            f.database_id,
            serde_json::json!([{ "name": "main", "sql": "SELECT 1 AS n" }]),
            serde_json::json!({}),
        )
        .await;
    f.publish_statements(
        "/modern",
        f.database_id,
        serde_json::json!([{ "name": "main", "sql": "SELECT 1 AS n" }]),
        serde_json::json!({}),
    )
    .await;

    // --- forge the pre-M2 row ---
    let touched = sqlx::query(
        "UPDATE dataapi.endpoint_versions \
            SET snapshot = ((snapshot #- '{output,envelope}') - 'statements') \
                || jsonb_build_object( \
                       'sql',     snapshot #> '{statements,0,sql}', \
                       'columns', snapshot #> '{statements,0,columns}') \
          WHERE endpoint_id = $1",
    )
    .bind(old)
    .execute(&f.db.pool)
    .await
    .expect("forge a pre-M2 snapshot")
    .rows_affected();
    assert_eq!(
        touched, 1,
        "the frozen snapshot must actually have been rewritten — a jsonb path \
         that does not exist still reports success"
    );

    // And it is in the shape this test is named after. Asserted from the stored
    // row rather than from the statement above, because the `UPDATE` succeeding
    // says nothing about what it wrote.
    let forged: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM dataapi.endpoint_versions WHERE endpoint_id = $1")
            .bind(old)
            .fetch_one(&f.db.pool)
            .await
            .expect("read the forged snapshot back");
    assert!(
        forged.get("statements").is_none(),
        "a pre-M2 snapshot has no `statements` key at all: {forged}"
    );
    assert_eq!(
        forged["sql"], "SELECT 1 AS n",
        "the pre-M2 pair's first half: {forged}"
    );
    assert_eq!(
        column_names(&forged["columns"]),
        vec!["n"],
        "the pre-M2 pair's second half — the columns frozen beside the sql: {forged}"
    );
    assert!(
        forged["output"].get("envelope").is_none(),
        "`output.envelope` is new in M2 too; a row published before this branch \
         does not carry it, and leaving it in would test a snapshot no \
         deployment has: {forged}"
    );

    // --- and it serves ---
    let legacy = f.call_as(f.principal_a, "legacy").await;
    assert_eq!(
        legacy.status,
        StatusCode::OK,
        "an endpoint published before this branch must serve without a \
         re-publish — published versions are immutable and a deploy cannot \
         require an author to act: {} {}",
        legacy.status,
        legacy.body
    );

    // The wire shape, stated explicitly and not only by comparison: the
    // synthesized statement name `main` must NOT appear anywhere. A consumer of
    // this endpoint reads `data` as an array, and keying it would break every
    // one of them under a 200.
    assert_eq!(
        legacy.body["data"],
        serde_json::json!([{ "n": marker(f.principal_a) }]),
        "one statement's `data` is the rows themselves, exactly as before: {}",
        legacy.body
    );
    assert_eq!(
        keys(&legacy.body["meta"]),
        vec!["version", "cached", "request_id", "row_count", "columns"],
        "a one-statement endpoint reports its contract DIRECTLY — the pre-M2 \
         `meta`, with no `statements` map and no synthesized name on the wire: {}",
        legacy.body
    );
    assert_eq!(legacy.body["meta"]["row_count"], 1, "{}", legacy.body);
    assert_eq!(
        column_names(&legacy.body["meta"]["columns"]),
        vec!["n"],
        "{}",
        legacy.body
    );

    // --- unchanged, against a natively published control ---
    let modern = f.call_as(f.principal_a, "modern").await;
    assert_eq!(modern.status, StatusCode::OK, "{}", modern.body);

    let strip = |mut b: serde_json::Value| {
        // Per-request by design: it is what correlates a caller's complaint with
        // a row in `dataapi.access_log`, so it is the one field that MUST differ.
        // Asserted present first, so stripping it cannot hide its absence.
        assert!(b["meta"]["request_id"].is_string(), "{b}");
        b["meta"]
            .as_object_mut()
            .expect("meta is an object")
            .remove("request_id");
        b
    };
    assert_eq!(
        strip(legacy.body.clone()),
        strip(modern.body.clone()),
        "the upgrade changes NOTHING a consumer can observe: a snapshot frozen \
         before this branch answers byte for byte what one frozen after it \
         does.\nlegacy: {}\nmodern: {}",
        legacy.body,
        modern.body
    );
}
