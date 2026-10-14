//! W7e Task 2: a trigger switched off at runtime -- a row in
//! `workflow.disabled_triggers` -- is honoured by the three hot statements
//! that already read the effective copy `sync` writes: the schedule claim, the
//! outbox insert and the listener select. Plus the switch row's survival of a
//! source that no longer parses, and the plan of the last-execution statement
//! (spec §3.6). Requires DATABASE_URL.
//!
//! **The DB tests run one at a time** (`SERIAL`) and each starts by emptying
//! the outbox, the executions and the listener rows, and by taking every
//! schedule off the queue: `fire_due`, `process_events`, `reconcile` and the
//! worker `tick` behind `common::drive_to` are all global. The binary owns its
//! database (`egret_test_db`: one clone per process).
//!
//! The switch row is written by SQL (`common::switch_off`) because its route
//! arrives in a later task; `reconcile` (the scheduler's start, which runs
//! `sync` on every flow) is what copies it, exactly as a save would.
//!
//! See docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md §3.1-§3.6.

mod common;

use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::flow_trigger::process_events;
use egret_workflow::scheduler::{fire_due, reconcile, PassReport};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";

fn parsed(src: String) -> String {
    egret_workflow::spec::parse(&src)
        .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"));
    src
}

/// A flow `id` with one `*/1 * * * *` schedule `s`.
fn scheduled(id: &str) -> String {
    parsed(format!(
        "id: {id}\nnamespace: w7e\ntriggers:\n  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    ))
}

/// A one-task flow with no trigger.
fn plain(id: &str) -> String {
    parsed(format!(
        "id: {id}\nnamespace: w7e\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    ))
}

/// A one-task flow whose window-free Flow trigger `on_done` hears the flow
/// `from` finish SUCCESS.
fn listening_to(id: &str, from: &str) -> String {
    parsed(format!(
        "id: {id}\nnamespace: w7e\ntriggers:\n  - id: on_done\n    type: {FLOW}\n    states: [SUCCESS]\n    when: \"{{{{ flow.id == '{from}' }}}}\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    ))
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    for sql in [
        "DELETE FROM workflow.flow_trigger_events",
        "DELETE FROM workflow.executions",
        "DELETE FROM workflow.flow_listeners",
        "UPDATE workflow.triggers SET next_evaluation_date = NULL",
    ] {
        sqlx::query(sql).execute(&db.pool).await.expect(sql);
    }
    let (ws, user) = common::seed_workspace(&db).await;
    Some(Fx {
        db,
        ws,
        user,
        _serial: serial,
    })
}

impl Fx {
    async fn flow(&self, src: &str) -> Uuid {
        common::seed_flow(&self.db, self.ws, self.user, src, "draft").await
    }

    async fn reconcile(&self) {
        let done = reconcile(&self.db.pool).await.expect("reconcile");
        assert_eq!(done.failed, 0, "no flow's sync may fail here: {done:?}");
    }

    /// Start `flow` and finish it SUCCESS through the real worker, so the
    /// outbox is written by the real `settle`.
    async fn run(&self, flow: Uuid, src: &str) -> Uuid {
        let id = egret_workflow::worker::start_execution(
            &self.db.pool,
            self.ws,
            Some(flow),
            src,
            serde_json::json!({}),
        )
        .await
        .expect("start the source");
        common::drive_to(&self.db, id, "SUCCESS").await;
        id
    }
}

async fn schedule_row(
    db: &Db,
    flow: Uuid,
) -> Option<(Option<chrono::DateTime<chrono::Utc>>, bool)> {
    sqlx::query_as(
        "SELECT next_evaluation_date, disabled FROM workflow.triggers \
         WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn executions_of(db: &Db, flow: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
        .bind(flow)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// §3.4: a stored source that no longer parses (a parser change leaves
/// exactly this; the API refuses to store one) loses its trigger rows to
/// `reconcile`, as before W7e, but keeps every switch row: no user switch is
/// lost to a broken source.
#[tokio::test]
async fn a_source_that_no_longer_parses_keeps_its_switch_rows() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&scheduled("broken")).await;
    common::switch_off(&fx.db, flow, "s").await;
    common::switch_off(&fx.db, flow, "gone_already").await;
    assert!(
        schedule_row(&fx.db, flow).await.is_some(),
        "control: a trigger row"
    );

    // The one raw write to workflow.flows in this binary: the state a parser
    // change leaves, which no FlowRepo call can produce.
    sqlx::query("UPDATE workflow.flows SET source = 'not: [valid' WHERE id = $1")
        .bind(flow)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert!(
        egret_workflow::spec::parse("not: [valid").is_err(),
        "control: the source really does not parse"
    );
    fx.reconcile().await;

    assert_eq!(
        schedule_row(&fx.db, flow).await,
        None,
        "the trigger row goes, as today"
    );
    assert_eq!(
        common::switched_off(&fx.db, flow).await,
        vec!["gone_already".to_string(), "s".into()],
        "every switch row stays, even one the last parse did not declare"
    );
}

/// §3.2: the schedule claim's `AND NOT t.disabled` keeps a switched-off, due
/// schedule from being claimed -- nothing fires and no tick is consumed. The
/// control switches it back on and the same pass fires it.
#[tokio::test]
async fn the_claim_skips_a_switched_off_schedule() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&scheduled("cron")).await;
    common::switch_off(&fx.db, flow, "s").await;
    common::make_due(&fx.db, flow, "s", 1).await;
    fx.reconcile().await;
    let (due, disabled) = schedule_row(&fx.db, flow).await.expect("a row");
    assert!(disabled, "control: reconcile copied the switch");
    let due = due.expect("control: on the queue");

    let report = fire_due(&fx.db.pool).await.expect("a pass");
    assert_eq!(report, PassReport::default(), "nothing claimed: {report:?}");
    assert_eq!(executions_of(&fx.db, flow).await, 0);
    assert_eq!(
        schedule_row(&fx.db, flow).await,
        Some((Some(due), true)),
        "no tick consumed: the date is where it was"
    );

    // Control: the same row switched back on is claimed and fired.
    common::switch_on(&fx.db, flow, "s").await;
    fx.reconcile().await;
    let report = fire_due(&fx.db.pool).await.expect("a pass");
    assert_eq!(report.fired, 1, "{report:?}");
    assert_eq!(executions_of(&fx.db, flow).await, 1);
}

/// §3.2: the outbox insert writes an event only if some listener that is not
/// disabled hears it. A workspace whose one listener is switched off gets no
/// event; a second listener left on makes one appear.
#[tokio::test]
async fn the_outbox_ignores_a_switched_off_listener() {
    let Some(fx) = fx().await else { return };
    let up_src = plain("up");
    let up = fx.flow(&up_src).await;
    let off = fx.flow(&listening_to("off", "up")).await;
    common::switch_off(&fx.db, off, "on_done").await;
    fx.reconcile().await;

    fx.run(up, &up_src).await;
    assert_eq!(
        common::events(&fx.db).await,
        Vec::new(),
        "the workspace's only listener is off, so nothing hears the event"
    );

    // Control: a listener left on hears the same kind of run.
    fx.flow(&listening_to("on", "up")).await;
    let second = fx.run(up, &up_src).await;
    assert_eq!(
        common::events(&fx.db).await,
        vec![(second, "SUCCESS".to_string())],
        "a listener that is on makes the event appear"
    );
}

/// §3.2: the listener select's `AND NOT l.disabled` -- two listeners hear the
/// same event, the switched-off one does not fire.
#[tokio::test]
async fn the_listener_select_skips_a_switched_off_listener() {
    let Some(fx) = fx().await else { return };
    let up_src = plain("up");
    let up = fx.flow(&up_src).await;
    let off = fx.flow(&listening_to("off", "up")).await;
    let on = fx.flow(&listening_to("on", "up")).await;
    common::switch_off(&fx.db, off, "on_done").await;
    fx.reconcile().await;

    let source = fx.run(up, &up_src).await;
    process_events(&fx.db.pool).await.expect("a pass");
    let fired: Vec<Uuid> = common::fired_by(&fx.db, source)
        .await
        .into_iter()
        .map(|(flow, _, _)| flow)
        .collect();
    assert_eq!(fired, vec![on], "only the listener that is on fires");
}

// ── `fire::resync` against a concurrent switch (review I1) ─────────────────

/// A flow `id` with two schedules: `x` (`0 2 * * *`, never due here) and `y`
/// (`*/1 * * * *`, the one a pass claims).
fn two_schedules(id: &str) -> String {
    parsed(format!(
        "id: {id}\nnamespace: w7e\ntriggers:\n  - id: x\n    type: {SCHEDULE}\n    cron: \"0 2 * * *\"\n  - id: y\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    ))
}

async fn disabled_of(db: &Db, flow: Uuid, trig: &str) -> bool {
    sqlx::query_scalar(
        "SELECT disabled FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn spec_key_of(db: &Db, flow: Uuid, trig: &str) -> String {
    sqlx::query_scalar(
        "SELECT spec_key FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Make `y` due and its row disagree with the source (a stale `spec_key`), so
/// the pass that claims it takes `fire_one`'s Frozen path and calls `resync`.
async fn stage_a_resync_of_y(db: &Db, flow: Uuid) {
    common::make_due(db, flow, "y", 1).await;
    sqlx::query(
        "UPDATE workflow.triggers SET spec_key = '0 3 * * *|false|' \
         WHERE flow_id = $1 AND trigger_id = 'y'",
    )
    .bind(flow)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// The switch route's first half (W7e spec §3.2 "Concurrency"; the route
/// itself is Task 5): in one open transaction, the flow row `FOR NO KEY
/// UPDATE`, then the switch row for `trig`.
async fn route_holds_the_flow(
    db: &Db,
    flow: Uuid,
    trig: &str,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM workflow.flows WHERE id = $1 FOR NO KEY UPDATE")
        .bind(flow)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workflow.disabled_triggers (flow_id, trigger_id, workspace_id) \
         SELECT id, $2, workspace_id FROM workflow.flows WHERE id = $1",
    )
    .bind(flow)
    .bind(trig)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx
}

/// The route's second half: the effective copy on the trigger's row.
async fn route_writes_the_copy(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    flow: Uuid,
    trig: &str,
) {
    sqlx::query(
        "UPDATE workflow.triggers SET disabled = true WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .execute(&mut **tx)
    .await
    .expect("the route's write of the copy");
}

/// Wait until `pass` has finished (`false`) or some backend of this database
/// is waiting on a lock (`true`).
async fn finished_or_blocked(
    db: &Db,
    pass: &tokio::task::JoinHandle<egret_core::Result<PassReport>>,
) -> bool {
    for _ in 0..200 {
        if pass.is_finished() {
            return false;
        }
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if waiting > 0 {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the pass neither finished nor blocked within 10 s");
}

/// Review I1: a pass that resyncs `y` while the switch route holds the flow
/// and switches `x` off must not write a stale `x.disabled = false` over the
/// route's `true` once the route commits. Before the fix `resync` read the
/// switch rows without the flow lock (the route's insert invisible), then
/// blocked on `x`'s row and applied its stale value after the route
/// committed: switch row off, copy on, and the claim would fire `x`.
///
/// `resync` now reads the flow `FOR SHARE SKIP LOCKED`: it finds the flow held,
/// leaves `y` frozen (nothing fires, the dates stay) and commits at once, so
/// it never waits on the route. The control: once the route has committed, the
/// next pass resyncs `y` and still leaves `x` off.
#[tokio::test]
async fn a_resync_does_not_undo_a_concurrent_switch_on_another_trigger() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&two_schedules("raced")).await;
    let own_key = spec_key_of(&fx.db, flow, "y").await;
    stage_a_resync_of_y(&fx.db, flow).await;

    let mut route = route_holds_the_flow(&fx.db, flow, "x").await;
    route_writes_the_copy(&mut route, flow, "x").await;
    let pool = fx.db.pool.clone();
    let pass = tokio::spawn(async move { fire_due(&pool).await });
    let blocked = finished_or_blocked(&fx.db, &pass).await;
    route.commit().await.unwrap();
    let report = pass.await.unwrap().expect("a pass");

    assert!(
        disabled_of(&fx.db, flow, "x").await,
        "the route's switch must survive the concurrent resync (lost update)"
    );
    assert!(!blocked, "the pass must not wait on the route's flow lock");
    assert_eq!(
        report,
        PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, flow).await, 0, "nothing fired");

    // Control: with the flow free, the next pass resyncs `y`.
    let report = fire_due(&fx.db.pool).await.expect("a pass");
    assert_eq!(
        report,
        PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    assert_eq!(spec_key_of(&fx.db, flow, "y").await, own_key, "y resynced");
    assert!(disabled_of(&fx.db, flow, "x").await, "x still off");
}

/// The same race on the claimed trigger itself: the route holds the flow and
/// wants `y`'s row, the pass holds `y`'s row and resyncs. A `resync` that
/// WAITED for the flow (plain `FOR SHARE`) would deadlock here; `SKIP LOCKED`
/// lets the pass finish frozen and the route then writes its copy.
#[tokio::test]
async fn a_resync_of_the_trigger_being_switched_does_not_deadlock() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&two_schedules("same")).await;
    stage_a_resync_of_y(&fx.db, flow).await;

    let mut route = route_holds_the_flow(&fx.db, flow, "y").await;
    let pool = fx.db.pool.clone();
    let pass = tokio::spawn(async move { fire_due(&pool).await });
    let blocked = finished_or_blocked(&fx.db, &pass).await;
    route_writes_the_copy(&mut route, flow, "y").await;
    route.commit().await.expect("the route commits");
    let report = pass.await.unwrap().expect("a pass");

    assert!(!blocked, "the pass must not wait on the route's flow lock");
    assert_eq!(
        report,
        PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    assert!(
        disabled_of(&fx.db, flow, "y").await,
        "the route's copy stands"
    );
    assert_eq!(
        common::switched_off(&fx.db, flow).await,
        vec!["y".to_string()]
    );
    assert_eq!(executions_of(&fx.db, flow).await, 0, "nothing fired");
}

// ── §3.6: the last execution per trigger ─────────────────────────────────────

/// §3.6's statement, verbatim. The read route (a later task) must use this
/// same text, or move it to a crate constant this test then reads.
const LAST_EXECUTION_SQL: &str = "\
SELECT k.flow_id, k.trigger_id, e.id, e.state, e.created_at
  FROM unnest($1::uuid[], $2::text[]) AS k(flow_id, trigger_id)
  CROSS JOIN LATERAL (
    SELECT id, state, created_at FROM workflow.executions e
     WHERE e.flow_id = k.flow_id
       AND e.execution_trigger IS NOT NULL
       AND e.execution_trigger->>'id' = k.trigger_id
     ORDER BY e.created_at DESC LIMIT 1) e";

/// `EXPLAIN` of §3.6's statement over 3,000 triggered executions of ONE flow
/// is served by `workflow_executions_last_by_trigger`, with no sequential scan
/// of `workflow.executions`. Control: the same rows without the index (dropped
/// in this test's own database) are not served by it, so the assertion is
/// about the index and not a name that appears in every plan.
///
/// Migration 0189 keeps the index because of this measurement (0183's rule:
/// an index the planner does not choose is not built).
#[sqlx::test(migrations = "../../migrations")]
async fn the_last_execution_per_trigger_is_served_by_its_index(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    common::install_served_default();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = common::seed_flow(&db, ws, user, &plain("busy"), "draft").await;
    // 3,000 executions of `busy`, spread over fifty webhook trigger ids (60
    // each), one second apart, so a probe by `flow_id` alone reads 3,000 rows
    // and sorts.
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            execution_trigger, created_at) \
         SELECT $1, $2, 'w7e', 'busy', 's', 'SUCCESS', '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]', \
                jsonb_build_object('id', 'hook' || (i % 50), \
                                   'type', 'io.kestra.plugin.core.trigger.Webhook', \
                                   'variables', '{}'::jsonb), \
                now() - make_interval(secs => i) \
           FROM generate_series(1, 3000) i",
    )
    .bind(ws)
    .bind(flow)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE workflow.executions")
        .execute(&pool)
        .await
        .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET plan_cache_mode = force_custom_plan")
        .execute(&mut *conn)
        .await
        .unwrap();
    let explain =
        format!("EXPLAIN EXECUTE last_q(ARRAY['{flow}']::uuid[], ARRAY['hook3']::text[])");
    // A page's worth of probes (the route's page is 50 by default, 200 at
    // most): every one of the fifty ids, one flow repeated.
    let ids: Vec<String> = (0..50).map(|i| format!("'hook{i}'")).collect();
    let flows: Vec<String> = (0..50).map(|_| format!("'{flow}'")).collect();
    let explain_page = format!(
        "EXPLAIN EXECUTE last_q(ARRAY[{}]::uuid[], ARRAY[{}]::text[])",
        flows.join(","),
        ids.join(",")
    );

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PREPARE last_q(uuid[], text[]) AS {LAST_EXECUTION_SQL}"
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(explain.clone()))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let plan = plan.join("\n");
    println!("{plan}");
    assert!(
        plan.contains("workflow_executions_last_by_trigger"),
        "{plan}"
    );
    assert!(!plan.contains("Seq Scan"), "{plan}");

    let page: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(explain_page.clone()))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let page = page.join("\n");
    println!("{page}");
    assert!(
        page.contains("workflow_executions_last_by_trigger"),
        "fifty probes keep the index: {page}"
    );
    assert!(!page.contains("Seq Scan"), "{page}");

    // Control: without the index the same statement is not served by it.
    for sql in [
        "DEALLOCATE last_q",
        "DROP INDEX workflow.workflow_executions_last_by_trigger",
        "ANALYZE workflow.executions",
    ] {
        sqlx::query(sql).execute(&mut *conn).await.expect(sql);
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PREPARE last_q(uuid[], text[]) AS {LAST_EXECUTION_SQL}"
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    let control: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(explain))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let control = control.join("\n");
    println!("{control}");
    let control_page: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(explain_page))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    println!("{}", control_page.join("\n"));
    assert!(
        !control.contains("workflow_executions_last_by_trigger"),
        "{control}"
    );
}
