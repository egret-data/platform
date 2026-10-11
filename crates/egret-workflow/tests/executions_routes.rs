//! The execution READ surface, driven over real HTTP against a real
//! `CedarPdp` (W10 Task 4) — the first time an execution is visible outside
//! the database.
//!
//! ```text
//! GET /api/ws/{workspace}/workflows/executions
//! GET /api/ws/{workspace}/workflows/executions/{id}
//! ```
//!
//! **The first three tests in this file were written before the handlers they
//! describe, and each fails for a different reason when the wrong thing
//! ships:** 401 without a session, 403 without `workflow:read`, and — the one
//! this package keeps guarding against — **404 and never 403** for an
//! execution in another workspace. A 403 on a foreign id confirms the id is
//! real, which is the same disclosure as serving the row; that is the
//! `chart_id` class migration 0128 shipped and spec §8 records.
//!
//! **Two shapes of test, and the difference is not cosmetic.** Everything
//! about *what the HTTP surface answers* runs `#[tokio::test]` against the
//! shared `DATABASE_URL` database, as `tests/routes.rs` does: those tests
//! issue no global statement, so a neighbouring binary cannot disturb them.
//! [`a_gantt_row_is_computable_from_the_execution_dto_alone`] is
//! `#[sqlx::test]` instead, because it drives the **real worker** to produce a
//! real execution, and `worker::claim` is global by construction — a shared
//! database would let it claim a neighbour's runs. That test uses the worker
//! rather than seeding rows itself precisely so the gantt input it reads is
//! production's shape and not a fixture's.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-workflow --test executions_routes

use chrono::{DateTime, TimeZone, Utc};
use egret_metastore::Db;
use serde_json::Value;
use uuid::Uuid;

mod common;

// ── Fixtures ────────────────────────────────────────────────────────────────

/// Insert one `workflow.executions` row directly.
///
/// Raw SQL rather than `start_execution`, for `tests/execution_repo.rs`'s
/// stated reason: these tests are about what the *reads* answer, so they have
/// to be able to spell states, flows and timestamps a writer would never
/// produce in one sitting. The one test that needs a production-shaped
/// execution drives the production writer instead.
async fn insert_execution(
    db: &Db,
    ws: Uuid,
    flow_id: Option<Uuid>,
    namespace: &str,
    flow_key: &str,
    state: &str,
    created_at: DateTime<Utc>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(ws)
    .bind(flow_id)
    .bind(namespace)
    .bind(flow_key)
    .bind(SEEDED_SOURCE)
    .bind(state)
    .bind(serde_json::json!(["CREATED", "RUNNING"]))
    .bind(created_at)
    .fetch_one(&db.pool)
    .await
    .expect("insert execution")
}

/// The frozen YAML every seeded execution carries. A distinctive string so
/// [`the_list_never_carries_source_and_the_detail_always_does`] can assert on
/// its presence and its absence rather than on a field name alone.
const SEEDED_SOURCE: &str = "id: seeded\nnamespace: a.b\ntasks: []\n";

fn at(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap()
}

/// `GET …/executions?{q}`, decoded. A free function rather than a closure per
/// test because the response future borrows the app, and a closure returning
/// it cannot express that.
async fn list(app: &common::App, ws: Uuid, user: egret_core::UserId, q: &str) -> Value {
    common::json(common::get(app, &format!("/api/ws/{ws}/workflows/executions?{q}"), user).await)
        .await
}

// ── The three authorization tests. Written first. ───────────────────────────

/// **401, not 403 and not 404**: with no session at all there is no principal
/// to decide anything about, and `Auth` refuses before `WsContext` has even
/// resolved the slug.
///
/// Both routes, because they are two handlers: a `require` left off the detail
/// route would be invisible to a test that only drove the list.
#[tokio::test]
async fn an_unauthenticated_caller_gets_401_from_both_routes() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let id = insert_execution(&db, ws, None, "a.anon", "f", "SUCCESS", at(1)).await;
    // Granted BEFORE the app is built: `CedarPdp::build` loads the policy set
    // once, so a grant written after it is invisible to that PDP -- a real
    // property of the decision point, and the reason `tests/routes.rs` orders
    // every test the same way.
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let app = common::app(&db).await;
    assert_eq!(
        common::anon_get(&app, &format!("/api/ws/{ws}/workflows/executions"))
            .await
            .status(),
        401,
        "the list route must refuse an anonymous caller"
    );
    assert_eq!(
        common::anon_get(&app, &format!("/api/ws/{ws}/workflows/executions/{id}"))
            .await
            .status(),
        401,
        "the detail route must refuse an anonymous caller"
    );

    // The positive control: the SAME two URLs, with a session, answer 200.
    // Without it a router that had no such routes at all would also "pass" the
    // two assertions above — a 404 is not a 401, but a typo'd path would be
    // neither, and this is what tells them apart.
    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user)
            .await
            .status(),
        200
    );
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user
        )
        .await
        .status(),
        200
    );
}

/// **403 without `workflow:read`.** The caller is a real, authenticated member
/// of the workspace holding `workflow:manage` and nothing else — egret's
/// permissions never imply one another, so the authoring permission does not
/// open the run list.
///
/// Granting `:manage` rather than nothing at all is deliberate: a handler that
/// had no `require` call would still 403 a member with *no* grants (Cedar
/// refuses by default), so a bare unprivileged user cannot tell "this route
/// checks `workflow:read`" from "this route checks anything at all". A
/// `:manage` holder can.
#[tokio::test]
async fn workflow_manage_alone_cannot_read_executions() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    // The control's grant is written before the PDP is built too -- see
    // [`an_unauthenticated_caller_gets_401_from_both_routes`].
    let (_ws2, reader) = common::seed_workspace(&db).await;
    common::grant(&db, ws, reader, &["workflow:read"]).await;
    let id = insert_execution(&db, ws, None, "a.perm", "f", "SUCCESS", at(1)).await;

    let app = common::app(&db).await;
    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user)
            .await
            .status(),
        403,
        "workflow:manage does not imply workflow:read"
    );
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user
        )
        .await
        .status(),
        403
    );

    // Control: the identical requests by a `workflow:read` holder succeed, so
    // the two 403s above are about the permission and not about the route.
    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), reader)
            .await
            .status(),
        200
    );
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            reader
        )
        .await
        .status(),
        200
    );
}

/// **404, never 403, for an execution in another workspace** — leaking
/// existence is the same defect as leaking content.
///
/// The caller here is a full `workflow:read` holder in workspace A, so the
/// refusal cannot be about permission: it is about the object belonging to
/// somebody else. A 403 would answer "that id is real, and you may not have
/// it", which is exactly the free existence oracle spec §8 records.
///
/// Three assertions, because a 404 alone is satisfiable by a broken route:
/// B's execution is **absent from A's list and from A's `total`** as well as
/// unreachable by id, and A's own execution is present in both — so a
/// repository that answered nothing to everybody fails here too.
#[tokio::test]
async fn an_execution_in_another_workspace_is_404_and_never_403() {
    let Some(db) = common::db().await else { return };
    let (ws_a, user_a) = common::seed_workspace(&db).await;
    let (ws_b, user_b) = common::seed_workspace(&db).await;
    common::grant(&db, ws_a, user_a, &["workflow:read"]).await;
    common::grant(&db, ws_b, user_b, &["workflow:read"]).await;

    let mine = insert_execution(&db, ws_a, None, "a.mine", "f", "SUCCESS", at(1)).await;
    let theirs = insert_execution(&db, ws_b, None, "b.theirs", "g", "SUCCESS", at(2)).await;

    let app = common::app(&db).await;

    let res = common::get(
        &app,
        &format!("/api/ws/{ws_a}/workflows/executions/{theirs}"),
        user_a,
    )
    .await;
    assert_eq!(
        res.status(),
        404,
        "a foreign execution must be indistinguishable from one that does not \
         exist — a 403 here confirms the id is real"
    );

    // Indistinguishable from an id that exists NOWHERE: same status, and the
    // body must not name the object either.
    let nowhere = common::get(
        &app,
        &format!("/api/ws/{ws_a}/workflows/executions/{}", Uuid::new_v4()),
        user_a,
    )
    .await;
    assert_eq!(nowhere.status(), 404);

    // The positive control, in B's own workspace: the very same id is a 200.
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws_b}/workflows/executions/{theirs}"),
            user_b
        )
        .await
        .status(),
        200,
        "the id is real; only A may not see it"
    );

    // And the list: A sees exactly its own row, and `total` agrees. Both
    // halves matter — the count is computed by a SECOND statement, so a
    // workspace predicate dropped from one of the two is visible only here.
    let body = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws_a}/workflows/executions?flow_key=g"),
            user_a,
        )
        .await,
    )
    .await;
    assert_eq!(body["total"], 0, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 0, "{body}");

    let body = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws_a}/workflows/executions"),
            user_a,
        )
        .await,
    )
    .await;
    let ids: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![mine.to_string()], "{body}");
    assert_eq!(body["total"], 1, "{body}");
}

// ── The module gate ─────────────────────────────────────────────────────────

/// The `workflows` entitlement gates these routes too. It is a path-prefix
/// rule (`WsContext`), so it covers them structurally — this asserts the
/// structure actually holds for a prefix nobody had mounted under before.
#[tokio::test]
async fn a_disabled_workflows_module_does_not_serve_the_execution_routes() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    common::disable_feature(&db, ws, "workflows").await;

    let id = insert_execution(&db, ws, None, "a.gate", "f", "SUCCESS", at(1)).await;
    let app = common::app(&db).await;
    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user)
            .await
            .status(),
        403,
        "the list route"
    );
    // **Both routes, because they are two handlers.** The gate is a path-prefix
    // rule and therefore structural, but "structural" is a claim about the
    // router as it happens to be mounted, and a route added under a prefix that
    // did not match would fail OPEN. Asserting only the list would certify the
    // prefix, not the pair.
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user
        )
        .await
        .status(),
        403,
        "the detail route"
    );
}

// ── The list DTO's shape ────────────────────────────────────────────────────

/// **`source` is the whole frozen YAML on every row.** The list must not carry
/// it; the detail must. Asserted on the *value*, not only on the key, so a
/// field renamed to something empty would not pass.
#[tokio::test]
async fn the_list_never_carries_source_and_the_detail_always_does() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let id = insert_execution(&db, ws, None, "a.src", "f", "SUCCESS", at(1)).await;

    let app = common::app(&db).await;
    let list =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user).await)
            .await;
    let row = &list["rows"][0];
    assert!(
        row.get("source").is_none(),
        "a fifty-row page would carry fifty YAML documents: {list}"
    );
    // The list row still identifies the flow without it.
    assert_eq!(row["namespace"], "a.src", "{list}");
    assert_eq!(row["flow_key"], "f", "{list}");
    assert_eq!(row["state"], "SUCCESS", "{list}");

    let detail = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(
        detail["source"], SEEDED_SOURCE,
        "the detail page draws its topology from the frozen source: {detail}"
    );
    assert!(detail["runs"].is_array(), "{detail}");
}

// ── Filters and pagination ──────────────────────────────────────────────────

/// Every filter narrows, and `state` takes several values because a run list
/// shows `FAILED` beside `RUNNING`.
///
/// `since` is inclusive and `until` exclusive, so consecutive windows tile
/// without double-counting the row on the boundary — asserted on the boundary
/// row itself, which is the only row that can tell the two halves apart.
#[tokio::test]
async fn the_filters_narrow_and_the_window_is_half_open() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    insert_execution(&db, ws, None, "a.one", "alpha", "SUCCESS", at(1)).await;
    insert_execution(&db, ws, None, "a.two", "beta", "FAILED", at(2)).await;
    insert_execution(&db, ws, None, "a.two", "gamma", "RUNNING", at(3)).await;

    let app = common::app(&db).await;

    let body = list(&app, ws, user, "namespace=a.two").await;
    assert_eq!(body["total"], 2, "{body}");

    let body = list(&app, ws, user, "flow_key=beta").await;
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(body["rows"][0]["flow_key"], "beta", "{body}");

    // Several states at once.
    let body = list(&app, ws, user, "state=FAILED,RUNNING").await;
    assert_eq!(body["total"], 2, "{body}");

    // A caller that deselected every state chip asks for nothing and gets
    // nothing — NOT everything, which is what an empty list silently treated
    // as "no filter" would return.
    let body = list(&app, ws, user, "state=").await;
    assert_eq!(
        body["total"], 0,
        "an empty state filter matches nothing: {body}"
    );

    // Half-open: `since` includes its own boundary row, `until` excludes it.
    let body = list(&app, ws, user, "since=2026-01-02T00:00:00Z").await;
    assert_eq!(body["total"], 2, "since is INCLUSIVE: {body}");
    let body = list(&app, ws, user, "until=2026-01-02T00:00:00Z").await;
    assert_eq!(body["total"], 1, "until is EXCLUSIVE: {body}");
}

