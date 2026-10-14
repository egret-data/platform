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
    // 3,000 executions of `busy`, spread over five webhook trigger ids, one
    // second apart, so a probe by `flow_id` alone reads 3,000 rows and sorts.
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            execution_trigger, created_at) \
         SELECT $1, $2, 'w7e', 'busy', 's', 'SUCCESS', '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]', \
                jsonb_build_object('id', 'hook' || (i % 5), \
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
    assert!(
        !control.contains("workflow_executions_last_by_trigger"),
        "{control}"
    );
}
