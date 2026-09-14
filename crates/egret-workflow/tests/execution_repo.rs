//! `ExecutionRepo` — the read layer over `workflow.executions` /
//! `workflow.task_runs` (W10 Task 3). Requires `DATABASE_URL`.
//!
//! **The first test in this file is the one that came before the query it
//! guards.** Migration 0128 once shipped a plain FK with no workspace
//! predicate, and the Cedar check that followed was told the *caller's*
//! workspace, never the object's. So
//! [`an_execution_in_another_workspace_is_invisible_by_id_and_in_every_list`]
//! inserts an execution in workspace B and asserts workspace A cannot reach
//! it through any method on this repository — and asserts the refusal is a
//! **404, not a 403**, because a 403 confirms the row exists.
//!
//! Rows are inserted with raw SQL rather than through `worker::spawn`'s
//! `start_execution` on purpose: these tests are about what the *reads*
//! answer, so they have to be able to spell states, flows and timestamps that
//! a writer would never produce in one sitting.

use chrono::{DateTime, TimeZone, Utc};
use egret_core::Error;
use egret_metastore::Db;
use egret_workflow::repo::{ExecutionFilter, ExecutionRepo, Page};
use uuid::Uuid;

mod common;

/// The columns a test actually varies. Everything else takes the schema's own
/// default, so a test names only what it is about.
struct Exec {
    /// Supplied only where the test needs to control the id's ORDER relative
    /// to insertion order -- the `created_at` tie-break case. `None` lets the
    /// column default to `gen_random_uuid()`.
    id: Option<Uuid>,
    flow_id: Option<Uuid>,
    namespace: &'static str,
    flow_key: &'static str,
    state: &'static str,
    /// `executions.history`, as stored. Deliberately settable independently of
    /// `state` — that separation is the whole point of
    /// [`a_list_reads_the_state_column_and_never_opens_the_history_array`].
    history: serde_json::Value,
    created_at: DateTime<Utc>,
}

impl Default for Exec {
    fn default() -> Self {
        Self {
            id: None,
            flow_id: None,
            namespace: "a.b",
            flow_key: "f",
            state: "RUNNING",
            history: serde_json::json!(["CREATED", "RUNNING"]),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }
}

async fn insert_execution(db: &Db, ws: Uuid, e: &Exec) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (id, workspace_id, flow_id, namespace, flow_key, source, state, history, created_at) \
         VALUES (COALESCE($8, gen_random_uuid()), $1, $2, $3, $4, \
                 'id: f\ntasks: []\n', $5, $6, $7) \
         RETURNING id",
    )
    .bind(ws)
    .bind(e.flow_id)
    .bind(e.namespace)
    .bind(e.flow_key)
    .bind(e.state)
    .bind(&e.history)
    .bind(e.created_at)
    .bind(e.id)
    .fetch_one(&db.pool)
    .await
    .expect("insert execution")
}

/// One `task_runs` row. `run_ix` is the ordering key under test; `task_id` is
/// what the assertion reads back, so the two can disagree and a wrong ORDER BY
/// is visible.
async fn insert_task_run(db: &Db, execution: Uuid, run_ix: i32, task_id: &str) {
    sqlx::query(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         VALUES ($1, $2, $3, $4, 'io.kestra.plugin.core.log.Log', 'CREATED', \
                 '[\"CREATED\"]'::jsonb)",
    )
    .bind(execution)
    .bind(run_ix)
    .bind(run_ix)
    .bind(task_id)
    .execute(&db.pool)
    .await
    .expect("insert task run");
}

/// A flow draft, so `executions.flow_id` can point at something real and the
/// flow filter has a value the FK will accept.
async fn insert_flow(db: &Db, ws: Uuid, user: egret_core::UserId, flow_key: &str) -> Uuid {
    let repo = egret_workflow::FlowRepo::new(db.clone());
    repo.create(
        ws,
        user,
        &egret_workflow::CreateFlowInput {
            namespace: "a.b".into(),
            flow_id: flow_key.into(),
            source: "id: f\n".into(),
            plan: serde_json::json!({}),
            folder_id: None,
        },
    )
    .await
    .expect("create flow")
    .id
}

// ---------------------------------------------------------------------------
// The cross-workspace guard. Written before the queries it guards.
// ---------------------------------------------------------------------------