/// Pagination: `limit`/`offset` page a stable, newest-first order, `total` is
/// the whole match rather than the page, and the **clamp is visible** — a
/// caller that asks for 1000 rows is told it got a page of 200, rather than
/// having to infer it.
#[tokio::test]
async fn pagination_pages_a_stable_order_and_reports_the_clamp() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    for d in 1..=3u32 {
        insert_execution(&db, ws, None, "a.page", "f", "SUCCESS", at(d)).await;
    }

    let app = common::app(&db).await;

    let first = list(&app, ws, user, "limit=2").await;
    assert_eq!(first["total"], 3, "total is the whole match: {first}");
    assert_eq!(first["limit"], 2, "{first}");
    assert_eq!(first["offset"], 0, "{first}");
    let page1: Vec<&str> = first["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["created_at"].as_str().unwrap())
        .collect();
    assert_eq!(
        page1,
        vec!["2026-01-03T00:00:00Z", "2026-01-02T00:00:00Z"],
        "newest first: {first}"
    );

    let second = list(&app, ws, user, "limit=2&offset=2").await;
    assert_eq!(second["total"], 3, "{second}");
    assert_eq!(second["offset"], 2, "{second}");
    assert_eq!(second["rows"].as_array().unwrap().len(), 1, "{second}");
    assert_eq!(
        second["rows"][0]["created_at"], "2026-01-01T00:00:00Z",
        "the two pages tile: {second}"
    );

    // The clamp, at both ends, reported rather than silent.
    let huge = list(&app, ws, user, "limit=1000").await;
    assert_eq!(huge["limit"], 200, "clamped to MAX_PAGE_LIMIT: {huge}");
    let tiny = list(&app, ws, user, "limit=0&offset=-5").await;
    assert_eq!(tiny["limit"], 1, "a zero page is a page of one: {tiny}");
    assert_eq!(tiny["offset"], 0, "a negative offset is zero: {tiny}");
}

/// **A trashed flow's executions are still listed**, and this route does not
/// pretend otherwise. `FlowRepo::delete` is soft, nothing in this crate issues
/// a `DELETE FROM`, so `ON DELETE SET NULL` never fires on the product's path
/// and `executions.flow_id` keeps pointing at the trashed draft.
///
/// This is also the test that would fail first if the list ever grew an INNER
/// join to `workflow.flows` — see this file's ruling in the task report.
#[tokio::test]
async fn a_trashed_flows_executions_are_still_listed() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let flow = common::draft(
        &repo,
        ws,
        user,
        "id: doomed\nnamespace: a.trash\ntasks:\n  - id: t\n    type: core.log\n",
    )
    .await;
    let ex = insert_execution(
        &db,
        ws,
        Some(flow.id),
        "a.trash",
        "doomed",
        "SUCCESS",
        at(1),
    )
    .await;
    repo.soft_delete(ws, flow.id).await.expect("soft delete");

    let app = common::app(&db).await;
    let body = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions?flow_id={}", flow.id),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(
        body["total"], 1,
        "the record of what ran outlives the draft: {body}"
    );
    assert_eq!(body["rows"][0]["id"], ex.to_string(), "{body}");
    assert_eq!(body["rows"][0]["flow_id"], flow.id.to_string(), "{body}");
}

// ── Two gates, not one; and what a malformed request gets ───────────────────

/// **A non-member is refused by `WsContext`, which is a DIFFERENT gate from
/// `require`** — and nothing in round 1 pinned it.
///
/// Both answer 403, so the status alone cannot tell them apart. The
/// discrimination is **revocation against a PDP that has already been built**:
/// `CedarPdp::build` snapshots the policy set, and this test's bus is a fresh
/// one nobody publishes to, so after the grant row is deleted the PDP still
/// believes the caller holds `workflow:read`. `require` would therefore ALLOW.
/// The 403 that comes back can only be `WsContext` step 4's membership query,
/// which reads the database live.
///
/// The 200 before the revocation is the control: without it, the 403 after it
/// could be any of the other four ways this route says 403.
#[tokio::test]
async fn a_revoked_member_is_refused_by_the_workspace_gate_and_not_by_the_pdp() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let id = insert_execution(&db, ws, None, "a.revoke", "f", "SUCCESS", at(1)).await;

    let app = common::app(&db).await;
    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user)
            .await
            .status(),
        200,
        "control: while the membership exists, the caller reads"
    );

    sqlx::query("DELETE FROM access_grants WHERE principal_id = $1 AND scope_id = $2")
        .bind(user.0)
        .bind(ws)
        .execute(&db.pool)
        .await
        .expect("revoke membership");

    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user)
            .await
            .status(),
        403,
        "membership is read live; the PDP snapshot still says `workflow:read`"
    );
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user
        )
        .await
        .status(),
        403
    );
}

/// **A's session against B's slug is refused, and B's execution stays
/// invisible.** Same gate as above — A holds no role in B — but a different
/// shape of attack from
/// [`an_execution_in_another_workspace_is_404_and_never_403`]: there the
/// caller used their OWN workspace in the path and a foreign id; here they put
/// the foreign workspace in the path. Both have to be closed and they are
/// closed by different code.
///
/// A never-member is a 403 rather than a 404 and that is not an existence
/// leak: workspace slugs are not secrets — they are in every URL the product
/// issues — and `WsContext` answers the same 403 whether or not B holds any
/// executions at all.
#[tokio::test]
async fn a_session_from_one_workspace_cannot_use_another_workspaces_slug() {
    let Some(db) = common::db().await else { return };
    let (ws_a, user_a) = common::seed_workspace(&db).await;
    let (ws_b, user_b) = common::seed_workspace(&db).await;
    common::grant(&db, ws_a, user_a, &["workflow:read"]).await;
    common::grant(&db, ws_b, user_b, &["workflow:read"]).await;
    let theirs = insert_execution(&db, ws_b, None, "b.slug", "g", "SUCCESS", at(1)).await;

    let app = common::app(&db).await;
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws_b}/workflows/executions"),
            user_a
        )
        .await
        .status(),
        403,
        "A is not a member of B"
    );
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws_b}/workflows/executions/{theirs}"),
            user_a
        )
        .await
        .status(),
        403
    );
    // Controls: B reaches it, and A reaches A's own list. Without these the two
    // 403s above could be a router that serves nobody.
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws_b}/workflows/executions/{theirs}"),
            user_b
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws_a}/workflows/executions"),
            user_a
        )
        .await
        .status(),
        200
    );
}

/// **A non-UUID `{id}` is a 400, and an anonymous caller still gets 401.**
///
/// The 400 is axum's `Path` extractor refusing before any handler runs. The
/// ORDER is the part worth pinning: `WsContext` (and therefore `Auth`) runs
/// first, so a caller with no session is told 401 and learns nothing about
/// whether the path would have parsed. If those two ever swapped, an
/// unauthenticated prober could distinguish a well-formed id from a malformed
/// one — small, but it is the same class of oracle this file exists to close.
#[tokio::test]
async fn a_malformed_execution_id_is_400_and_authentication_still_comes_first() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let app = common::app(&db).await;
    assert_eq!(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/not-a-uuid"),
            user
        )
        .await
        .status(),
        400,
        "the path parameter does not parse"
    );
    assert_eq!(
        common::anon_get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/not-a-uuid")
        )
        .await
        .status(),
        401,
        "authentication is decided BEFORE the path parses, so this leaks nothing"
    );
}

/// **A repeated `state=` key is REFUSED, not silently narrowed** — the
/// mechanism this parameter's documentation now states.
///
/// Round 1 shipped the opposite claim in `routes.rs`, and from there into
/// `openapi.json` and the generated TypeScript JSDoc, where a frontend author
/// would have read it. The design (comma-separation) was right; the reason
/// given for it was false. This test is here so the reason cannot drift again
/// without something going red: **a decision and its mechanism are two claims,
/// and the next person reasons from the mechanism.**
///
/// Asserted with the single-key control beside it, so the 400 is attributable
/// to the repetition and not to the values.
#[tokio::test]
async fn a_repeated_state_key_is_refused_rather_than_silently_narrowed() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    insert_execution(&db, ws, None, "a.dup", "alpha", "SUCCESS", at(1)).await;
    insert_execution(&db, ws, None, "a.dup", "beta", "FAILED", at(2)).await;

    let app = common::app(&db).await;
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/executions?state=SUCCESS&state=FAILED"),
        user,
    )
    .await;
    assert_eq!(
        res.status(),
        400,
        "serde_urlencoded rejects a duplicate field; it does NOT keep the last"
    );
    let body = common::body_text(res).await;
    assert!(
        body.contains("duplicate field"),
        "the refusal names the repetition: {body}"
    );

    // Control 1: ONE key with the same two values is the supported spelling
    // and returns both rows.
    let ok = list(&app, ws, user, "state=SUCCESS,FAILED").await;
    assert_eq!(ok["total"], 2, "{ok}");
    // Control 2: a single key is fine, so the 400 above is about repetition.
    let one = list(&app, ws, user, "state=FAILED").await;
    assert_eq!(one["total"], 1, "{one}");
}

// ── The DTO contract W7 consumes ────────────────────────────────────────────

/// Seed one `workflow.task_runs` row in which **every column carries a value
/// distinct from every other column's**.
///
/// Distinctness is the whole instrument. A fixture that left the text columns
/// `NULL`, or gave two of them the same string, cannot tell a DTO that maps
/// `rendered` to `rendered` from one that maps it to `rendered_run_if`: both
/// produce the same JSON. With distinct values the swap is visible, which is
/// exactly the mutation that survived the first version of this guard.
///
/// `resume_with` is the one column whose value is not free text:
/// `workflow_task_runs_resume_with_known` constrains it to five spellings, so
/// it is seeded with `TERMINAL:WARNING` — still distinct from every other
/// value here. The constraint refused the invented string, which is the schema
/// doing its job.
async fn insert_wired_task_run(db: &Db, execution: Uuid) {
    sqlx::query(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, \
            rendered, rendered_run_if, state, history, attempts, not_before_ms, \
            claimed_at, outputs, message, created_seq, ended_seq, resume_with, started_at) \
         VALUES ($1, 7, 9, 'task-id-value', 'type-id-value', 11, 'value-value', \
                 'rendered-value', 'rendered-run-if-value', 'RUNNING', \
                 '[\"CREATED\",\"RUNNING\"]'::jsonb, \
                 '[{\"history\":[\"CREATED\"],\"started_at\":13}]'::jsonb, \
                 17, '2026-02-03T04:05:06Z'::timestamptz, \
                 '{\"outputs-key\":\"outputs-value\"}'::jsonb, 'message-value', \
                 19, 23, 'TERMINAL:WARNING', '2026-02-01T02:03:04Z'::timestamptz)",
    )
    .bind(execution)
    .execute(&db.pool)
    .await
    .expect("insert wired task run");
}

