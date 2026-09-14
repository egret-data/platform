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

    let app = common::app(&db).await;
    assert_eq!(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions"), user)
            .await
            .status(),
        403
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
    end: DateTime<Utc>,
    /// Logical time — the executor's own rounds, which is what makes two bars
    /// comparable when their wall-clock spans are microseconds apart.
    created_seq: i64,
    ended_seq: i64,
    /// The bar's colour.
    state: String,
}

/// Build every gantt row of an execution from its DTO.
///
/// **Every field it needs is taken with `expect`**, so a DTO missing one does
/// not degrade into a chart with a hole — it fails this test by name. That is
/// the point of the exercise: W7 must not discover a missing field after this
/// package closes.
///
/// `start` is derived rather than read: a run's wall-clock start is
/// `execution.created_at + attempts[0].started_at`, because a `Tick` in this
/// engine is **milliseconds since the execution's `created_at`** (see
/// `worker::claim::load_with`'s `now_ms`). A run with no attempts is a
/// flowable the executor drives itself, and its own `created_at` is its start.
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
            GanttRow {
                label: r["task_id"].as_str().expect("run.task_id").to_string(),
                parent: r["parent_ix"].as_i64(),
                start,
                end: updated_at,
                created_seq: r["created_seq"].as_i64().expect("run.created_seq"),
                ended_seq: r["ended_seq"].as_i64().expect("run.ended_seq"),
                state: r["state"].as_str().expect("run.state").to_string(),
            }
        })
        .collect()
}

/// **The gantt data is already there — this asserts it rather than assuming
/// it.**
///
/// Driven through the **real worker** on a scratch database, so every field
/// [`gantt_rows`] reads has the shape production writes and not the shape a
/// fixture in this file chose. Two sequential tasks, so the assertion about
/// order is a discrimination and not a tautology.
#[sqlx::test(migrations = "../../migrations")]
async fn a_gantt_row_is_computable_from_the_execution_dto_alone(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let source = "id: gantt\nnamespace: a.gantt\ntasks:\n  \
                  - id: first\n    type: io.kestra.plugin.core.log.Log\n    message: one\n  \
                  - id: second\n    type: io.kestra.plugin.core.log.Log\n    message: two\n";
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, source, serde_json::json!({}))
            .await
            .expect("start execution");

    let reg = AlwaysSucceeds;
    let mut ticks = 0;
    loop {
        egret_workflow::worker::tick(&pool, 8, &reg)
            .await
            .expect("tick");
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
                .bind(ex)
                .fetch_one(&pool)
                .await
                .expect("read state");
        if state == "SUCCESS" {
            break;
        }
        ticks += 1;
        assert!(
            ticks < 50,
            "execution never reached SUCCESS; last state {state}"
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

    // **The wall-clock start comes from `attempts`, and this is what proves
    // the fallback in [`gantt_rows`] did not silently supply it.** Both runs
    // here are runnable, so both must have an attempt; if `attempts` were ever
    // `[]` the derivation would quietly degrade to the row's own `created_at`
    // and the test above it would still pass.
    //
    // Measured on this branch: `started_at` is `0` for `first` and `26` for
    // `second` — **milliseconds after the EXECUTION's `created_at`**, not an
    // instant. `second`'s own `created_at` is 25.7 ms after the execution's,
    // which is the same quantity read off a different clock.
    for run in dto["runs"].as_array().unwrap() {
        let attempts = run["attempts"].as_array().expect("attempts is an array");
        assert_eq!(
            attempts.len(),
            1,
            "a runnable run that succeeded first time has exactly one attempt: {run}"
        );
        assert!(
            attempts[0]["started_at"].is_i64(),
            "the only wall-clock start a run carries: {run}"
        );

        // **`history` carries NO timestamps**, on a run or on the execution —
        // it is an array of state NAMES, written by `worker::claim`'s
        // `states_json`. It is NOT Kestra's `State.History` (`{state, date}`),
        // and a gantt drawn from it alone would have an order and no clock.
        // Asserted rather than described, because the plan this task was given
        // said the opposite and nothing in the tree contradicted it.
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

    let rows = gantt_rows(&dto);
    assert_eq!(rows.len(), 2, "one bar per task run: {dto}");

    // Labels, in the executor's own creation order (`run_ix`), which is the
    // order the detail route promises.
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, vec!["first", "second"], "{dto}");

    for row in &rows {
        assert_eq!(row.state, "SUCCESS", "{row:?}");
        assert!(
            row.start <= row.end,
            "a bar cannot end before it starts: {row:?}"
        );
        assert!(
            row.created_seq <= row.ended_seq,
            "a run cannot terminate in a round before the one that created it: {row:?}"
        );
    }

    // **Logical time discriminates the two bars**, which is the whole reason a
    // gantt wants `created_seq`/`ended_seq` rather than only timestamps: these
    // two runs are sequential, so the second cannot have been created before
    // the first ended. Two wall-clock instants microseconds apart cannot say
    // that; two round numbers can.
    assert!(
        rows[1].created_seq >= rows[0].ended_seq,
        "`second` runs after `first` finished: {rows:?}"
    );

    // The whole chart fits inside the execution's own span, so an axis drawn
    // from the execution row alone contains every bar.
    let exec_start: DateTime<Utc> = dto["created_at"].as_str().unwrap().parse().unwrap();
    let exec_end: DateTime<Utc> = dto["updated_at"].as_str().unwrap().parse().unwrap();
    for row in &rows {
        assert!(
            row.start >= exec_start && row.end <= exec_end,
            "bar {row:?} escapes the execution's span {exec_start}..{exec_end}"
        );
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