/// **The `chart_id` class.** Workspace B owns an execution; workspace A must
/// not learn it exists through *any* read on this repository, and the refusal
/// must be `NotFound` (404) rather than `Forbidden` (403) — a 403 is itself a
/// disclosure that the id is real.
///
/// Every assertion here is paired with a positive control from workspace B, so
/// a repository that answered nothing to everybody would fail too.
///
/// **Workspace A owns a row of its own** (fix round 1). Without it, A's three
/// negatives -- empty `rows`, `total == 0`, empty `task_runs` -- were three
/// zeros on an empty workspace, and the test could not tell *"the predicate
/// filters correctly"* from *"A happened to have nothing"*. A's row is what
/// turns each negative into a **discrimination**: A sees exactly one
/// execution, its own, and the count is 1 rather than 0 or 2.
#[tokio::test]
async fn an_execution_in_another_workspace_is_invisible_by_id_and_in_every_list() {
    let Some(db) = common::db().await else { return };
    let (ws_a, _user_a) = common::seed_workspace(&db).await;
    let (ws_b, _user_b) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let bs = insert_execution(&db, ws_b, &Exec::default()).await;
    insert_task_run(&db, bs, 0, "only").await;

    // A's own execution, so every negative below sits beside a positive in the
    // SAME workspace and cannot be satisfied by A being empty.
    let a_own = insert_execution(
        &db,
        ws_a,
        &Exec {
            flow_key: "a-owns-this",
            ..Exec::default()
        },
    )
    .await;
    insert_task_run(&db, a_own, 0, "a-own-run").await;

    // Positive control: B can see its own, by id and in its list.
    let owned = repo
        .get(ws_b, bs)
        .await
        .expect("B must see its own execution");
    assert_eq!(owned.execution.id, bs);
    assert_eq!(owned.runs.len(), 1, "B must see its own task runs");
    let b_page = repo
        .list(ws_b, &ExecutionFilter::default(), Page::default())
        .await
        .expect("B lists");
    assert!(
        b_page.rows.iter().any(|r| r.id == bs),
        "B's own list must contain B's own execution"
    );
    assert_eq!(b_page.total, 1, "B's total counts B's own execution");
    assert_eq!(
        repo.task_runs(ws_b, bs).await.expect("B task_runs").len(),
        1,
        "B must reach its own task runs directly too"
    );

    // A, asking for exactly the same id, is told nothing at all.
    let err = repo
        .get(ws_a, bs)
        .await
        .expect_err("workspace A must not read workspace B's execution");
    assert!(
        matches!(err, Error::NotFound(_)),
        "a foreign execution must be NotFound, not {err:?}"
    );
    assert_eq!(
        err.status_code(),
        axum::http::StatusCode::NOT_FOUND,
        "404, not 403 — a 403 would confirm the id exists"
    );

    // ...and it is nowhere in A's list, nor in A's count, nor in A's runs --
    // each asserted against A's OWN row, not against an empty workspace.
    let a_page = repo
        .list(ws_a, &ExecutionFilter::default(), Page::default())
        .await
        .expect("A lists");
    assert!(
        !a_page.rows.iter().any(|r| r.id == bs),
        "workspace A's list must not contain workspace B's execution: {:?}",
        a_page.rows.iter().map(|r| r.id).collect::<Vec<_>>()
    );
    assert_eq!(
        a_page.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a_own],
        "workspace A's list is exactly its own row -- neither empty (which \
         would pass for the wrong reason) nor two rows"
    );
    assert_eq!(
        a_page.total, 1,
        "workspace A's total counts A's own execution and ONLY it; 2 means \
         B's row leaked into the count, 0 means the count lost A's own"
    );
    assert!(
        repo.task_runs(ws_a, bs)
            .await
            .expect("task_runs must answer empty, not error")
            .is_empty(),
        "workspace A must not read workspace B's task runs"
    );
    assert_eq!(
        repo.task_runs(ws_a, a_own)
            .await
            .expect("A task_runs")
            .len(),
        1,
        "...while A DOES reach its own, so the emptiness above is the \
         predicate and not a broken join"
    );
    assert_eq!(
        repo.get(ws_a, a_own)
            .await
            .expect("A must see its own execution")
            .execution
            .id,
        a_own
    );

    // An id that never existed anywhere answers exactly as the foreign one
    // does — that indistinguishability is what makes the 404 honest.
    let never = Uuid::new_v4();
    let never_err = repo.get(ws_a, never).await.expect_err("unknown id");
    assert!(matches!(never_err, Error::NotFound(_)));
    assert_eq!(never_err.status_code(), err.status_code());
}