/// **Every column of the repository row reaches the wire, under its own
/// name, carrying its own value.**
///
/// Step 2 of this task is a column list, so this asserts the list: `TaskRunRow`
/// carries all twenty-four columns and `ExecutionRow` all of its own minus
/// `source`, and a DTO that quietly dropped one would otherwise be discovered
/// by W7 rather than here.
///
/// **This test exists because a mutation survived, and its SECOND version
/// exists because a second one did.** Removing `rendered_run_if` from
/// `TaskRunDto` left the whole suite green: the gantt test reads six fields of
/// a run and the other sixteen were asserted by nothing at all. The first
/// version of this guard compared the key SET and killed that — but
/// **swapping the sources of `rendered` and `rendered_run_if` survived it
/// 2/2, green and byte-identical**, because a key set pins names and says
/// nothing about the mapping. A value can still reach the wire under the
/// wrong key. So this version pins **both**: the key set (an extra or missing
/// field) and each field's value against the column it was seeded from (a
/// crossed wire).
///
/// There is deliberately **no assertion that the expected list has 22
/// entries**. The first version had one, and it could not fail: it compared a
/// literal in this file against a count of the same literal. An assertion that
/// cannot fail is a guard nobody has seen.
#[tokio::test]
async fn every_repository_column_reaches_the_wire() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let id = insert_execution(&db, ws, None, "a.keys", "f", "RUNNING", at(1)).await;
    insert_wired_task_run(&db, id).await;
    // W7b Task 1's three execution columns, each with a value of its own. A
    // RUNNING row cannot carry an `ended_at` (migration 0180's CHECK), so the
    // end is asserted `null` -- still distinct from the start beside it, which
    // is what would show a crossed wire.
    let replayed_from =
        insert_execution(&db, ws, None, "a.keys-source", "src", "SUCCESS", at(1)).await;
    sqlx::query(
        "UPDATE workflow.executions SET started_at = '2026-01-01T00:00:05Z', \
                replayed_from_execution_id = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(replayed_from)
    .execute(&db.pool)
    .await
    .expect("seed the W7b columns");

    let app = common::app(&db).await;
    let detail = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user,
        )
        .await,
    )
    .await;

    // ---- the key sets ----
    //
    // `ExecutionRow`'s columns minus `workspace_id`, plus the detail route's
    // own two. `source` is here and NOT in the list row below.
    let want_execution: Vec<&str> = vec![
        "id",
        "flow_id",
        "namespace",
        "flow_key",
        "state",
        "history",
        "inputs",
        "seq",
        "execution_trigger",
        "trigger_execution_id",
        "trigger_run_ix",
        "parent_execution_id",
        "parent_run_ix",
        "loop_index",
        "loop_value",
        "attempt_number",
        "origin_offset_ms",
        "replayed_from_execution_id",
        "started_at",
        "ended_at",
        "created_at",
        "updated_at",
        "source",
        "runs",
        // Not a repository column: the PDP's `workflow:manage` answer (W7b Task 3).
        "can_manage",
    ];
    let got: std::collections::BTreeSet<&str> = detail
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let want: std::collections::BTreeSet<&str> = want_execution.iter().copied().collect();
    assert_eq!(got, want, "the detail DTO's key set: {detail}");

    let want_run: Vec<&str> = vec![
        "id",
        "execution_id",
        "run_ix",
        "task_ix",
        "task_id",
        "type_id",
        "parent_ix",
        "value",
        "rendered",
        "rendered_run_if",
        "state",
        "history",
        "attempts",
        "not_before_ms",
        "claimed_at",
        "outputs",
        "message",
        "created_seq",
        "ended_seq",
        "resume_with",
        "started_at",
        "ended_at",
        "created_at",
        "updated_at",
    ];
    let run = &detail["runs"][0];
    let got_run: std::collections::BTreeSet<&str> = run
        .as_object()
        .expect("one task run")
        .keys()
        .map(|k| k.as_str())
        .collect();
    let want_run_set: std::collections::BTreeSet<&str> = want_run.iter().copied().collect();
    assert_eq!(
        got_run, want_run_set,
        "the task-run DTO's key set: {detail}"
    );

    // ---- the mapping ----
    //
    // Each field against the column `insert_wired_task_run` seeded it from.
    // Every seeded value is distinct, so a crossed pair of wires shows up as
    // two failures rather than as none. `id`, `created_at` and `updated_at`
    // are database-assigned and are checked for shape rather than value.
    assert_eq!(run["execution_id"], id.to_string(), "{run}");
    assert_eq!(run["run_ix"], 7, "{run}");
    assert_eq!(run["task_ix"], 9, "{run}");
    assert_eq!(run["task_id"], "task-id-value", "{run}");
    assert_eq!(run["type_id"], "type-id-value", "{run}");
    assert_eq!(run["parent_ix"], 11, "{run}");
    assert_eq!(run["value"], "value-value", "{run}");
    assert_eq!(run["rendered"], "rendered-value", "{run}");
    assert_eq!(
        run["rendered_run_if"], "rendered-run-if-value",
        "a value under the wrong key is the mutation the key set could not see: {run}"
    );
    assert_eq!(run["state"], "RUNNING", "{run}");
    assert_eq!(
        run["history"],
        serde_json::json!(["CREATED", "RUNNING"]),
        "{run}"
    );
    assert_eq!(
        run["attempts"],
        serde_json::json!([{"history": ["CREATED"], "started_at": 13}]),
        "{run}"
    );
    assert_eq!(run["not_before_ms"], 17, "{run}");
    assert_eq!(run["claimed_at"], "2026-02-03T04:05:06Z", "{run}");
    assert_eq!(
        run["outputs"],
        serde_json::json!({"outputs-key": "outputs-value"}),
        "{run}"
    );
    assert_eq!(run["message"], "message-value", "{run}");
    assert_eq!(run["created_seq"], 19, "{run}");
    assert_eq!(run["ended_seq"], 23, "{run}");
    assert_eq!(run["resume_with"], "TERMINAL:WARNING", "{run}");
    assert_eq!(run["started_at"], "2026-02-01T02:03:04Z", "{run}");
    assert_eq!(run["ended_at"], Value::Null, "{run}");
    assert!(
        run["id"]
            .as_str()
            .and_then(|v| v.parse::<Uuid>().ok())
            .is_some(),
        "{run}"
    );
    assert!(
        run["created_at"].is_string() && run["updated_at"].is_string(),
        "{run}"
    );

    // The execution side, same idea: the columns this fixture controls.
    assert_eq!(detail["id"], id.to_string(), "{detail}");
    assert_eq!(detail["namespace"], "a.keys", "{detail}");
    assert_eq!(detail["flow_key"], "f", "{detail}");
    assert_eq!(detail["state"], "RUNNING", "{detail}");
    assert_eq!(detail["source"], SEEDED_SOURCE, "{detail}");
    assert_eq!(detail["created_at"], "2026-01-01T00:00:00Z", "{detail}");
    assert_eq!(detail["started_at"], "2026-01-01T00:00:05Z", "{detail}");
    assert_eq!(detail["ended_at"], Value::Null, "{detail}");
    assert_eq!(
        detail["replayed_from_execution_id"],
        replayed_from.to_string(),
        "{detail}"
    );

    // `can_manage` is the caller's, on the detail only: a list row answers no PDP question.
    assert_eq!(detail["can_manage"], false, "a reader: {detail}");

    // ---- and the list row: the same execution fields, WITHOUT source/runs/can_manage ----
    let page = list(&app, ws, user, "namespace=a.keys").await;
    let row = &page["rows"][0];
    let got_list: std::collections::BTreeSet<&str> = row
        .as_object()
        .expect("one row")
        .keys()
        .map(|k| k.as_str())
        .collect();
    let want_list: std::collections::BTreeSet<&str> = want_execution
        .iter()
        .copied()
        .filter(|k| !matches!(*k, "source" | "runs" | "can_manage"))
        .collect();
    assert_eq!(got_list, want_list, "the list DTO's key set: {page}");
    // The flattened parent's fields carry the same values through the list
    // route as through the detail route -- two `From` impls, one contract.
    for k in [
        "id",
        "namespace",
        "flow_key",
        "state",
        "created_at",
        "started_at",
        "ended_at",
        "replayed_from_execution_id",
    ] {
        assert_eq!(
            row[k], detail[k],
            "list and detail disagree about {k}: {page}"
        );
    }
}

// ── The gantt row ───────────────────────────────────────────────────────────

/// One row of a rendered gantt chart, computed **from the execution DTO and
/// nothing else** — no database handle, no second request, no knowledge of
/// what this test seeded.
#[derive(Debug)]
#[allow(dead_code)]
struct GanttRow {
    label: String,
    /// Nesting: the parent run's `run_ix`, or `None` at the top level.
    parent: Option<i64>,
    /// Wall clock, absolute.
    start: DateTime<Utc>,
    /// **`None` while the run is still in flight** — an open-ended bar that a
    /// chart draws to the right edge of its axis, not a bar of zero length and
    /// not a panic. See [`gantt_rows`].
    end: Option<DateTime<Utc>>,
    /// Logical time — the executor's own rounds, which is what makes two bars
    /// comparable when their wall-clock spans are microseconds apart.
    created_seq: i64,
    /// `None` for the same reason [`Self::end`] is.
    ended_seq: Option<i64>,
    /// The bar's colour.
    state: String,
}

/// Build every gantt row of an execution from its DTO.
///
/// **The fields a bar cannot exist without are taken with `expect`**, so a DTO
/// missing one does not degrade into a chart with a hole — it fails by name.
/// That is the point of the exercise: W7 must not discover a missing field
/// after this package closes.
///
/// **The fields that are legitimately absent WHILE A RUN IS IN FLIGHT are
/// taken as `Option`, and that distinction is the whole correction of round
/// 1.** `task_runs.ended_seq` is nullable and is `NULL` for exactly as long as
/// the run has not terminated; the first version of this recipe did
/// `.expect("run.ended_seq")` on it and was proved only against a **terminal**
/// execution. Handed to W7 unchanged it would have panicked on the one case a
/// gantt exists for — watching an execution in flight. A never-started run was
/// already handled (the `attempts` fallback); the in-flight one was not, and
/// the fixture below now contains one.
///
/// `start` is derived rather than read: a run's wall-clock start is
/// `execution.created_at + attempts[0].started_at`, because a `Tick` in this
/// engine is **milliseconds since the execution's `created_at`** (see
/// `worker::claim::load_with`'s `now_ms`). A run with no attempts is a
/// flowable the executor drives itself, and its own `created_at` is its start.
///
/// `end` is `updated_at` only once the run has terminated. `updated_at` moves
/// on every write, so on a live run it is "when we last heard from it", which
/// is not an end; `ended_seq` being `NULL` is the authority on whether the run
/// has one at all, so it — and not the state name — is what decides.
fn gantt_rows(dto: &Value) -> Vec<GanttRow> {
    let origin: DateTime<Utc> = dto["created_at"]
        .as_str()
        .expect("execution.created_at")
        .parse()
        .expect("execution.created_at parses");
    dto["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .map(|r| {
            let created_at: DateTime<Utc> = r["created_at"]
                .as_str()
                .expect("run.created_at")
                .parse()
                .expect("run.created_at parses");
            let updated_at: DateTime<Utc> = r["updated_at"]
                .as_str()
                .expect("run.updated_at")
                .parse()
                .expect("run.updated_at parses");
            let start = r["attempts"]
                .as_array()
                .expect("run.attempts")
                .first()
                .map(|a| {
                    origin
                        + chrono::Duration::milliseconds(
                            a["started_at"].as_i64().expect("attempt.started_at"),
                        )
                })
                .unwrap_or(created_at);
            // `null` and a missing key are different failures and only one of
            // them is legitimate: a run in flight has `ended_seq: null`, while
            // a DTO that dropped the field has no key at all. `get` tells them
            // apart; `as_i64()` alone would not.
            let ended_seq = r
                .get("ended_seq")
                .expect("run.ended_seq must be PRESENT, even when null")
                .as_i64();
            GanttRow {
                label: r["task_id"].as_str().expect("run.task_id").to_string(),
                parent: r["parent_ix"].as_i64(),
                start,
                end: ended_seq.map(|_| updated_at),
                created_seq: r
                    .get("created_seq")
                    .expect("run.created_seq must be present")
                    .as_i64()
                    .expect("a run that exists was created in some round"),
                ended_seq,
                state: r["state"].as_str().expect("run.state").to_string(),
            }
        })
        .collect()
}