// ---------------------------------------------------------------------------
// List: filters, ordering, pagination.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_filters_by_flow_by_state_and_by_time_window() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let flow_one = insert_flow(&db, ws, user, "one").await;
    let flow_two = insert_flow(&db, ws, user, "two").await;

    let jan = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let feb = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let mar = Utc.with_ymd_and_hms(2026, 3, 10, 0, 0, 0).unwrap();

    let a = insert_execution(
        &db,
        ws,
        &Exec {
            flow_id: Some(flow_one),
            flow_key: "one",
            state: "SUCCESS",
            created_at: jan,
            ..Exec::default()
        },
    )
    .await;
    let b = insert_execution(
        &db,
        ws,
        &Exec {
            flow_id: Some(flow_one),
            flow_key: "one",
            state: "FAILED",
            created_at: feb,
            ..Exec::default()
        },
    )
    .await;
    let c = insert_execution(
        &db,
        ws,
        &Exec {
            flow_id: Some(flow_two),
            flow_key: "two",
            namespace: "x.y",
            state: "RUNNING",
            created_at: mar,
            ..Exec::default()
        },
    )
    .await;

    let ids =
        |p: &egret_workflow::repo::ExecutionPage| p.rows.iter().map(|r| r.id).collect::<Vec<_>>();

    // Newest first, with no filter at all.
    let all = repo
        .list(ws, &ExecutionFilter::default(), Page::default())
        .await
        .expect("list all");
    assert_eq!(ids(&all), vec![c, b, a], "newest first");
    assert_eq!(all.total, 3);

    // By flow.
    let by_flow = repo
        .list(
            ws,
            &ExecutionFilter {
                flow_id: Some(flow_one),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("by flow");
    assert_eq!(ids(&by_flow), vec![b, a]);
    assert_eq!(by_flow.total, 2);

    // By state — several, because a run list shows FAILED beside RUNNING.
    let by_state = repo
        .list(
            ws,
            &ExecutionFilter {
                states: Some(vec!["FAILED".into(), "RUNNING".into()]),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("by state");
    assert_eq!(ids(&by_state), vec![c, b]);
    assert_eq!(by_state.total, 2);

    // By time window: `since` inclusive, `until` exclusive, so a window whose
    // end is exactly `mar` excludes the execution created at `mar`.
    let windowed = repo
        .list(
            ws,
            &ExecutionFilter {
                since: Some(feb),
                until: Some(mar),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("by window");
    assert_eq!(
        ids(&windowed),
        vec![b],
        "since is inclusive and until is exclusive"
    );
    assert_eq!(windowed.total, 1);

    // By namespace and by flow key, the two halves of the human identity.
    let by_ns = repo
        .list(
            ws,
            &ExecutionFilter {
                namespace: Some("x.y".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("by namespace");
    assert_eq!(ids(&by_ns), vec![c]);
    let by_key = repo
        .list(
            ws,
            &ExecutionFilter {
                flow_key: Some("one".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("by flow key");
    assert_eq!(ids(&by_key), vec![b, a]);
}

#[tokio::test]
async fn a_page_is_a_window_and_the_total_is_the_whole_match() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let mut ids = Vec::new();
    for minute in 0..5u32 {
        ids.push(
            insert_execution(
                &db,
                ws,
                &Exec {
                    created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, minute, 0).unwrap(),
                    ..Exec::default()
                },
            )
            .await,
        );
    }
    ids.reverse(); // newest first, the list's own order

    let first = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 2,
                offset: 0,
            },
        )
        .await
        .expect("page 1");
    assert_eq!(
        first.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids[0..2].to_vec()
    );
    assert_eq!(
        first.total, 5,
        "the total is every matching row, not the page's length"
    );

    let second = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 2,
                offset: 2,
            },
        )
        .await
        .expect("page 2");
    assert_eq!(
        second.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids[2..4].to_vec()
    );
    assert_eq!(second.total, 5);

    let past_the_end = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 2,
                offset: 10,
            },
        )
        .await
        .expect("page past the end");
    assert!(past_the_end.rows.is_empty());
    assert_eq!(
        past_the_end.total, 5,
        "an empty page still reports the whole match, or a UI cannot draw a pager"
    );
}

/// **Read the column, do not open the jsonb.** `executions.state` *is*
/// `history.last()` — `claim::persist` computes both in one expression, one
/// statement, one transaction — so there is exactly one place to read it from.
///
/// The row below makes the two disagree, which production cannot produce and
/// no CHECK on `workflow.executions` forbids. That is deliberate: it is the
/// only way to tell a query that reads `state` from one that re-derives it
/// from `history`, and a query that re-derives it would answer `SUCCESS` here.
#[tokio::test]
async fn a_list_reads_the_state_column_and_never_opens_the_history_array() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let id = insert_execution(
        &db,
        ws,
        &Exec {
            state: "FAILED",
            history: serde_json::json!(["CREATED", "RUNNING", "SUCCESS"]),
            ..Exec::default()
        },
    )
    .await;

    let page = repo
        .list(ws, &ExecutionFilter::default(), Page::default())
        .await
        .expect("list");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(
        page.rows[0].state, "FAILED",
        "the state column is the answer; the history array is not re-read"
    );

    // ...and the state FILTER reads the same column, so a row cannot be
    // filtered by one representation and displayed as the other.
    let by_column = repo
        .list(
            ws,
            &ExecutionFilter {
                states: Some(vec!["FAILED".into()]),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("filter by the column's value");
    assert_eq!(by_column.rows.len(), 1);
    let by_array = repo
        .list(
            ws,
            &ExecutionFilter {
                states: Some(vec!["SUCCESS".into()]),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .expect("filter by the array's last element");
    assert!(
        by_array.rows.is_empty(),
        "filtering must not match the history array's last element"
    );

    assert_eq!(
        repo.get(ws, id).await.expect("get").execution.state,
        "FAILED"
    );
}

// ---------------------------------------------------------------------------
// Get-one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_one_returns_its_task_runs_ordered_by_run_ix() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let id = insert_execution(&db, ws, &Exec::default()).await;
    // Inserted out of order on purpose: a missing ORDER BY would most likely
    // hand these back in insertion order, which is not `run_ix` order.
    insert_task_run(&db, id, 2, "third").await;
    insert_task_run(&db, id, 0, "first").await;
    insert_task_run(&db, id, 1, "second").await;

    // A second execution's runs must not leak into the first's.
    let other = insert_execution(&db, ws, &Exec::default()).await;
    insert_task_run(&db, other, 0, "someone-elses").await;

    let detail = repo.get(ws, id).await.expect("get");
    assert_eq!(detail.execution.id, id);
    assert_eq!(
        detail.execution.history,
        serde_json::json!(["CREATED", "RUNNING"]),
        "history comes back whole for a detail page that draws it"
    );
    assert!(
        detail.source.contains("tasks:"),
        "the frozen source is what a detail page draws its topology from: {:?}",
        detail.source
    );
    assert_eq!(
        detail.runs.iter().map(|r| r.run_ix).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "task runs are ordered by run_ix, their creation order"
    );
    assert_eq!(
        detail
            .runs
            .iter()
            .map(|r| r.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );

    // `task_runs` on its own answers the same thing, so a poller does not have
    // to re-read the execution to refresh the run table.
    let runs = repo.task_runs(ws, id).await.expect("task_runs");
    assert_eq!(
        runs.iter().map(|r| r.run_ix).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

/// An execution with no task runs yet is a real execution, not a missing one —
/// the state every execution passes through between its INSERT and the first
/// tick that plans it.
#[tokio::test]
async fn an_execution_with_no_task_runs_is_found_with_an_empty_run_list() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let id = insert_execution(
        &db,
        ws,
        &Exec {
            state: "CREATED",
            history: serde_json::json!(["CREATED"]),
            ..Exec::default()
        },
    )
    .await;

    let detail = repo
        .get(ws, id)
        .await
        .expect("an execution with no runs exists");
    assert_eq!(detail.execution.state, "CREATED");
    assert!(detail.runs.is_empty());
}

/// **The ORDER BY, proven rather than assumed.**
///
/// [`get_one_returns_its_task_runs_ordered_by_run_ix`] above does NOT prove it:
/// with three rows Postgres plans that read as an `Index Scan using
/// task_runs_execution_id_run_ix_key`, and that index IS `(execution_id,
/// run_ix)` -- so the rows arrive in `run_ix` order whether the statement asks
/// for it or not. Measured, not supposed: deleting `ORDER BY t.run_ix` from
/// `ExecutionRepo::task_runs` leaves that test green (mutation M7).
///
/// This one makes the index stop answering. Five hundred runs on one
/// execution flips the plan to a `Seq Scan`, which returns PHYSICAL order --
/// and the rows are inserted shuffled, so physical order is not `run_ix`
/// order.
///
/// The first assertion is the instrument check, and it is the point: it reads
/// the same rows with the same predicate and NO `ORDER BY`, and requires that
/// answer to be unsorted. If a future planner sorts it anyway, this test fails
/// LOUDLY rather than silently certifying an ORDER BY it can no longer see.
#[tokio::test]
async fn task_runs_are_sorted_by_the_query_and_not_by_the_plan() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let id = insert_execution(&db, ws, &Exec::default()).await;
    // Shuffled by an md5 of the index, so the insertion order -- and therefore
    // the heap order -- is deterministic across runs but is not `run_ix` order.
    sqlx::query(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         SELECT $1, g, g, 't' || g, 'io.kestra.plugin.core.log.Log', 'CREATED', \
                '[\"CREATED\"]'::jsonb \
           FROM (SELECT generate_series(0, 499) AS g \
                  ORDER BY md5(generate_series(0, 499)::text)) s",
    )
    .bind(id)
    .execute(&db.pool)
    .await
    .expect("insert 500 shuffled task runs");
    sqlx::query("ANALYZE workflow.task_runs")
        .execute(&db.pool)
        .await
        .expect("analyze");

    // The instrument: the same join, the same predicate, no ORDER BY.
    let unordered: Vec<i32> = sqlx::query_scalar(
        "SELECT t.run_ix FROM workflow.task_runs t \
         JOIN workflow.executions e ON e.id = t.execution_id \
         WHERE t.execution_id = $1 AND e.workspace_id = $2",
    )
    .bind(id)
    .bind(ws)
    .fetch_all(&db.pool)
    .await
    .expect("unordered control");
    assert_eq!(unordered.len(), 500);
    assert!(
        !unordered.windows(2).all(|w| w[0] <= w[1]),
        "INSTRUMENT FAILURE, not a product failure: at this row count the plan \
         is supposed to stop handing back `run_ix` order for free, and it did \
         not -- so the assertion below cannot tell an ORDER BY from an index \
         and must not be read as proving one"
    );

    let runs = repo.task_runs(ws, id).await.expect("task_runs");
    assert_eq!(
        runs.iter().map(|r| r.run_ix).collect::<Vec<_>>(),
        (0..500).collect::<Vec<i32>>(),
        "the statement's ORDER BY is what sorts these, not the plan"
    );
}

/// **The clamp, tested at both ends -- because a guard nobody has seen fail is
/// a guard nobody has seen** (fix round 1).
///
/// `Page::clamped` existed with a doc comment explaining what it prevents and
/// no test at either end: replacing its body with the identity survived the
/// whole suite, twice. Worse, the hazard it names is real rather than
/// theoretical -- Postgres treats a negative `LIMIT` as an **ERROR**, not as
/// an empty page, so an unclamped `?limit=-1` becomes a 500. The first
/// assertion below is that hazard, measured directly, so the rest of the test
/// is defending something known to bite.
#[tokio::test]
async fn a_page_is_clamped_at_both_ends_rather_than_handed_to_postgres() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    // One more than MAX_PAGE_LIMIT, so the upper clamp has something to cut.
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, created_at) \
         SELECT $1, 'a.b', 'f', 'src', 'RUNNING', '[\"CREATED\",\"RUNNING\"]'::jsonb, \
                timestamptz '2026-01-01 00:00:00Z' + (g || ' seconds')::interval \
           FROM generate_series(0, 200) AS g",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("insert 201 executions");

    // THE HAZARD, measured: Postgres refuses a negative LIMIT outright. This
    // is what reaches the client as a 500 if the clamp is not there.
    let refused = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workflow.executions WHERE workspace_id = $1 LIMIT $2",
    )
    .bind(ws)
    .bind(-1_i64)
    .fetch_all(&db.pool)
    .await;
    assert!(
        refused.is_err(),
        "a negative LIMIT is supposed to be a database ERROR -- if Postgres \
         has started accepting it, the clamp's stated reason no longer holds \
         and this test is no longer defending what it says it defends"
    );

    // LOWER end: a malformed query string must produce a page, not an error.
    let low = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: -1,
                offset: 0,
            },
        )
        .await
        .expect("a negative limit must be clamped, not handed to Postgres");
    assert_eq!(low.rows.len(), 1, "clamped up to the minimum page of 1");
    assert_eq!(
        low.total, 201,
        "the clamp changes the page, never the total"
    );

    let zero = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 0,
                offset: 0,
            },
        )
        .await
        .expect("limit 0 must be clamped too");
    assert_eq!(zero.rows.len(), 1);

    // A negative OFFSET is equally an error in Postgres, and equally clamped.
    let neg_offset = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 3,
                offset: -5,
            },
        )
        .await
        .expect("a negative offset must be clamped, not handed to Postgres");
    let from_zero = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 3,
                offset: 0,
            },
        )
        .await
        .expect("offset 0");
    assert_eq!(
        neg_offset.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        from_zero.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        "a negative offset lands on the first page, not before it"
    );

    // UPPER end: a caller asking for everything gets MAX_PAGE_LIMIT, and the
    // fixture is deliberately 201 rows so "no upper clamp" is visible as 201.
    let high = repo
        .list(
            ws,
            &ExecutionFilter::default(),
            Page {
                limit: 10_000,
                offset: 0,
            },
        )
        .await
        .expect("an enormous limit must be clamped, not served");
    assert_eq!(
        high.rows.len() as i64,
        egret_workflow::repo::MAX_PAGE_LIMIT,
        "the upper clamp is what stops one request reading a whole workspace"
    );
    assert_eq!(
        high.total, 201,
        "...and 201 here would mean the clamp let the whole match through"
    );
}

/// **The `, id DESC` tie-break, which no fixture was forcing** (fix round 1).
///
/// Deleting it from `ExecutionRepo::list` survived the whole suite twice,
/// because every other test gives its rows DISTINCT `created_at` values -- so
/// pager stability, the reason the clause exists, was unguarded.
///
/// Three rows share one `created_at` and carry EXPLICIT, ascending ids,
/// inserted in ascending order. Correct output is `id DESC`, i.e. the exact
/// reverse of insertion order, so an untie-broken plan that falls back to
/// physical order is wrong deterministically rather than probabilistically.
///
/// The first assertion is the instrument check, as in
/// [`task_runs_are_sorted_by_the_query_and_not_by_the_plan`]: it runs the same
/// statement WITHOUT the tie-break and requires that answer to differ. If a
/// future plan happens to emit `id DESC` anyway, this test fails loudly
/// instead of certifying a clause it can no longer see.
#[tokio::test]
async fn a_tie_on_created_at_is_broken_by_id_so_a_pager_cannot_repeat_a_row() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let repo = ExecutionRepo::new(db.clone());

    let tie = Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap();
    let mut ids = Vec::new();
    for n in 1..=3u8 {
        let id = Uuid::from_bytes([n; 16]);
        insert_execution(
            &db,
            ws,
            &Exec {
                id: Some(id),
                created_at: tie,
                ..Exec::default()
            },
        )
        .await;
        ids.push(id);
    }
    let descending = {
        let mut d = ids.clone();
        d.reverse();
        d
    };

    // The instrument: the same predicate and the same primary sort key, with
    // no tie-break.
    let untied: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE workspace_id = $1 \
         ORDER BY created_at DESC",
    )
    .bind(ws)
    .fetch_all(&db.pool)
    .await
    .expect("untied control");
    assert_eq!(untied.len(), 3);
    assert_ne!(
        untied, descending,
        "INSTRUMENT FAILURE, not a product failure: without the tie-break the \
         plan is supposed to STOP producing id-descending order, and it did \
         not -- so the assertion below cannot tell a tie-break from a plan and \
         must not be read as proving one"
    );

    let page = repo
        .list(ws, &ExecutionFilter::default(), Page::default())
        .await
        .expect("list");
    assert_eq!(
        page.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        descending,
        "equal created_at is broken by id DESC, by the statement and not the plan"
    );

    // The property the tie-break exists for: three single-row pages partition
    // the three rows -- no row seen twice, none missed. This is what a pager
    // does, and it is what an unstable sort breaks.
    let mut seen = Vec::new();
    for offset in 0..3 {
        let p = repo
            .list(ws, &ExecutionFilter::default(), Page { limit: 1, offset })
            .await
            .expect("one-row page");
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.total, 3);
        seen.push(p.rows[0].id);
    }
    assert_eq!(
        seen, descending,
        "consecutive one-row pages must walk the same order the whole list \
         has, or a pager repeats one row and never shows another"
    );
}