/// **The gantt data is already there — this asserts it rather than assuming
/// it, for a terminated run AND for one still in flight.**
///
/// Driven through the **real worker** on a scratch database, so every field
/// [`gantt_rows`] reads has the shape production writes and not the shape a
/// fixture in this file chose.
///
/// **The flow is `Log` then `Pause`, and the pause is an hour long.** That is
/// what makes the fixture contain both elements of the class: by the time the
/// execution settles, `first` has terminated (attempts, both seqs, a real end)
/// and `pause` is in flight (no `ended_seq`, therefore no end) — and it will
/// stay that way for an hour, so nothing here races. A two-`Log` flow, which
/// is what round 1 used, reaches `SUCCESS` with every run terminal and cannot
/// see the in-flight case at all.
#[sqlx::test(migrations = "../../migrations")]
async fn a_gantt_row_is_computable_from_the_execution_dto_alone(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let source = "id: gantt\nnamespace: a.gantt\ntasks:\n  \
                  - id: first\n    type: io.kestra.plugin.core.log.Log\n    message: one\n  \
                  - id: pause\n    type: io.kestra.plugin.core.flow.Pause\n    \
                  pauseDuration: PT1H\n";
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, source, serde_json::json!({}))
            .await
            .expect("start execution");

    // Tick until the execution stops moving: the `Log` runs and terminates,
    // the `Pause` opens and then nothing further can happen for an hour.
    let reg = AlwaysSucceeds;
    let mut ticks = 0;
    loop {
        egret_workflow::worker::tick(&pool, 8, &reg)
            .await
            .expect("tick");
        let runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND ended_seq IS NOT NULL",
        )
        .bind(ex)
        .fetch_one(&pool)
        .await
        .expect("count terminated runs");
        let total: i64 =
            sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1")
                .bind(ex)
                .fetch_one(&pool)
                .await
                .expect("count runs");
        if total == 2 && runs == 1 {
            break;
        }
        ticks += 1;
        assert!(
            ticks < 50,
            "never reached one-terminated-one-in-flight; {runs} of {total} terminated"
        );
    }

    let app = common::app(&db).await;
    let dto = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{ex}"),
            user,
        )
        .await,
    )
    .await;

    // **`history` carries NO timestamps**, on a run or on the execution — it is
    // an array of state NAMES, written by `worker::claim`'s `states_json`. It
    // is NOT Kestra's `State.History` (`{state, date}`), and a gantt drawn from
    // it alone would have an order and no clock. Asserted rather than
    // described, because the plan this task was given said the opposite and
    // nothing in the tree contradicted it.
    for run in dto["runs"].as_array().unwrap() {
        for entry in run["history"].as_array().expect("run history") {
            assert!(
                entry.is_string(),
                "a run's history entry is a bare state name: {run}"
            );
        }
    }
    for entry in dto["history"].as_array().expect("execution history") {
        assert!(entry.is_string(), "{dto}");
    }
    // The OpenAPI document types these as `Vec<String>` and `Vec<AttemptDto>`
    // while the DTO passes the stored `jsonb` through: the wire must decode
    // into the documented types, or the generated client lies.
    serde_json::from_value::<Vec<String>>(dto["history"].clone())
        .expect("execution history is Vec<String>");
    for run in dto["runs"].as_array().unwrap() {
        serde_json::from_value::<Vec<String>>(run["history"].clone())
            .expect("run history is Vec<String>");
        let attempts: Vec<egret_workflow::routes::AttemptDto> =
            serde_json::from_value(run["attempts"].clone()).expect("attempts are Vec<AttemptDto>");
        for a in &attempts {
            assert!(!a.history.is_empty(), "an attempt has a history: {run}");
        }
    }

    let rows = gantt_rows(&dto);
    assert_eq!(rows.len(), 2, "one bar per task run: {dto}");

    // W7b Task 1: the real times beside the recipe's derived ones. The
    // terminated run has both; the open pause has a start and NO end -- the
    // in-flight case `updated_at` could never express.
    let (first, pause) = (&dto["runs"][0], &dto["runs"][1]);
    assert!(
        first["started_at"].is_string() && first["ended_at"].is_string(),
        "{first}"
    );
    assert!(
        pause["started_at"].is_string() && pause["ended_at"].is_null(),
        "{pause}"
    );
    assert!(
        dto["started_at"].is_string() && dto["ended_at"].is_null(),
        "{dto}"
    );

    // Labels, in the executor's own creation order (`run_ix`), which is the
    // order the detail route promises.
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, vec!["first", "pause"], "{dto}");

    // ---- the terminated bar ----
    let done = &rows[0];
    assert_eq!(done.state, "SUCCESS", "{done:?}");
    let done_end = done.end.expect("a terminated run has an end: {done:?}");
    assert!(
        done.start <= done_end,
        "a bar cannot end before it starts: {done:?}"
    );
    let done_ended_seq = done.ended_seq.expect("a terminated run has an ended_seq");
    assert!(
        done.created_seq <= done_ended_seq,
        "a run cannot terminate in a round before the one that created it: {done:?}"
    );
    // The wall-clock start came from `attempts`, not from the fallback: a
    // runnable run that succeeded first time has exactly one attempt, and
    // without this an empty `attempts` would quietly supply `created_at`
    // instead and every assertion above would still pass.
    let attempts = dto["runs"][0]["attempts"]
        .as_array()
        .expect("attempts is an array");
    assert_eq!(attempts.len(), 1, "{dto}");
    assert!(
        attempts[0]["started_at"].is_i64(),
        "the only wall-clock start a run carries: {dto}"
    );

    // ---- the in-flight bar: the element round 1's fixture did not contain ----
    let live = &rows[1];
    assert!(
        live.end.is_none(),
        "a run still in flight has no end, and `updated_at` is not one: {live:?}"
    );
    assert!(live.ended_seq.is_none(), "{live:?}");
    assert_ne!(
        live.state, "SUCCESS",
        "the pause is still open, so this bar is not a terminal colour: {live:?}"
    );
    // It still has everything a bar NEEDS: a label, a start, and a place in
    // logical time. An open-ended bar is drawable; a panic is not.
    assert!(!live.label.is_empty(), "{live:?}");
    assert!(
        live.created_seq >= done_ended_seq,
        "`pause` was created no earlier than the round `first` ended in: {live:?}"
    );

    // The whole chart fits inside the execution's own span, so an axis drawn
    // from the execution row alone contains every bar that HAS an end; the
    // in-flight one starts inside it and runs off the right edge.
    let exec_start: DateTime<Utc> = dto["created_at"].as_str().unwrap().parse().unwrap();
    let exec_end: DateTime<Utc> = dto["updated_at"].as_str().unwrap().parse().unwrap();
    for row in &rows {
        assert!(
            row.start >= exec_start,
            "bar {row:?} starts before {exec_start}"
        );
        if let Some(end) = row.end {
            assert!(end <= exec_end, "bar {row:?} ends after {exec_end}");
        }
    }
}

/// A registry that answers `SUCCESS` to everything — the gantt test is about
/// the *shape* of what a completed execution leaves behind, not about what any
/// handler does.
struct AlwaysSucceeds;

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for AlwaysSucceeds {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        Ok(egret_workflow::worker::Outcome::success())
    }
    async fn cancel(&self, task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing here declares a `timeout:`, so {} is never killed",
            task.task_id
        );
    }
}

// ── The WRITE surface (W10 Task 5) ──────────────────────────────────────────
//
// ```text
// POST /api/ws/{workspace}/workflows/flows/{id}/executions
// POST /api/ws/{workspace}/workflows/executions/{id}/resume
// POST /api/ws/{workspace}/workflows/executions/{id}/replay
// ```
//
// **The gate test here is the MIRROR IMAGE of
// [`workflow_manage_alone_cannot_read_executions`] above, and the asymmetry is
// the whole point.** That test proves a READ gate with a `:manage`-only holder,
// because a caller with no grants is refused by a handler that checks nothing.
// A WRITE gate needs the opposite witness: a `:read`-only holder is exactly the
// principal who would SUCCEED if `WF_MANAGE` were swapped for `WF_READ` or the
// `require` removed, so
// [`workflow_read_alone_cannot_start_resume_or_replay`] fails when the claim is
// false. A bare unprivileged user would pass that test whether or not the gate
// exists.

/// A one-task flow, startable and not finishable without a worker.
fn runnable_source(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: a.write\ntasks:\n  \
         - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

/// A flow that is a single **unbounded** `Pause` — neither `pauseDuration:`
/// nor `timeout:`. `executor::Resume::Never`, which is the case the resume
/// route used to leave alone; see [`a_resume_releases_every_bound_of_pause_at_once`].
fn unbounded_pause_source(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: a.write\ntasks:\n  \
         - id: pause\n    type: io.kestra.plugin.core.flow.Pause\n"
    )
}

/// Insert one terminal execution with an explicit frozen `source` and `inputs`.
///
/// [`insert_execution`] above hard-codes [`SEEDED_SOURCE`] and leaves `inputs`
/// at its column default, and the replay tests need both to be values a DRAFT
/// does not hold — that is the only way to tell "replay ran the frozen source"
/// from "replay re-read the flow".
async fn insert_execution_with(
    db: &Db,
    ws: Uuid,
    flow_id: Option<Uuid>,
    state: &str,
    source: &str,
    inputs: Value,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs) \
         VALUES ($1, $2, 'a.write', 'frozen', $3, $4, $5, $6) RETURNING id",
    )
    .bind(ws)
    .bind(flow_id)
    .bind(source)
    .bind(state)
    .bind(serde_json::json!(["CREATED", state]))
    .bind(inputs)
    .fetch_one(&db.pool)
    .await
    .expect("insert execution with source")
}

/// The `workflow.executions` row's `state` column, read straight out of the
/// table rather than off a DTO — the resume tests assert about a state the
/// route is supposed NOT to have changed, and reading it back through the same
/// handler under test would let one bug hide another.
async fn state_of(db: &Db, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("read execution state")
}

/// **401 from every write route**, with a positive control on each so a
/// mistyped path cannot pass as a refusal: a 404 is not a 401, but neither is
/// it a route.
#[tokio::test]
async fn an_unauthenticated_caller_gets_401_from_every_write_route() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let flow = common::seed_flow(&db, ws, user, &runnable_source("anon-write"), "draft").await;
    // **Linked to a real, open flow**, because `replay` gates on the flow's
    // current state (fix round 1) and a flow-less execution is refused 404. An
    // orphan fixture here would make the replay control below measure the
    // orphan arm instead of the 401 this test is about.
    let done = insert_execution_with(
        &db,
        ws,
        Some(flow),
        "SUCCESS",
        &runnable_source("anon-write"),
        serde_json::json!({}),
    )
    .await;

    let app = common::app(&db).await;
    for path in [
        format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        format!("/api/ws/{ws}/workflows/executions/{done}/resume"),
        format!("/api/ws/{ws}/workflows/executions/{done}/replay"),
    ] {
        assert_eq!(
            common::anon_post(&app, &path).await.status(),
            401,
            "{path} must refuse an anonymous caller"
        );
    }

    // The controls, same three URLs with a session. Each is a DIFFERENT
    // success-shaped answer and none of them is 404, which is what says the
    // routes exist: start creates, resume refuses a SUCCESS execution, replay
    // creates.
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
            user
        )
        .await
        .status(),
        201
    );
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{done}/resume"),
            user
        )
        .await
        .status(),
        409
    );
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{done}/replay"),
            user
        )
        .await
        .status(),
        201
    );
}

/// **`workflow:read` alone cannot start, resume or replay.** The discriminating
/// witness for a write gate — see this section's comment.
///
/// Both arms matter and they are different claims: the `:read` holder's three
/// 403s say the gate is at least `WF_MANAGE`, and the `:manage` holder's three
/// non-403s say it is not something stricter and that the routes are reachable
/// at all. Swap `WF_MANAGE` for `WF_READ` on any one handler and the first arm
/// fails on exactly that route; delete the `require` and it fails there too.
#[tokio::test]
async fn workflow_read_alone_cannot_start_resume_or_replay() {
    let Some(db) = common::db().await else { return };
    let (ws, reader) = common::seed_workspace(&db).await;
    common::grant(&db, ws, reader, &["workflow:read"]).await;
    let (_ws2, manager) = common::seed_workspace(&db).await;
    common::grant(&db, ws, manager, &["workflow:manage"]).await;

    let flow = common::seed_flow(&db, ws, reader, &runnable_source("gate-write"), "draft").await;
    // Linked, for `an_unauthenticated_caller_gets_401_from_every_write_route`'s
    // stated reason: the `:manage` control must reach the 201, not the orphan
    // 404.
    let done = insert_execution_with(
        &db,
        ws,
        Some(flow),
        "SUCCESS",
        &runnable_source("gate-write"),
        serde_json::json!({}),
    )
    .await;
    let start = format!("/api/ws/{ws}/workflows/flows/{flow}/executions");
    let resume = format!("/api/ws/{ws}/workflows/executions/{done}/resume");
    let replay = format!("/api/ws/{ws}/workflows/executions/{done}/replay");

    let app = common::app(&db).await;
    for path in [&start, &resume, &replay] {
        assert_eq!(
            common::post_no_body(&app, path, reader).await.status(),
            403,
            "{path}: workflow:read does not imply workflow:manage"
        );
    }
    // Nothing was created by the three refusals -- a 403 with a side effect is
    // the failure a status assertion alone cannot see.
    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "the three refused writes must not have created anything beside the seeded execution"
    );

    // The `:manage` control. Three different non-403 answers, for this
    // section's stated reason.
    assert_eq!(
        common::post_no_body(&app, &start, manager).await.status(),
        201
    );
    assert_eq!(
        common::post_no_body(&app, &resume, manager).await.status(),
        409
    );
    assert_eq!(
        common::post_no_body(&app, &replay, manager).await.status(),
        201
    );
}

/// **404, never 403, for a flow or an execution in another workspace — and
/// nothing is created.** A 403 would confirm the id is real; a 404 that still
/// started something would be worse than either, and only a count can see it.
#[tokio::test]
async fn writes_against_another_workspace_are_404_and_start_nothing() {
    let Some(db) = common::db().await else { return };
    let (mine, user) = common::seed_workspace(&db).await;
    common::grant(&db, mine, user, &["workflow:manage"]).await;
    let (theirs, other) = common::seed_workspace(&db).await;
    common::grant(&db, theirs, other, &["workflow:manage"]).await;

    let their_flow =
        common::seed_flow(&db, theirs, other, &runnable_source("foreign"), "draft").await;
    let their_execution = insert_execution_with(
        &db,
        theirs,
        Some(their_flow),
        "SUCCESS",
        &runnable_source("foreign"),
        serde_json::json!({}),
    )
    .await;

    let app = common::app(&db).await;
    for path in [
        format!("/api/ws/{mine}/workflows/flows/{their_flow}/executions"),
        format!("/api/ws/{mine}/workflows/executions/{their_execution}/resume"),
        format!("/api/ws/{mine}/workflows/executions/{their_execution}/replay"),
    ] {
        assert_eq!(
            common::post_no_body(&app, &path, user).await.status(),
            404,
            "{path} must be indistinguishable from an id that never existed"
        );
    }
    assert_eq!(common::execution_count(&db, mine).await, 0);
    assert_eq!(
        common::execution_count(&db, theirs).await,
        1,
        "the seeded execution, and nothing a foreign caller made"
    );

    // Controls, in the OWNING workspace: the same three ids answer 201/409/201
    // there, so the 404s above are about the workspace and not about the ids.
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{theirs}/workflows/flows/{their_flow}/executions"),
            other
        )
        .await
        .status(),
        201
    );
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{theirs}/workflows/executions/{their_execution}/resume"),
            other
        )
        .await
        .status(),
        409
    );
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{theirs}/workflows/executions/{their_execution}/replay"),
            other
        )
        .await
        .status(),
        201
    );
}

/// **A start runs the DRAFT's source and the caller's inputs, through
/// `worker::start_execution` and not a second creation path.**
///
/// Three things say it is that function rather than an INSERT of the same
/// shape: the row carries a `NULL` `execution_trigger` (the webhook's caller is
/// the only one that fills it), the first settle has already run so a task run
/// exists, and `seq` has moved off zero. An open-coded INSERT would produce a
/// row with none of those.
#[tokio::test]
async fn a_start_runs_the_drafts_source_with_the_inputs_the_caller_sent() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let source = runnable_source("start-shape");
    let flow = common::seed_flow(&db, ws, user, &source, "draft").await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
        serde_json::json!({ "inputs": { "who": "world", "n": 3 } }),
    )
    .await;
    assert_eq!(res.status(), 201);
    let dto = common::json(res).await;
    let id: Uuid = dto["id"].as_str().expect("id").parse().expect("uuid");

    assert_eq!(
        dto["source"].as_str(),
        Some(source.as_str()),
        "the execution runs the draft's frozen source"
    );
    assert_eq!(
        dto["flow_id"].as_str().map(str::to_string),
        Some(flow.to_string())
    );

    let (trigger, inputs, seq, runs): (Option<Value>, Value, i64, i64) = sqlx::query_as(
        "SELECT e.execution_trigger, e.inputs, e.seq, \
                (SELECT count(*) FROM workflow.task_runs r WHERE r.execution_id = e.id) \
           FROM workflow.executions e WHERE e.id = $1",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("read the started row");
    assert_eq!(
        trigger, None,
        "an API start records no trigger; a webhook does"
    );
    // **The MAPPING, not the key set.** `{"who":"world","n":3}` and
    // `{"who":3,"n":"world"}` have the same keys, so an assertion about keys
    // alone would survive a handler that swapped the values it forwarded.
    assert_eq!(inputs, serde_json::json!({ "who": "world", "n": 3 }));
    assert!(seq > 0, "the first settle ran; seq is {seq}");
    // W13a Task 3: every caller that is not the Flow-trigger processor starts
    // at the bottom of a chain -- no source link, depth 0, no outbox event.
    let chain: (Option<Uuid>, i32, Option<i64>) = sqlx::query_as(
        "SELECT trigger_execution_id, execution_depth, flow_trigger_event_id \
           FROM workflow.executions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("read the chain columns");
    assert_eq!(
        chain,
        (None, 0, None),
        "an API start is not a Flow-trigger fire"
    );

    assert_eq!(runs, 1, "the first settle created the Log run");

    // **The audit record, asserted rather than emitted into the void.** Mutation
    // M11 replaced this whole payload with `{"mutated": …}` and the suite stayed
    // `ok. 24 passed; 0 failed` — byte-identical to the control — because
    // nothing read the sink. The MAPPING is asserted, not the key set: three
    // keys with their values swapped would satisfy a key-set assertion.
    let audit = common::audit_mutations(&app);
    assert_eq!(audit.len(), 1, "one start, one audit record");
    assert_eq!(audit[0].0, "workflow_execution:start");
    assert_eq!(
        audit[0].1.as_deref(),
        Some(id.to_string().as_str()),
        "the record names the execution that was created"
    );
    assert_eq!(
        audit[0].2,
        serde_json::json!({
            "flow_id": flow,
            "namespace": "a.write",
            "flow_key": "start-shape",
        })
    );
}

/// **`inputs` that is not an object is REFUSED, not coerced to `{}`.**
///
/// `start_execution_with_trigger` writes `{}` for any non-object, which would
/// turn `"inputs": [1,2]` into a silently empty start. Both halves are measured
/// here, because a guard that only refuses cannot be told from one that refuses
/// everything: the three non-objects 400, and absent / `null` / `{}` all start.
#[tokio::test]
async fn inputs_that_are_not_an_object_are_refused_rather_than_coerced() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let flow = common::seed_flow(&db, ws, user, &runnable_source("inputs"), "draft").await;
    let path = format!("/api/ws/{ws}/workflows/flows/{flow}/executions");

    let app = common::app(&db).await;
    for bad in [
        serde_json::json!({ "inputs": [1, 2] }),
        serde_json::json!({ "inputs": "world" }),
        serde_json::json!({ "inputs": 7 }),
    ] {
        let res = common::post(&app, &path, user, bad.clone()).await;
        assert_eq!(res.status(), 400, "{bad} must be refused, not coerced");
    }
    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "a refused start creates nothing"
    );

    // The accepting half: three spellings of "no inputs", all of which start
    // and all of which store `{}`.
    assert_eq!(common::post_no_body(&app, &path, user).await.status(), 201);
    assert_eq!(
        common::post(&app, &path, user, serde_json::json!({}))
            .await
            .status(),
        201
    );
    assert_eq!(
        common::post(&app, &path, user, serde_json::json!({ "inputs": null }))
            .await
            .status(),
        201
    );
    let empties: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions WHERE workspace_id = $1 AND inputs = '{}'::jsonb",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("count empty-input executions");
    assert_eq!(empties, 3);
}

/// **Both doors a flow can be switched off by**, and a trashed flow is a 404
/// rather than a 409.
///
/// The `state` column is the one the management UI writes and the YAML's own
/// `disabled: true` is the other; reading only the column is what let a
/// disabled flow fire through the webhook (that route's fix round 1, Critical
/// 1). The enabled control is what says the two 409s are about `disabled` and
/// not about the route.
#[tokio::test]
async fn a_disabled_or_trashed_flow_cannot_be_started() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;

    let by_column =
        common::seed_flow(&db, ws, user, &runnable_source("off-column"), "disabled").await;
    let by_yaml = common::seed_flow(
        &db,
        ws,
        user,
        &format!("{}disabled: true\n", runnable_source("off-yaml")),
        "draft",
    )
    .await;
    let trashed = common::seed_flow(&db, ws, user, &runnable_source("trashed"), "draft").await;
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&db.pool)
        .await
        .expect("trash the flow");
    let live = common::seed_flow(&db, ws, user, &runnable_source("on"), "draft").await;

    let app = common::app(&db).await;
    for (flow, want, why) in [
        (by_column, 409, "the state column"),
        (by_yaml, 409, "the YAML's own disabled:"),
        // 404 and NOT 409: a trashed flow must be indistinguishable from one
        // that never existed, the way `subflow::resolve` and the webhook both
        // treat `deleted_at IS NOT NULL`.
        (trashed, 404, "deleted_at"),
    ] {
        assert_eq!(
            common::post_no_body(
                &app,
                &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
                user
            )
            .await
            .status(),
            want,
            "{why} must refuse with {want}"
        );
    }
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{live}/executions"),
            user
        )
        .await
        .status(),
        201,
        "the control: an enabled, untrashed flow starts"
    );
    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "only the control started anything"
    );
}

/// **Resuming something that is not `PAUSED` is a 409, not a silent success.**
///
/// `worker::resume` on a running or finished execution commits a settle that
/// changes nothing and answers `Ok(())`, so without this check the route would
/// answer 200 to a caller who pressed a button that did nothing. The control is
/// a really-paused execution, started through the route above, which answers
/// 200 — so the 409s are about the state and not about the route.
#[tokio::test]
async fn resume_on_a_non_paused_execution_is_a_conflict_not_a_silent_success() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let running = insert_execution_with(
        &db,
        ws,
        None,
        "RUNNING",
        &runnable_source("resume-running"),
        serde_json::json!({}),
    )
    .await;
    let done = insert_execution_with(
        &db,
        ws,
        None,
        "SUCCESS",
        &runnable_source("resume-done"),
        serde_json::json!({}),
    )
    .await;
    let pause_flow =
        common::seed_flow(&db, ws, user, &unbounded_pause_source("resume-ok"), "draft").await;

    let app = common::app(&db).await;
    for (id, state) in [(running, "RUNNING"), (done, "SUCCESS")] {
        let res = common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}/resume"),
            user,
        )
        .await;
        assert_eq!(res.status(), 409, "a {state} execution cannot be resumed");
        assert_eq!(
            state_of(&db, id).await,
            state,
            "and the refusal left the row alone"
        );
    }

    // The control: a genuinely `PAUSED` execution answers 200 on the same
    // route. Started through the start route so the pause is the executor's own
    // and not a state string this test wrote.
    let started = common::json(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{pause_flow}/executions"),
            user,
        )
        .await,
    )
    .await;
    let paused: Uuid = started["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        state_of(&db, paused).await,
        "PAUSED",
        "a flow that is one Pause reaches PAUSED in its own first settle"
    );
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{paused}/resume"),
            user
        )
        .await
        .status(),
        200
    );

    // The start above and this resume, in that order, and nothing from the two
    // 409s — a refused write must leave no trace in the trail either.
    let audit = common::audit_mutations(&app);
    let actions: Vec<&str> = audit.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        vec!["workflow_execution:start", "workflow_execution:resume"],
        "the two refusals emitted nothing; the start and the resume emitted one each"
    );
    assert_eq!(audit[1].1.as_deref(), Some(paused.to_string().as_str()));
    assert_eq!(audit[1].2, serde_json::json!({ "from_state": "PAUSED" }));
}

/// **A replay runs the replayed execution's FROZEN source and inputs, not the
/// draft's** — and it is a new execution, leaving the original untouched.
///
/// The draft is edited to a different source before the replay, which is what
/// makes this discriminating: a handler that re-read the flow would produce the
/// draft's source and pass every other assertion here.
#[tokio::test]
async fn a_replay_runs_the_frozen_source_and_inputs_and_not_the_drafts() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let frozen = runnable_source("replay-frozen");
    let flow = common::seed_flow(&db, ws, user, &frozen, "draft").await;
    let done = insert_execution_with(
        &db,
        ws,
        Some(flow),
        "SUCCESS",
        &frozen,
        serde_json::json!({ "who": "world", "n": 3 }),
    )
    .await;
    // The draft moves on. Same namespace and `flow_id` so the row still
    // matches, a different body so the two sources cannot be confused.
    let edited =
        format!("{frozen}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: later\n");
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(&edited)
        .execute(&db.pool)
        .await
        .expect("edit the draft");

    let app = common::app(&db).await;
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{done}/replay"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let dto = common::json(res).await;
    let replacement: Uuid = dto["id"].as_str().unwrap().parse().unwrap();

    assert_ne!(replacement, done, "a replay is a NEW execution");
    assert_eq!(
        dto["source"].as_str(),
        Some(frozen.as_str()),
        "the frozen source, not the edited draft"
    );
    let (inputs, parent): (Value, Option<Uuid>) =
        sqlx::query_as("SELECT inputs, parent_execution_id FROM workflow.executions WHERE id = $1")
            .bind(replacement)
            .fetch_one(&db.pool)
            .await
            .expect("read the replacement");
    // The mapping, not the key set -- see
    // [`a_start_runs_the_drafts_source_with_the_inputs_the_caller_sent`].
    assert_eq!(inputs, serde_json::json!({ "who": "world", "n": 3 }));
    // **No parent, asserted rather than assumed.** Migration 0138's
    // `workflow_executions_loop_run_idx` is UNIQUE over
    // `(parent_execution_id, parent_run_ix, loop_index)` NULLS NOT DISTINCT, so
    // the one `(X, NULL, NULL)` slot belongs to `replay::spawn_due`'s
    // `CREATE_NEW_EXECUTION` replacement. A manual replay that took it would
    // make a later retry-replay of the same execution fail a constraint. The
    // manual replay's link is its own column, `replayed_from_execution_id`
    // (W9a, migration 0166; `tests/storage_index.rs`
    // `the_manual_replay_route_records_its_source`).
    assert_eq!(parent, None);
    assert_eq!(
        state_of(&db, done).await,
        "SUCCESS",
        "the replayed execution itself is untouched"
    );

    // The audit trail carries the link too. Both ids, in the right roles: a
    // payload that swapped them would name the replacement as the thing
    // replayed.
    let audit = common::audit_mutations(&app);
    assert_eq!(audit.len(), 1, "one replay, one audit record");
    assert_eq!(audit[0].0, "workflow_execution:replay");
    assert_eq!(
        audit[0].1.as_deref(),
        Some(replacement.to_string().as_str()),
        "the record is filed against the NEW execution"
    );
    assert_eq!(
        audit[0].2,
        serde_json::json!({ "replayed_from": done, "replacement": replacement })
    );
}

/// **A replay of an execution that has not finished is a 409.**
///
/// Two copies of one flow in flight at once, a replay and its original, is
/// what the narrow door refuses. The control is the same route on a terminal
/// execution.
#[tokio::test]
async fn a_replay_of_an_unfinished_execution_is_a_conflict() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let source = runnable_source("replay-state");
    // One open flow behind every fixture: `replay` gates on the flow before it
    // gates on the state (fix round 1), so a flow-less execution would be
    // refused 404 and this test would measure the orphan arm rather than
    // terminality.
    let flow = common::seed_flow(&db, ws, user, &source, "draft").await;
    let mut unfinished = Vec::new();
    for state in ["CREATED", "RUNNING", "PAUSED", "RETRYING"] {
        unfinished.push((
            state,
            insert_execution_with(&db, ws, Some(flow), state, &source, serde_json::json!({})).await,
        ));
    }
    let app = common::app(&db).await;
    for (state, id) in &unfinished {
        assert_eq!(
            common::post_no_body(
                &app,
                &format!("/api/ws/{ws}/workflows/executions/{id}/replay"),
                user
            )
            .await
            .status(),
            409,
            "a {state} execution has not finished"
        );
    }
    assert_eq!(
        common::execution_count(&db, ws).await,
        unfinished.len() as i64,
        "nothing was created by the refusals"
    );

    // Every terminal state `RunState::is_terminal` names, so the predicate is
    // pinned at both ends rather than by one lucky SUCCESS.
    for state in [
        "SUCCESS", "FAILED", "WARNING", "KILLED", "SKIPPED", "RETRIED",
    ] {
        let id =
            insert_execution_with(&db, ws, Some(flow), state, &source, serde_json::json!({})).await;
        assert_eq!(
            common::post_no_body(
                &app,
                &format!("/api/ws/{ws}/workflows/executions/{id}/replay"),
                user
            )
            .await
            .status(),
            201,
            "a {state} execution is finished and can be replayed"
        );
    }
}

/// One `Pause` with `props`, nothing after it: a resumed execution ends there.
fn pause_source(id: &str, props: &str) -> String {
    format!(
        "id: {id}\nnamespace: a.write\ntasks:\n  \
         - id: pause\n    type: io.kestra.plugin.core.flow.Pause\n{props}"
    )
}

/// Start `flow` through the route; the id of an execution that is `PAUSED`.
async fn start_paused(
    app: &common::App,
    db: &Db,
    ws: Uuid,
    user: egret_core::UserId,
    flow: Uuid,
) -> Uuid {
    let res = common::post_no_body(
        app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let id: Uuid = common::json(res).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        state_of(db, id).await,
        "PAUSED",
        "the fixture must reach the pause"
    );
    id
}

async fn resume_status(app: &common::App, ws: Uuid, user: egret_core::UserId, id: Uuid) -> u16 {
    common::post_no_body(
        app,
        &format!("/api/ws/{ws}/workflows/executions/{id}/resume"),
        user,
    )
    .await
    .status()
    .as_u16()
}

/// The execution's history and `ended_at`, and each `pause*` run's history,
/// `ended_at` and attempts, read from the tables.
async fn pause_rows(
    db: &Db,
    id: Uuid,
) -> (Vec<String>, bool, Vec<(String, Vec<String>, bool, Value)>) {
    let (history, ended): (Value, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT history, ended_at FROM workflow.executions WHERE id = $1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let runs: Vec<(String, Value, Option<DateTime<Utc>>, Value)> = sqlx::query_as(
        "SELECT task_id, history, ended_at, attempts FROM workflow.task_runs \
         WHERE execution_id = $1 AND task_id LIKE 'pause%' ORDER BY run_ix",
    )
    .bind(id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let names = |v: Value| -> Vec<String> { serde_json::from_value(v).unwrap() };
    (
        names(history),
        ended.is_some(),
        runs.into_iter()
            .map(|(t, h, e, a)| (t, names(h), e.is_some(), a))
            .collect(),
    )
}

/// **A resume releases a paused `Pause` AT ONCE, whatever its bound, and it
/// ends `SUCCESS`** (W7b Task 3 fix round 1; Kestra's `ExecutionService.resume`
/// and `Pause.java`: a pause resumed by hand "will always succeed").
///
/// Three bounds, none of them due: an unbounded pause (`Resume::Never`, which
/// nothing ended before), an hour's `pauseDuration:`, and an hour's
/// `timeout:` (whose natural end is `FAILED`). Each goes, in one call, to the
/// natural end's shape: the run `CREATED RUNNING PAUSED SUCCESS`, its attempt
/// closed with an end, `RESTARTED` on the execution, and every `ended_at` set.
/// A second resume is a 409.
///
/// `#[sqlx::test]`: the worker's waker selects due runs globally.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resume_releases_every_bound_of_pause_at_once(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    for (label, props) in [
        ("unbounded", ""),
        ("pauseDuration", "    pauseDuration: PT1H\n"),
        ("timeout", "    timeout: PT1H\n"),
    ] {
        let flow = common::seed_flow(&db, ws, user, &pause_source(label, props), "draft").await;
        let id = start_paused(&app, &db, ws, user, flow).await;

        let res = common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}/resume"),
            user,
        )
        .await;
        assert_eq!(res.status(), 200, "{label}");
        let dto = common::json(res).await;
        assert_eq!(
            dto["state"], "SUCCESS",
            "{label}: the answer is the state after the resume: {dto}"
        );

        let (history, ended, runs) = pause_rows(&db, id).await;
        assert_eq!(
            history,
            [
                "CREATED",
                "RUNNING",
                "PAUSED",
                "RESTARTED",
                "RUNNING",
                "SUCCESS"
            ],
            "{label}"
        );
        assert!(ended, "{label}: the execution's ended_at");
        assert_eq!(runs.len(), 1, "{label}");
        let (_, run_history, run_ended, attempts) = &runs[0];
        assert_eq!(
            run_history,
            &["CREATED", "RUNNING", "PAUSED", "SUCCESS"],
            "{label}"
        );
        assert!(run_ended, "{label}: the run's ended_at");
        assert_eq!(
            attempts[0]["history"],
            serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
            "{label}"
        );
        assert!(
            attempts[0]["ended_at"].is_i64(),
            "{label}: the attempt's end: {attempts}"
        );

        assert_eq!(
            resume_status(&app, ws, user, id).await,
            409,
            "{label}: nothing is paused any more"
        );
    }
}

/// **Several parallel pauses are released by one resume**, and the flow runs on.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resume_releases_every_parallel_pause(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let source = "id: fan\nnamespace: a.write\ntasks:\n  \
         - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
         - id: pause_a\n        type: io.kestra.plugin.core.flow.Pause\n      \
         - id: pause_b\n        type: io.kestra.plugin.core.flow.Pause\n        pauseDuration: PT1H\n      \
         - id: pause_c\n        type: io.kestra.plugin.core.flow.Pause\n        timeout: PT1H\n";
    let flow = common::seed_flow(&db, ws, user, source, "draft").await;
    let id = start_paused(&app, &db, ws, user, flow).await;
    let (_, _, before) = pause_rows(&db, id).await;
    assert_eq!(before.len(), 3, "all three branches reached their pause");
    assert!(
        before
            .iter()
            .all(|(_, h, _, _)| h.last().map(String::as_str) == Some("PAUSED")),
        "{before:?}"
    );

    assert_eq!(resume_status(&app, ws, user, id).await, 200);
    assert_eq!(state_of(&db, id).await, "SUCCESS");
    let (_, _, runs) = pause_rows(&db, id).await;
    for (task, history, ended, _) in &runs {
        assert_eq!(
            history,
            &["CREATED", "RUNNING", "PAUSED", "SUCCESS"],
            "{task}"
        );
        assert!(ended, "{task}");
    }
}

/// **The race with the waker is decided by the execution's lock, and the
/// run transitions once.** A `timeout:` pause whose deadline has passed (the
/// execution is backdated) is due for the waker, whose natural end is `FAILED`.
///
/// 1. The waker first: the run ends `FAILED`, and the resume that follows is a
///    409 that writes nothing.
/// 2. The resume first: the run ends `SUCCESS` (a manual resume always
///    succeeds), and the waker that follows finds nothing due.
/// 3. Both at once, several times: exactly one terminal on the run, and the
///    resume's answer agrees with which one won.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resume_and_the_waker_transition_a_pause_once(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &pause_source("race", "    timeout: PT1S\n"),
        "draft",
    )
    .await;
    let backdate = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE workflow.executions SET created_at = created_at - interval '1 hour' WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let run_history = |id: Uuid| {
        let db = db.clone();
        async move { pause_rows(&db, id).await.2.remove(0).1 }
    };

    // 1. The waker first.
    let id = start_paused(&app, &db, ws, user, flow).await;
    backdate(id).await;
    assert_eq!(
        egret_workflow::worker::wake(&pool, 10).await.unwrap(),
        vec![id]
    );
    assert_eq!(
        run_history(id).await,
        ["CREATED", "RUNNING", "PAUSED", "FAILED"]
    );
    assert_eq!(resume_status(&app, ws, user, id).await, 409);
    // The worker's own answer, under the lock, which is what decides a real
    // race: the route's pre-check never lets this case reach it.
    assert_eq!(
        egret_workflow::worker::resume_by_hand(&pool, id)
            .await
            .unwrap(),
        egret_workflow::worker::Resumed::NotPaused
    );
    assert_eq!(
        run_history(id).await,
        ["CREATED", "RUNNING", "PAUSED", "FAILED"],
        "the 409 wrote nothing"
    );

    // 2. The resume first.
    let id = start_paused(&app, &db, ws, user, flow).await;
    backdate(id).await;
    assert_eq!(resume_status(&app, ws, user, id).await, 200);
    assert_eq!(
        run_history(id).await,
        ["CREATED", "RUNNING", "PAUSED", "SUCCESS"]
    );
    assert!(
        egret_workflow::worker::wake(&pool, 10)
            .await
            .unwrap()
            .is_empty(),
        "nothing is due"
    );
    assert_eq!(
        run_history(id).await,
        ["CREATED", "RUNNING", "PAUSED", "SUCCESS"]
    );

    // 3. Both at once.
    for round in 0..6 {
        let id = start_paused(&app, &db, ws, user, flow).await;
        backdate(id).await;
        // Alternate which future is polled first, so either may take the lock.
        let (by_hand, woken) = if round % 2 == 0 {
            tokio::join!(
                egret_workflow::worker::resume_by_hand(&pool, id),
                egret_workflow::worker::wake(&pool, 10),
            )
        } else {
            let (w, r) = tokio::join!(
                egret_workflow::worker::wake(&pool, 10),
                egret_workflow::worker::resume_by_hand(&pool, id),
            );
            (r, w)
        };
        let history = run_history(id).await;
        assert_eq!(
            history.len(),
            4,
            "round {round}: one terminal, written once: {history:?}"
        );
        match by_hand.unwrap() {
            egret_workflow::worker::Resumed::Released(1) => {
                assert_eq!(history[3], "SUCCESS", "round {round}")
            }
            egret_workflow::worker::Resumed::NotPaused => {
                assert_eq!(history[3], "FAILED", "round {round}");
                assert_eq!(woken.unwrap(), vec![id], "round {round}");
            }
            other => panic!("round {round}: {other:?}"),
        }
    }
}

/// `task_id, state, resume_with, not_before_ms, updated_at` of one run.
type RunRow = (String, String, Option<String>, Option<i64>, DateTime<Utc>);

/// **An execution paused only by a waiting `LoopUntil` is a 409, and nothing
/// changes** (W7b Task 3 fix round 2, review N-1). `pause_mark` puts the
/// execution in `PAUSED` while a `LoopUntil` waits for its next iteration, but
/// a resume releases `Pause` tasks only (Kestra too), so there is nothing for
/// it to do -- and a 200 that changed nothing would be a silent success.
///
/// The control is the same route on a real `Pause` (above), which answers 200.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resume_of_an_execution_paused_only_by_a_loop_until_is_a_conflict(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let source = "id: until\nnamespace: a.write\ntasks:\n  - id: loop\n    \
                  type: io.kestra.plugin.core.flow.LoopUntil\n    \
                  condition: \"{{ false }}\"\n    \
                  checkFrequency:\n      interval: PT1H\n      maxIterations: 3\n    \
                  tasks:\n      \
                  - id: work\n        type: io.kestra.plugin.core.log.Log\n        \
                  message: hi\n";
    let flow = common::seed_flow(&db, ws, user, source, "draft").await;
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let id: Uuid = common::json(res).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // The body's one task runs; the condition is false; the loop waits an hour.
    drain_with(&pool).await;
    assert_eq!(
        state_of(&db, id).await,
        "PAUSED",
        "the fixture must reach the loop's wait"
    );
    let rows = |pool: sqlx::PgPool| async move {
        let ex: (Value, DateTime<Utc>) =
            sqlx::query_as("SELECT history, updated_at FROM workflow.executions WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let runs: Vec<RunRow> = sqlx::query_as(
            "SELECT task_id, state, resume_with, not_before_ms, updated_at FROM workflow.task_runs \
             WHERE execution_id = $1 ORDER BY run_ix",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();
        (ex, runs)
    };
    let before = rows(pool.clone()).await;
    assert!(
        before
            .1
            .iter()
            .any(|r| r.0 == "loop" && r.1 == "PAUSED" && r.2.as_deref() == Some("ITERATE")),
        "the loop is what waits: {before:?}"
    );

    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{id}/resume"),
        user,
    )
    .await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Nothing to resume: no Pause task is waiting"),
        "{body}"
    );
    assert_eq!(rows(pool.clone()).await, before, "nothing was written");
    assert_eq!(
        egret_workflow::worker::resume_by_hand(&pool, id)
            .await
            .unwrap(),
        egret_workflow::worker::Resumed::NoPauseWaiting
    );
    assert!(
        common::audit_mutations(&app)
            .iter()
            .all(|(a, _, _)| a != "workflow_execution:resume"),
        "a refused resume leaves no audit record"
    );
}

// ── Fix round 1 ─────────────────────────────────────────────────────────────

/// **Replay closes the same THREE doors start does.** Fix round 1's shipped
/// over-acceptance, made into a test.
///
/// `replay_execution` used to check terminality and nothing else — measured as
/// **0** occurrences of `deleted_at|disabled` inside that handler against **7**
/// inside `start_flow_execution` — so on one and the same flow, `start`
/// answered **404** and `replay` answered **201**, linking a fresh execution to
/// a flow that was both trashed and disabled.
///
/// **Both routes are driven over the SAME fixtures**, in one loop, rather than
/// two tests that could drift the way the two handlers did. A door added to one
/// route and not the other fails here.
#[tokio::test]
async fn replay_closes_the_same_three_doors_start_does() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;

    // Each case is one flow plus one SUCCESS execution of it: the same subject,
    // reachable by both routes.
    async fn case(
        db: &Db,
        ws: Uuid,
        user: egret_core::UserId,
        key: &str,
        source: &str,
        state: &str,
        trash: bool,
    ) -> (Uuid, Uuid) {
        let flow = common::seed_flow(db, ws, user, source, state).await;
        if trash {
            sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
                .bind(flow)
                .execute(&db.pool)
                .await
                .expect("trash the flow");
        }
        let execution =
            insert_execution_with(db, ws, Some(flow), "SUCCESS", source, serde_json::json!({}))
                .await;
        let _ = key;
        (flow, execution)
    }

    let (trashed_flow, trashed_ex) = case(
        &db,
        ws,
        user,
        "trashed",
        &runnable_source("doors-trashed"),
        "draft",
        true,
    )
    .await;
    let (column_flow, column_ex) = case(
        &db,
        ws,
        user,
        "column",
        &runnable_source("doors-column"),
        "disabled",
        false,
    )
    .await;
    let yaml_source = format!("{}disabled: true\n", runnable_source("doors-yaml"));
    let (yaml_flow, yaml_ex) = case(&db, ws, user, "yaml", &yaml_source, "draft", false).await;
    let (live_flow, live_ex) = case(
        &db,
        ws,
        user,
        "live",
        &runnable_source("doors-live"),
        "draft",
        false,
    )
    .await;

    let seeded = common::execution_count(&db, ws).await;
    assert_eq!(seeded, 4, "one seeded execution per case");

    let app = common::app(&db).await;
    for (what, flow, execution, want) in [
        ("in the trash", trashed_flow, trashed_ex, 404),
        ("disabled by the state column", column_flow, column_ex, 409),
        ("disabled by its own YAML", yaml_flow, yaml_ex, 409),
    ] {
        assert_eq!(
            common::post_no_body(
                &app,
                &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
                user
            )
            .await
            .status(),
            want,
            "start must refuse a flow {what}"
        );
        assert_eq!(
            common::post_no_body(
                &app,
                &format!("/api/ws/{ws}/workflows/executions/{execution}/replay"),
                user
            )
            .await
            .status(),
            want,
            "replay must refuse a flow {what} with the SAME status start does"
        );
    }
    assert_eq!(
        common::execution_count(&db, ws).await,
        seeded,
        "six refusals, and not one of them created an execution"
    );
    assert!(
        common::audit_mutations(&app).is_empty(),
        "nor did any of them write an audit record"
    );

    // **An execution whose flow was hard-deleted names no flow, so neither door
    // can be asked** — refused rather than run. Unreachable on the product's
    // path (`ExecutionRow::flow_id`'s doc measures that this crate issues no
    // `DELETE FROM`), which is exactly why it needs a test rather than a
    // comment.
    let orphan = insert_execution_with(
        &db,
        ws,
        None,
        "SUCCESS",
        &runnable_source("doors-orphan"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{orphan}/replay"),
            user
        )
        .await
        .status(),
        404,
        "a replay whose flow no longer exists at all is refused, not run"
    );

    // **The control**, and it is what says the six refusals above are about the
    // doors rather than about the routes: the identical pair of calls on a flow
    // with every door open both answer 201.
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{live_flow}/executions"),
            user
        )
        .await
        .status(),
        201
    );
    assert_eq!(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{live_ex}/replay"),
            user
        )
        .await
        .status(),
        201
    );
}

/// **A start runs the DRAFT even when the flow has a published version.**
///
/// Until this test the three-site draft ruling (`worker::subflow::resolve`,
/// `webhook_routes`, and this crate's start route) was pinned by **prose
/// only**: no fixture anywhere had a published version at all, so nothing
/// discriminated a draft read from a `flow_versions` read — every test would
/// have passed unchanged against a handler that resolved through the version
/// archive. Paragraphs go stale; this does not.
///
/// **The version is written with `FlowRepo::insert_version` + `set_live` rather
/// than through `POST .../publish`**, and that is measured rather than lazy:
/// `spec::registry` knows exactly three task types — `core.http`, `core.slack`
/// and `core.log` — so `io.kestra.plugin.core.log.Log`, which is what every
/// executable fixture in this file uses, cannot be published at all. The two
/// repository calls leave precisely the row publish leaves: a `flow_versions`
/// snapshot, `live_version_id` pointing at it, and `state = 'live'`.
///
/// The fixture's own control is asserted before the start, because a `set_live`
/// that silently did nothing would make this test pass while proving nothing.
#[tokio::test]
async fn a_start_runs_the_draft_even_when_a_published_version_exists() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;

    let published = runnable_source("versioned");
    let flow = common::seed_flow(&db, ws, user, &published, "draft").await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let version = repo
        .insert_version(
            flow,
            user,
            &serde_json::json!({ "source": published, "plan": {}, "refs": {} }),
            "the published one",
        )
        .await
        .expect("insert version");
    repo.set_live(flow, version.id).await.expect("set live");

    // The draft moves on, and the two sources differ in a way an assertion can
    // see. Same namespace and `flow_id`, so the row still matches.
    let edited = format!(
        "{published}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: edited\n"
    );
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(&edited)
        .execute(&db.pool)
        .await
        .expect("edit the draft");

    // **The fixture's control**: the flow really is published, the snapshot
    // really holds the OLD source, and the draft really holds the new one. All
    // three before the route is called, so a start that answered with the draft
    // for the wrong reason cannot pass.
    let (live, snapshot_source, draft_source): (Option<Uuid>, String, String) = sqlx::query_as(
        "SELECT f.live_version_id, v.snapshot->>'source', f.source \
           FROM workflow.flows f JOIN workflow.flow_versions v ON v.id = f.live_version_id \
          WHERE f.id = $1",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .expect("read the published flow");
    assert_eq!(live, Some(version.id), "the flow has a live version");
    assert_eq!(
        snapshot_source, published,
        "the snapshot holds the OLD source"
    );
    assert_eq!(draft_source, edited, "the draft holds the NEW source");
    assert_ne!(published, edited);

    let app = common::app(&db).await;
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
    )
    .await;
    // `state = 'live'` is not `'disabled'`, so the gate lets a published flow
    // through -- asserted here rather than assumed, since `set_live` is what
    // moves the column off `'draft'`.
    assert_eq!(res.status(), 201, "a published flow is startable");
    let dto = common::json(res).await;
    assert_eq!(
        dto["source"].as_str(),
        Some(edited.as_str()),
        "the start ran the DRAFT, not the live version's snapshot"
    );
    let stored: String = sqlx::query_scalar(
        "SELECT source FROM workflow.executions WHERE workspace_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("read the started execution's frozen source");
    assert_eq!(stored, edited, "and the row agrees with the response");
}

// ── W7b Task 1: the `root_only` and `parent` filters ────────────────────────

/// Insert a child execution of either kind, in the shape migration 0138's
/// CHECKs admit: a loop iteration (`parent_execution_id` + run + index +
/// value) or a subflow/flow-trigger child (`trigger_execution_id` + run).
async fn insert_child(
    db: &Db,
    ws: Uuid,
    parent: Option<(Uuid, i32, i32)>,
    trigger: Option<(Uuid, i32)>,
    created_at: DateTime<Utc>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, created_at, \
            parent_execution_id, parent_run_ix, loop_index, loop_value, \
            trigger_execution_id, trigger_run_ix) \
         VALUES ($1, 'a.tree', 'child', $2, 'SUCCESS', '[\"CREATED\",\"SUCCESS\"]', $3, \
                 $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(ws)
    .bind(SEEDED_SOURCE)
    .bind(created_at)
    .bind(parent.map(|p| p.0))
    .bind(parent.map(|p| p.1))
    .bind(parent.map(|p| p.2))
    .bind(parent.map(|p| p.2.to_string()))
    .bind(trigger.map(|t| t.0))
    .bind(trigger.map(|t| t.1))
    .fetch_one(&db.pool)
    .await
    .expect("insert child execution")
}

/// Insert the replacement a `CREATE_NEW_EXECUTION` retry starts, in the shape
/// `worker::replay` writes: `parent_execution_id` alone (no run, no loop
/// index), `attempt_number` 2 — the shape 0138/0139's CHECKs admit for it.
async fn insert_retry(db: &Db, ws: Uuid, parent: Uuid, created_at: DateTime<Utc>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, created_at, \
            parent_execution_id, attempt_number) \
         VALUES ($1, 'a.tree', 'root', $2, 'SUCCESS', '[\"CREATED\",\"SUCCESS\"]', $3, $4, 2) \
         RETURNING id",
    )
    .bind(ws)
    .bind(SEEDED_SOURCE)
    .bind(created_at)
    .bind(parent)
    .fetch_one(&db.pool)
    .await
    .expect("insert retry execution")
}

fn ids(page: &Value) -> std::collections::BTreeSet<String> {
    page["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// **`root_only=true` hides loop iterations and subflow/flow-trigger
/// children and keeps `CREATE_NEW_EXECUTION` retries (the user's ruling,
/// 2026-10-11); `parent=<id>` lists exactly that execution's children, of
/// every link kind; a foreign parent is an empty page; both at once, or a
/// malformed value, is a 400.**
///
/// The fixture holds every kind of row each filter must tell apart: two
/// roots, a loop iteration, a subflow child and a retry of the first, a
/// grandchild (a child of the iteration, which `parent=<root>` must NOT reach
/// — the filter is one level), and a root with a child in ANOTHER workspace.
#[tokio::test]
async fn the_root_only_and_parent_filters_split_the_tree() {
    let Some(db) = common::db().await else { return };
    // Arrange
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let root = insert_execution(&db, ws, None, "a.tree", "root", "SUCCESS", at(1)).await;
    let lone = insert_execution(&db, ws, None, "a.tree", "lone", "SUCCESS", at(2)).await;
    let iteration = insert_child(&db, ws, Some((root, 0, 0)), None, at(3)).await;
    let subflow = insert_child(&db, ws, None, Some((root, 1)), at(4)).await;
    let grandchild = insert_child(&db, ws, None, Some((iteration, 0)), at(5)).await;
    let retry = insert_retry(&db, ws, root, at(6)).await;
    let foreign = insert_execution(&db, other_ws, None, "a.tree", "far", "SUCCESS", at(1)).await;
    insert_child(&db, other_ws, None, Some((foreign, 0)), at(2)).await;
    let app = common::app(&db).await;
    let set = |v: &[Uuid]| -> std::collections::BTreeSet<String> {
        v.iter().map(Uuid::to_string).collect()
    };

    // Act + Assert: no filter is every row of the workspace (the control).
    let all = list(&app, ws, user, "namespace=a.tree").await;
    assert_eq!(
        ids(&all),
        set(&[root, lone, iteration, subflow, grandchild, retry]),
        "{all}"
    );

    // The retry stays beside the roots; the iteration, the subflow child and
    // the grandchild (a subflow child of the iteration) do not.
    let roots = list(&app, ws, user, "namespace=a.tree&root_only=true").await;
    assert_eq!(ids(&roots), set(&[root, lone, retry]), "{roots}");
    assert_eq!(
        roots["total"], 3,
        "the count carries the same predicate: {roots}"
    );
    let everything = list(&app, ws, user, "namespace=a.tree&root_only=false").await;
    assert_eq!(
        everything["total"], 6,
        "root_only=false is no filter: {everything}"
    );

    let kids = list(&app, ws, user, &format!("parent={root}")).await;
    assert_eq!(
        ids(&kids),
        set(&[iteration, subflow, retry]),
        "every link kind, one level only: {kids}"
    );
    assert_eq!(kids["total"], 3, "{kids}");
    let grand = list(&app, ws, user, &format!("parent={iteration}")).await;
    assert_eq!(ids(&grand), set(&[grandchild]), "{grand}");
    let none = list(&app, ws, user, &format!("parent={lone}")).await;
    assert_eq!(none["total"], 0, "{none}");

    // Another workspace's parent: an empty page, never an error.
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/executions?parent={foreign}"),
        user,
    )
    .await;
    assert_eq!(res.status(), 200);
    let page = common::json(res).await;
    assert_eq!(
        (page["total"].clone(), ids(&page).len()),
        (Value::from(0), 0),
        "{page}"
    );

    // 400s: both filters; a malformed uuid; a root_only that is not a bool.
    for q in [
        format!("root_only=true&parent={root}"),
        "parent=not-a-uuid".to_string(),
        "root_only=maybe".to_string(),
    ] {
        let res = common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions?{q}"),
            user,
        )
        .await;
        assert_eq!(res.status(), 400, "{q}");
    }
    // `root_only=false` beside `parent` is the default spelled out, not a
    // contradiction.
    let ok = list(&app, ws, user, &format!("root_only=false&parent={root}")).await;
    assert_eq!(ok["total"], 3, "{ok}");
}

/// **`namespace_prefix` is a namespace and its children, dot-anchored, in
/// this workspace only; beside `namespace` it is a 400.**
///
/// The fixture holds what a prefix match could get wrong: the namespace
/// itself, a child and a grandchild, a sibling that shares the prefix without
/// the dot (`a.bc` against `a.b`), a parent (`a`), an underscore that `LIKE`
/// would read as a wildcard (`a_b`), and the same namespace in ANOTHER
/// workspace.
#[tokio::test]
async fn namespace_prefix_matches_a_namespace_and_its_children_only() {
    let Some(db) = common::db().await else { return };
    // Arrange
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let exact = insert_execution(&db, ws, None, "a.b", "f", "SUCCESS", at(1)).await;
    let child = insert_execution(&db, ws, None, "a.b.c", "f", "SUCCESS", at(2)).await;
    let grandchild = insert_execution(&db, ws, None, "a.b.c.d", "f", "SUCCESS", at(3)).await;
    insert_execution(&db, ws, None, "a.bc", "f", "SUCCESS", at(4)).await;
    insert_execution(&db, ws, None, "a", "f", "SUCCESS", at(5)).await;
    insert_execution(&db, ws, None, "a_b", "f", "SUCCESS", at(6)).await;
    insert_execution(&db, other_ws, None, "a.b", "f", "SUCCESS", at(7)).await;
    insert_execution(&db, other_ws, None, "a.b.c", "f", "SUCCESS", at(8)).await;
    let app = common::app(&db).await;
    let set = |v: &[Uuid]| -> std::collections::BTreeSet<String> {
        v.iter().map(Uuid::to_string).collect()
    };

    // Act + Assert: the subtree, and nothing beside or above it.
    let tree = list(&app, ws, user, "namespace_prefix=a.b").await;
    assert_eq!(ids(&tree), set(&[exact, child, grandchild]), "{tree}");
    assert_eq!(tree["total"], 3, "the count carries the predicate: {tree}");

    // A leaf prefix is the exact namespace.
    let leaf = list(&app, ws, user, "namespace_prefix=a.b.c.d").await;
    assert_eq!(ids(&leaf), set(&[grandchild]), "{leaf}");

    // The exact filter, for contrast: the namespace alone.
    let only = list(&app, ws, user, "namespace=a.b").await;
    assert_eq!(ids(&only), set(&[exact]), "{only}");

    // `_` is a character: `a_b` matches itself, not `a.b`.
    let underscore = list(&app, ws, user, "namespace_prefix=a_b").await;
    assert_eq!(underscore["total"], 1, "{underscore}");

    // Both at once is a 400, whichever values they carry.
    for q in [
        "namespace=a.b&namespace_prefix=a.b",
        "namespace=a&namespace_prefix=a.b",
    ] {
        let res = common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions?{q}"),
            user,
        )
        .await;
        assert_eq!(res.status(), 400, "{q}");
    }
}

// ── W7b Task 1: real times through the DTO, and the replay link ─────────────

/// **A replay's DTO names what it replays, and its times are its own.**
///
/// Driven through the real worker and the real routes: start, finish, replay,
/// finish. The replay's `started_at` is after the original's `ended_at`, and
/// the original's times are untouched by it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_carries_its_source_and_fresh_times(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let flow = common::seed_flow(&db, ws, user, &runnable_source("timed"), "draft").await;
    let app = common::app(&db).await;
    let started = common::json(
        common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
            user,
        )
        .await,
    )
    .await;
    let original: Uuid = started["id"].as_str().unwrap().parse().unwrap();
    drain_with(&pool).await;
    let detail = |id: Uuid| {
        let app = &app;
        async move {
            common::json(
                common::get(
                    app,
                    &format!("/api/ws/{ws}/workflows/executions/{id}"),
                    user,
                )
                .await,
            )
            .await
        }
    };
    let before = detail(original).await;
    assert_eq!(before["state"], "SUCCESS", "{before}");
    assert_eq!(
        before["replayed_from_execution_id"],
        Value::Null,
        "{before}"
    );
    let ts = |v: &Value, k: &str| -> DateTime<Utc> {
        v[k].as_str()
            .unwrap_or_else(|| panic!("{k} is set: {v}"))
            .parse()
            .unwrap()
    };
    let (o_start, o_end) = (ts(&before, "started_at"), ts(&before, "ended_at"));
    for run in before["runs"].as_array().unwrap() {
        assert!(
            ts(run, "started_at") >= o_start && ts(run, "ended_at") <= o_end,
            "{run}"
        );
        assert!(run["attempts"][0]["ended_at"].is_i64(), "{run}");
    }

    // Act
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{original}/replay"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let replay: Uuid = common::json(res).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    drain_with(&pool).await;

    // Assert
    let after = detail(replay).await;
    assert_eq!(after["state"], "SUCCESS", "{after}");
    assert_eq!(
        after["replayed_from_execution_id"],
        original.to_string(),
        "{after}"
    );
    let (r_start, r_end) = (ts(&after, "started_at"), ts(&after, "ended_at"));
    assert!(
        r_start >= o_end && r_start <= r_end,
        "fresh: {o_end} then {after}"
    );
    let row = list(&app, ws, user, &format!("flow_id={flow}")).await;
    let listed = row["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == replay.to_string())
        .expect("the replay is listed")
        .clone();
    assert_eq!(listed["replayed_from_execution_id"], original.to_string());
    assert_eq!(listed["started_at"], after["started_at"]);
    let again = detail(original).await;
    assert_eq!(
        (again["started_at"].clone(), again["ended_at"].clone()),
        (before["started_at"].clone(), before["ended_at"].clone()),
        "the original is untouched by its replay"
    );
}

async fn drain_with(pool: &sqlx::PgPool) {
    for _ in 0..32 {
        if egret_workflow::worker::tick(pool, 8, &AlwaysSucceeds)
            .await
            .expect("tick")
            == 0
        {
            return;
        }
    }
    panic!("the worker never stopped claiming");
}

// ── `can_manage` on the execution (W7b Task 3, spec §5.2) ───────────────────

/// `GET …/executions/{id}` carries `can_manage`, the PDP's `workflow:manage`
/// answer, so the detail page can hide Resume and Replay from a reader. A
/// reader gets `false`; the control, identical but for the grant, `true`.
#[tokio::test]
async fn get_execution_says_whether_the_caller_may_manage_it() {
    let Some(db) = common::db().await else { return };
    for (perms, expected) in [
        (&["workflow:read"][..], false),
        (&["workflow:read", "workflow:manage"][..], true),
    ] {
        let (ws, user) = common::seed_workspace(&db).await;
        common::grant(&db, ws, user, perms).await;
        let id = insert_execution(&db, ws, None, "a.cm", "f", "SUCCESS", at(1)).await;
        let app = common::app(&db).await;
        let res = common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{id}"),
            user,
        )
        .await;
        assert_eq!(res.status(), 200, "{perms:?}");
        let body = common::json(res).await;
        assert_eq!(body["can_manage"], expected, "{perms:?}: {body}");
        // An API start has no trigger record: the wire says `null`, which is
        // what the schema now declares (`Option<Object>`).
        assert!(body["execution_trigger"].is_null(), "{body}");
    }
}

/// A PDP that fails while deciding `workflow:manage` is an error (500), not a
/// "no": the page must not hide the actions on an outage. The control is the
/// same request through an app whose PDP fails on an action nobody asks.
#[tokio::test]
async fn get_execution_answers_an_error_when_the_can_manage_decision_fails() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let id = insert_execution(&db, ws, None, "a.cmpdp", "f", "SUCCESS", at(1)).await;
    let url = format!("/api/ws/{ws}/workflows/executions/{id}");

    let ok = common::app_with_failing_pdp_action(&db, "no-such-action").await;
    assert_eq!(common::get(&ok, &url, user).await.status(), 200);

    let app = common::app_with_failing_pdp_action(&db, "workflow:manage").await;
    assert_eq!(common::get(&app, &url, user).await.status(), 500);
}

/// Start, resume and replay answer `can_manage: true`: the caller just passed
/// the `workflow:manage` check those routes make.
#[tokio::test]
async fn start_resume_and_replay_answer_can_manage_true() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let pause_flow =
        common::seed_flow(&db, ws, user, &unbounded_pause_source("cm-pause"), "draft").await;
    let source = runnable_source("cm-replay");
    let flow = common::seed_flow(&db, ws, user, &source, "draft").await;
    let done = insert_execution_with(
        &db,
        ws,
        Some(flow),
        "SUCCESS",
        &source,
        serde_json::json!({}),
    )
    .await;
    let app = common::app(&db).await;

    let started = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{pause_flow}/executions"),
        user,
    )
    .await;
    assert_eq!(started.status(), 201);
    let started = common::json(started).await;
    assert_eq!(started["can_manage"], true, "start: {started}");

    let paused = started["id"].as_str().unwrap();
    let resumed = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{paused}/resume"),
        user,
    )
    .await;
    assert_eq!(resumed.status(), 200);
    let resumed = common::json(resumed).await;
    assert_eq!(resumed["can_manage"], true, "resume: {resumed}");

    let replayed = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{done}/replay"),
        user,
    )
    .await;
    assert_eq!(replayed.status(), 201);
    let replayed = common::json(replayed).await;
    assert_eq!(replayed["can_manage"], true, "replay: {replayed}");
}

/// **The 2026-10-11 ruling, on the API start.** A `Pause` declaring
/// `behavior`, `onPause`, `tasks` or `errors` is a validation error, and a
/// saved draft carrying one is refused at start with a 400 that names the
/// property -- before the ruling it was a warning, the start answered 201, and
/// the execution stalled in `RUNNING` for ever (`NotALeaf`, reaped, reclaimed).
/// The refusal is `build_plan`'s `validate_for_run` gate inside
/// `worker::start_execution`, the one function every start path goes through.
///
/// Each of the four properties is driven, and the control is the same draft
/// with a plain bounded `Pause`, which starts: without it the first half
/// passes under a route that refuses every `Pause`.
#[tokio::test]
async fn a_start_refuses_a_draft_whose_pause_egret_does_not_run() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    let pause = |id: &str, extra: &str| {
        format!(
            "id: {id}\nnamespace: a.write\ntasks:\n  - id: gate\n    \
             type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n{extra}"
        )
    };

    for (property, extra) in [
        ("behavior", "    behavior: RESUME\n"),
        (
            "onPause",
            "    onPause:\n      id: hi\n      type: io.kestra.plugin.core.log.Log\n      message: hi\n",
        ),
        (
            "tasks",
            "    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n",
        ),
        (
            "errors",
            "    errors:\n      - id: oops\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n",
        ),
    ] {
        let flow = common::seed_flow(
            &db,
            ws,
            user,
            &pause(&format!("pause-{}", property.to_lowercase()), extra),
            "draft",
        )
        .await;
        let res = common::post_no_body(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
            user,
        )
        .await;
        assert_eq!(res.status(), 400, "{property}");
        let body = common::body_text(res).await;
        assert!(
            body.contains("unsupported_pause_property")
                && body.contains(&format!("tasks.gate.{property}")),
            "{property}: the refusal names the finding and the property: {body}"
        );
        assert_eq!(
            common::execution_count(&db, ws).await,
            0,
            "{property}: a refused start leaves no execution behind"
        );
    }

    // The control: a plain bounded Pause starts.
    let ok = common::seed_flow(&db, ws, user, &pause("pause-plain", ""), "draft").await;
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{ok}/executions"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}
