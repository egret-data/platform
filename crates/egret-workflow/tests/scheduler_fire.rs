//! W12 Task 4: `scheduler::fire_due` fires every due `Schedule` trigger once
//! per pass, one trigger per transaction. Requires DATABASE_URL.
//!
//! **The tests in this binary run one at a time** (`SERIAL`) and each starts
//! by taking every trigger an earlier test left behind off the queue. A pass
//! is global -- it fires every due trigger in the database, not just the
//! calling test's -- so without both, a `PassReport` would count other tests'
//! triggers and the fault injected by `fire_and_advance_are_atomic` would
//! abort their passes too. The binary owns its database
//! (`egret_test_db`: one clone per process), so nothing outside it is touched.
//!
//! "Due" is judged against Postgres's `now()`, so a test makes a trigger due by
//! writing its date into the past (`common::make_due`), never by moving a Rust
//! clock.
//!
//! See docs/en/specs/2026-09-24-workflow-w12-trigger-loop-design.md §4-§6.

mod common;

use chrono::{DateTime, Duration, Utc};
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::scheduler::{fire_due, PassReport};
use egret_workflow::{FlowRepo, UpdateFlowInput};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";

/// A flow `id` with one `*/1 * * * *` schedule `s`. `prelude` goes before
/// `triggers:` (flow-level keys such as `inputs:` or `disabled:`), `extra` is
/// appended to the trigger (trigger-level keys, 4-space indented).
fn flow_yaml(id: &str, prelude: &str, extra: &str) -> String {
    let src = format!(
        "id: {id}\nnamespace: w12\n{prelude}triggers:\n  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n{extra}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    egret_workflow::spec::parse(&src)
        .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"));
    src
}

struct Fx {
    db: Db,
    repo: FlowRepo,
    ws: Uuid,
    user: UserId,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = NULL")
        .execute(&db.pool)
        .await
        .unwrap();
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = FlowRepo::new(db.clone());
    Some(Fx {
        db,
        repo,
        ws,
        user,
        _serial: serial,
    })
}

impl Fx {
    async fn flow(&self, src: &str) -> Uuid {
        common::seed_flow(&self.db, self.ws, self.user, src, "draft").await
    }

    async fn update(&self, id: Uuid, src: &str) {
        self.repo
            .update(
                self.ws,
                id,
                self.user,
                &UpdateFlowInput {
                    source: Some(src.into()),
                    plan: Some(serde_json::json!({})),
                    ..Default::default()
                },
            )
            .await
            .expect("update");
    }

    async fn pass(&self) -> PassReport {
        fire_due(&self.db.pool).await.expect("a pass")
    }
}

/// Everything about a trigger row that a pass could change.
#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Row {
    next_evaluation_date: Option<DateTime<Utc>>,
    evaluated_at: Option<DateTime<Utc>>,
    disabled: bool,
    last_error: Option<String>,
    updated_at: DateTime<Utc>,
}

async fn row(db: &Db, flow: Uuid) -> Option<Row> {
    sqlx::query_as(
        "SELECT next_evaluation_date, evaluated_at, disabled, last_error, updated_at \
         FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn due_date(db: &Db, flow: Uuid) -> DateTime<Utc> {
    row(db, flow)
        .await
        .expect("a trigger row")
        .next_evaluation_date
        .expect("on the queue")
}

async fn executions_of(db: &Db, flow: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
        .bind(flow)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// Formatted here, independently of the implementation's formatter, in the
/// shape `oracle-exec/w12-schedule-trigger.json` measured for
/// `Asia/Ho_Chi_Minh`: the zone's offset, six fractional digits.
fn vn(at: DateTime<Utc>) -> String {
    (at + Duration::hours(7))
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f+07:00")
        .to_string()
}

#[tokio::test]
async fn a_due_trigger_fires_once_and_moves_on() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_yaml("once", "", "    timezone: Asia/Ho_Chi_Minh\n"))
        .await;
    common::make_due(&fx.db, flow, "s", 1).await;
    let due = due_date(&fx.db, flow).await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        fx.pass().await,
        PassReport::default(),
        "the second pass has nothing due"
    );

    assert_eq!(common::scheduled_dates(&fx.db, flow).await, vec![due]);
    let trigger: serde_json::Value =
        sqlx::query_scalar("SELECT execution_trigger FROM workflow.executions WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        trigger,
        serde_json::json!({
            "id": "s",
            "type": SCHEDULE,
            "variables": {
                "date": vn(due),
                "next": vn(due + Duration::minutes(1)),
                "previous": vn(due - Duration::minutes(1)),
            }
        })
    );

    // W13a Task 3: every caller that is not the Flow-trigger processor starts
    // at the bottom of a chain -- no source link, depth 0, no outbox event.
    let chain: (Option<Uuid>, i32, Option<i64>) = sqlx::query_as(
        "SELECT trigger_execution_id, execution_depth, flow_trigger_event_id \
           FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(flow)
    .fetch_one(&fx.db.pool)
    .await
    .expect("read the chain columns");
    assert_eq!(
        chain,
        (None, 0, None),
        "a Schedule start is not a Flow-trigger fire"
    );

    let after = row(&fx.db, flow).await.unwrap();
    assert_eq!(after.next_evaluation_date, Some(due + Duration::minutes(1)));
    assert_eq!(after.evaluated_at, Some(due));
    assert_eq!(after.last_error, None);
}

#[tokio::test]
async fn all_recovers_every_missed_tick_one_per_pass() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("catchup", "", "")).await;
    common::make_due(&fx.db, flow, "s", 5).await;
    let first = due_date(&fx.db, flow).await;

    for i in 1..=5 {
        assert_eq!(
            fx.pass().await,
            PassReport {
                fired: 1,
                ..Default::default()
            },
            "pass {i}"
        );
        assert_eq!(executions_of(&fx.db, flow).await, i, "after pass {i}");
    }
    assert_eq!(
        fx.pass().await,
        PassReport::default(),
        "caught up after five"
    );
    let want: Vec<_> = (0..5).map(|k| first + Duration::minutes(k)).collect();
    assert_eq!(common::scheduled_dates(&fx.db, flow).await, want);
}

#[tokio::test]
async fn a_trigger_that_is_behind_does_not_starve_the_rest() {
    let Some(fx) = fx().await else { return };
    let behind = fx.flow(&flow_yaml("behind", "", "")).await;
    let on_time = fx.flow(&flow_yaml("on_time", "", "")).await;
    common::make_due(&fx.db, behind, "s", 5).await;
    common::make_due(&fx.db, on_time, "s", 1).await;
    let behind_due = due_date(&fx.db, behind).await;
    let on_time_due = due_date(&fx.db, on_time).await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 2,
            ..Default::default()
        }
    );
    assert_eq!(
        common::scheduled_dates(&fx.db, behind).await,
        vec![behind_due]
    );
    assert_eq!(
        common::scheduled_dates(&fx.db, on_time).await,
        vec![on_time_due]
    );
}

#[tokio::test]
async fn a_disabled_row_is_never_claimed_and_does_not_block() {
    let Some(fx) = fx().await else { return };
    let off = fx.flow(&flow_yaml("off", "", "")).await;
    fx.update(off, &flow_yaml("off", "", "    disabled: true\n"))
        .await;
    let on = fx.flow(&flow_yaml("on", "", "")).await;
    common::make_due(&fx.db, off, "s", 5).await;
    common::make_due(&fx.db, on, "s", 1).await;
    let before = row(&fx.db, off).await.unwrap();
    assert!(
        before.disabled,
        "the YAML's `disabled: true` reached the column"
    );

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, on).await, 1);
    assert_eq!(executions_of(&fx.db, off).await, 0);
    // `updated_at` included: a claimed-then-frozen row is re-synced, which
    // writes it, so an identical row means the row was never claimed at all.
    assert_eq!(row(&fx.db, off).await.unwrap(), before);
}

#[tokio::test]
async fn every_off_switch_leaves_the_dates_frozen() {
    let Some(fx) = fx().await else { return };
    let state_off = fx.flow(&flow_yaml("state_off", "", "")).await;
    let sql_deleted = fx.flow(&flow_yaml("sql_deleted", "", "")).await;
    let soft_deleted = fx.flow(&flow_yaml("soft_deleted", "", "")).await;
    let flow_yaml_off = fx
        .flow(&flow_yaml("flow_off", "disabled: true\n", ""))
        .await;
    let trig_yaml_off = fx
        .flow(&flow_yaml("trig_off", "", "    disabled: true\n"))
        .await;
    for f in [
        state_off,
        sql_deleted,
        soft_deleted,
        flow_yaml_off,
        trig_yaml_off,
    ] {
        common::make_due(&fx.db, f, "s", 1).await;
    }
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(state_off)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    // Bypasses `sync`, so the row stays and only the claim's `deleted_at`
    // clause stands between it and a fire.
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(sql_deleted)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    fx.repo.soft_delete(fx.ws, soft_deleted).await.unwrap();
    assert_eq!(
        row(&fx.db, soft_deleted).await,
        None,
        "soft_delete removes the row"
    );

    let survivors = [state_off, sql_deleted, flow_yaml_off, trig_yaml_off];
    let mut before = Vec::new();
    for f in survivors {
        before.push(row(&fx.db, f).await.expect("row survives"));
    }

    assert_eq!(fx.pass().await, PassReport::default());
    assert_eq!(common::execution_count(&fx.db, fx.ws).await, 0);
    for (f, b) in survivors.into_iter().zip(before) {
        assert_eq!(row(&fx.db, f).await.unwrap(), b, "flow {f}");
    }
}

#[tokio::test]
async fn yaml_disagreeing_with_the_column_does_not_fire_and_resyncs() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_yaml("disagree", "disabled: true\n", ""))
        .await;
    common::make_due(&fx.db, flow, "s", 1).await;
    sqlx::query("UPDATE workflow.triggers SET disabled = false WHERE flow_id = $1")
        .bind(flow)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let before = row(&fx.db, flow).await.unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, flow).await, 0);
    let after = row(&fx.db, flow).await.unwrap();
    assert!(after.disabled, "sync re-ran and put the YAML's answer back");
    assert_eq!(
        (after.next_evaluation_date, after.evaluated_at),
        (before.next_evaluation_date, before.evaluated_at),
        "the dates did not move"
    );
}

#[tokio::test]
async fn a_broken_trigger_is_taken_off_the_queue_and_its_sibling_still_fires() {
    let Some(fx) = fx().await else { return };
    let broken = fx.flow(&flow_yaml("broken", "", "")).await;
    let sibling = fx.flow(&flow_yaml("sibling", "", "")).await;
    // Older than the sibling, so it is claimed first.
    common::make_due(&fx.db, broken, "s", 2).await;
    common::make_due(&fx.db, sibling, "s", 1).await;
    let garbage = "id: broken\ntasks: [\n";
    assert!(egret_workflow::spec::parse(garbage).is_err());
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(broken)
        .bind(garbage)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, sibling).await, 1);
    assert_eq!(executions_of(&fx.db, broken).await, 0);
    let after = row(&fx.db, broken).await.unwrap();
    assert_eq!(after.next_evaluation_date, None, "off the queue");
    assert!(after.last_error.is_some(), "and says why");
}

#[tokio::test]
async fn templated_inputs_are_refused() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_yaml(
            "templated",
            "inputs:\n  - id: x\n    type: STRING\n",
            "    inputs:\n      x: \"{{ now() }}\"\n",
        ))
        .await;
    common::make_due(&fx.db, flow, "s", 1).await;
    let due = due_date(&fx.db, flow).await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            skipped: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, flow).await, 0);
    let after = row(&fx.db, flow).await.unwrap();
    let err = after.last_error.expect("the refusal is recorded");
    assert!(err.contains("inputs.x"), "{err}");
    assert_eq!(
        after.next_evaluation_date,
        Some(due + Duration::minutes(1)),
        "tick consumed"
    );
    assert_eq!(after.evaluated_at, Some(due));
}

/// DDL naming the test's own flow. The only interpolation is a `Uuid` the test
/// generated, never input.
async fn ddl(db: &Db, sql: String) {
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&db.pool)
        .await
        .unwrap();
}

/// A fault that RAISEs on `event` (e.g. `BEFORE UPDATE`) on `table`, for
/// `flow`'s rows only, so nothing else in the pass is touched. Named per flow
/// so parallel binaries cannot collide, and dropped first in case an earlier
/// run died before its cleanup. In `public`, not `pg_temp`: a session-local
/// function cannot back a trigger that the pool's other connections fire.
async fn install_fault(db: &Db, flow: Uuid, event: &str, table: &str) {
    let hex = flow.simple();
    drop_fault(db, flow, table).await;
    ddl(
        db,
        format!(
            "CREATE FUNCTION public.w12_fail_{hex}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN \
                  IF NEW.flow_id = '{flow}' THEN RAISE EXCEPTION 'w12 injected'; END IF; \
                  RETURN NEW; \
                END $$"
        ),
    )
    .await;
    ddl(
        db,
        format!(
            "CREATE TRIGGER w12_fail_{hex} {event} ON {table} \
             FOR EACH ROW EXECUTE FUNCTION public.w12_fail_{hex}()"
        ),
    )
    .await;
}

async fn drop_fault(db: &Db, flow: Uuid, table: &str) {
    let hex = flow.simple();
    ddl(
        db,
        format!("DROP TRIGGER IF EXISTS w12_fail_{hex} ON {table}"),
    )
    .await;
    ddl(
        db,
        format!("DROP FUNCTION IF EXISTS public.w12_fail_{hex}()"),
    )
    .await;
}

#[tokio::test]
async fn fire_and_advance_are_atomic() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("atomic", "", "")).await;
    common::make_due(&fx.db, flow, "s", 1).await;
    install_fault(&fx.db, flow, "BEFORE UPDATE", "workflow.triggers").await;
    let before = row(&fx.db, flow).await.unwrap();

    let result = fire_due(&fx.db.pool).await;
    drop_fault(&fx.db, flow, "workflow.triggers").await;

    let err = result.expect_err("the aborted transaction fails the pass");
    assert!(err.to_string().contains("w12 injected"), "{err}");
    assert_eq!(
        executions_of(&fx.db, flow).await,
        0,
        "the execution rolled back too"
    );
    assert_eq!(row(&fx.db, flow).await.unwrap(), before);
}

#[tokio::test]
async fn the_unique_index_refuses_a_second_fire() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("unique", "", "")).await;
    common::make_due(&fx.db, flow, "s", 1).await;
    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        }
    );

    let copy = |shift: i32| {
        sqlx::query(
            "INSERT INTO workflow.executions \
               (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
                execution_trigger, scheduled_date) \
             SELECT workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
                    execution_trigger, scheduled_date + make_interval(mins => $2) \
               FROM workflow.executions WHERE flow_id = $1",
        )
        .bind(flow)
        .bind(shift)
    };
    let err = copy(0)
        .execute(&fx.db.pool)
        .await
        .expect_err("a second execution for the same tick");
    let db_err = err.as_database_error().expect("a database error");
    assert_eq!(db_err.code().as_deref(), Some("23505"));
    assert_eq!(
        db_err.constraint(),
        Some("workflow_executions_one_per_schedule_tick")
    );
    // Control: the identical row one tick later is accepted, so the refusal
    // above was the index and nothing else about the copied row.
    copy(1)
        .execute(&fx.db.pool)
        .await
        .expect("a different tick");
    assert_eq!(executions_of(&fx.db, flow).await, 2);
}

/// W7f (migration 0192): a copy of `flow`'s first execution, with
/// `backfill_id` and `labels` as given. `scheduled` keeps the copied row a
/// schedule fire; `false` makes it an API-shaped row (no trigger record, no
/// `scheduled_date`), which 0156's iff CHECK requires together.
async fn copy_as(
    db: &Db,
    flow: Uuid,
    scheduled: bool,
    backfill_id: Option<Uuid>,
    labels: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
            execution_trigger, scheduled_date, backfill_id, labels) \
         SELECT workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
                CASE WHEN $2 THEN execution_trigger END, \
                CASE WHEN $2 THEN scheduled_date END, $3, $4::jsonb \
           FROM workflow.executions WHERE flow_id = $1 \
          ORDER BY created_at LIMIT 1",
    )
    .bind(flow)
    .bind(scheduled)
    .bind(backfill_id)
    .bind(labels)
    .execute(&db.pool)
    .await
    .map(|_| ())
}

fn refused_by(err: sqlx::Error, code: &str) -> Option<String> {
    let d = err.as_database_error().expect("a database error");
    assert_eq!(d.code().as_deref(), Some(code), "{d}");
    d.constraint().map(str::to_owned)
}

/// A fired tick of `flow`: one pass over a due trigger.
async fn fired(fx: &Fx, name: &str) -> Uuid {
    let flow = fx.flow(&flow_yaml(name, "", "")).await;
    common::make_due(&fx.db, flow, "s", 1).await;
    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        }
    );
    flow
}

/// D2: the per-tick index of 0156, rebuilt by 0192, covers regular fires only.
/// A backfill execution for a tick that already ran is accepted; a second
/// REGULAR execution for it is still refused.
#[tokio::test]
async fn the_regular_index_ignores_backfill_executions() {
    let Some(fx) = fx().await else { return };
    let flow = fired(&fx, "bf-regular").await;

    copy_as(&fx.db, flow, true, Some(Uuid::new_v4()), None)
        .await
        .expect("a backfill execution for a tick that already ran");
    let err = copy_as(&fx.db, flow, true, None, None)
        .await
        .expect_err("a second regular execution for the tick");
    assert_eq!(
        refused_by(err, "23505").as_deref(),
        Some("workflow_executions_one_per_schedule_tick")
    );
    assert_eq!(executions_of(&fx.db, flow).await, 2);
}

/// D2: one execution per (backfill, tick).
#[tokio::test]
async fn one_execution_per_backfill_tick() {
    let Some(fx) = fx().await else { return };
    let flow = fired(&fx, "bf-unique").await;
    let backfill = Uuid::new_v4();

    copy_as(&fx.db, flow, true, Some(backfill), None)
        .await
        .expect("the backfill's first execution for the tick");
    let err = copy_as(&fx.db, flow, true, Some(backfill), None)
        .await
        .expect_err("a second execution of one backfill for one tick");
    assert_eq!(
        refused_by(err, "23505").as_deref(),
        Some("workflow_executions_one_per_backfill_tick")
    );
    // Control: another backfill over the same tick is accepted, so the
    // refusal above was the pair and nothing else about the copied row.
    copy_as(&fx.db, flow, true, Some(Uuid::new_v4()), None)
        .await
        .expect("another backfill, same tick");
    assert_eq!(executions_of(&fx.db, flow).await, 3);
}

/// 0192's CHECKs: a backfill execution is a schedule fire, and `labels` is a
/// list or NULL.
#[tokio::test]
async fn backfill_id_needs_a_scheduled_date_and_labels_are_a_list() {
    let Some(fx) = fx().await else { return };
    let flow = fired(&fx, "bf-checks").await;

    // Control first: an API-shaped row with neither column is accepted.
    copy_as(&fx.db, flow, false, None, None)
        .await
        .expect("an API-shaped row");
    let err = copy_as(&fx.db, flow, false, Some(Uuid::new_v4()), None)
        .await
        .expect_err("a backfill id on a row that is not a schedule fire");
    assert_eq!(
        refused_by(err, "23514").as_deref(),
        Some("workflow_executions_backfill_is_a_schedule_fire")
    );
    let err = copy_as(&fx.db, flow, false, None, Some("{}"))
        .await
        .expect_err("labels as an object");
    assert_eq!(
        refused_by(err, "23514").as_deref(),
        Some("workflow_executions_labels_are_a_list")
    );
    copy_as(&fx.db, flow, false, None, Some("[]"))
        .await
        .expect("an empty list");
    copy_as(
        &fx.db,
        flow,
        false,
        None,
        Some(r#"[{"key":"a","value":"b"}]"#),
    )
    .await
    .expect("a list of labels");
    assert_eq!(executions_of(&fx.db, flow).await, 4);
}

/// 0192: a backfill's next tick stays inside its window. The row is deleted
/// once the next tick passes `window_end` (spec §3.6, §3.7).
#[tokio::test]
async fn a_backfill_tick_stays_inside_its_window() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("bf-window", "", "")).await;
    let row = |past_end: i32| {
        sqlx::query(
            "INSERT INTO workflow.trigger_backfills \
               (flow_id, trigger_id, workspace_id, window_start, window_end, current_tick, \
                previous_next_evaluation_date, total_ticks) \
             VALUES ($1, 's', $2, '2026-10-01T00:00:00Z', '2026-10-01T01:00:00Z', \
                     '2026-10-01T01:00:00Z'::timestamptz + make_interval(mins => $3), \
                     now(), 61)",
        )
        .bind(flow)
        .bind(fx.ws)
        .bind(past_end)
    };
    let err = row(1)
        .execute(&fx.db.pool)
        .await
        .expect_err("a tick after window_end");
    assert_eq!(
        refused_by(err, "23514").as_deref(),
        Some("workflow_trigger_backfills_tick_within_window")
    );
    // Control: the last tick of the window itself is accepted.
    row(0)
        .execute(&fx.db.pool)
        .await
        .expect("current_tick = window_end");
}

/// The first execution of `flow`: its source and trigger record, to start
/// more through `start_execution_in`.
async fn first_execution(db: &Db, flow: Uuid) -> (String, serde_json::Value, DateTime<Utc>) {
    sqlx::query_as(
        "SELECT source, execution_trigger, scheduled_date FROM workflow.executions \
          WHERE flow_id = $1 ORDER BY created_at LIMIT 1",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn start_in(
    fx: &Fx,
    flow: Uuid,
    source: &str,
    trigger: &serde_json::Value,
    scheduled_date: DateTime<Utc>,
    labels: Option<serde_json::Value>,
    backfill_id: Option<Uuid>,
) -> egret_core::Result<Uuid> {
    let mut tx = fx.db.pool.begin().await.unwrap();
    let started = egret_workflow::worker::start_execution_in(
        &mut tx,
        egret_workflow::worker::NewExecution {
            workspace_id: fx.ws,
            flow_id: Some(flow),
            source,
            inputs: serde_json::json!({}),
            trigger: Some(trigger.clone()),
            scheduled_date: Some(scheduled_date),
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels,
            backfill_id,
            variables: None,
            started_by: None,
        },
    )
    .await;
    if started.is_ok() {
        tx.commit().await.unwrap();
    }
    started
}

/// `NewExecution`'s two new fields reach the row; `None` stores NULL.
#[tokio::test]
async fn start_execution_in_writes_labels_and_backfill_id() {
    let Some(fx) = fx().await else { return };
    let flow = fired(&fx, "bf-insert").await;
    let (source, trigger, tick) = first_execution(&fx.db, flow).await;
    let backfill = Uuid::new_v4();
    let labels = serde_json::json!([{"key": "a", "value": "b"}]);

    let with = start_in(
        &fx,
        flow,
        &source,
        &trigger,
        tick,
        Some(labels.clone()),
        Some(backfill),
    )
    .await
    .expect("a backfill execution");
    // One tick later, so the regular index is not what this exercises.
    let without = start_in(
        &fx,
        flow,
        &source,
        &trigger,
        tick + Duration::minutes(1),
        None,
        None,
    )
    .await
    .expect("a regular execution");

    let read = |id: Uuid| {
        sqlx::query_as::<
            _,
            (
                Option<serde_json::Value>,
                Option<Uuid>,
                Option<DateTime<Utc>>,
            ),
        >(
            "SELECT labels, backfill_id, scheduled_date FROM workflow.executions WHERE id = $1"
        )
        .bind(id)
        .fetch_one(&fx.db.pool)
    };
    assert_eq!(
        read(with).await.unwrap(),
        (Some(labels), Some(backfill), Some(tick))
    );
    assert_eq!(
        read(without).await.unwrap(),
        (None, None, Some(tick + Duration::minutes(1)))
    );
}

/// What `scheduler::start::classify` reads: the second start of one
/// (backfill, tick) fails with the backfill index's name on a `sqlx`
/// database error, kept as the source of `Error::Internal`.
#[tokio::test]
async fn a_second_start_of_one_backfill_tick_names_the_backfill_index() {
    let Some(fx) = fx().await else { return };
    let flow = fired(&fx, "bf-classify").await;
    let (source, trigger, tick) = first_execution(&fx.db, flow).await;
    let backfill = Uuid::new_v4();

    start_in(&fx, flow, &source, &trigger, tick, None, Some(backfill))
        .await
        .expect("the backfill's first execution for the tick");
    let err = start_in(&fx, flow, &source, &trigger, tick, None, Some(backfill))
        .await
        .expect_err("a second one");
    let egret_core::Error::Internal(source) = &err else {
        panic!("not Internal: {err:?}");
    };
    let constraint = source
        .downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .and_then(|d| d.constraint());
    assert_eq!(
        constraint,
        Some("workflow_executions_one_per_backfill_tick"),
        "{err}"
    );
}

#[tokio::test]
async fn a_flow_that_does_not_plan_is_taken_off_the_queue_and_its_sibling_still_fires() {
    let Some(fx) = fx().await else { return };
    // Parses and resolves, but `build_plan`'s validation refuses it: the
    // required input `x` is supplied by nothing (`schedule_missing_inputs`).
    let src = flow_yaml("unplannable", "inputs:\n  - id: x\n    type: STRING\n", "");
    let plan_err = egret_workflow::executor::build_plan(&src).expect_err("must not plan");
    let broken = fx.flow(&src).await;
    let sibling = fx.flow(&flow_yaml("sibling", "", "")).await;
    // Older than the sibling, so it is claimed first.
    common::make_due(&fx.db, broken, "s", 2).await;
    common::make_due(&fx.db, sibling, "s", 1).await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, broken).await, 0);
    assert_eq!(executions_of(&fx.db, sibling).await, 1);
    let after = row(&fx.db, broken).await.unwrap();
    assert_eq!(after.next_evaluation_date, None, "off the queue");
    let err = after.last_error.expect("and says why");
    assert!(err.contains(&plan_err.message), "{err}");
}

#[tokio::test]
async fn a_transient_failure_while_starting_keeps_the_tick_for_the_next_pass() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("transient", "", "")).await;
    let sibling = fx.flow(&flow_yaml("sibling", "", "")).await;
    common::make_due(&fx.db, flow, "s", 2).await;
    common::make_due(&fx.db, sibling, "s", 1).await;
    let due = due_date(&fx.db, flow).await;
    let before = row(&fx.db, flow).await.unwrap();
    install_fault(&fx.db, flow, "BEFORE INSERT", "workflow.executions").await;

    let first = fire_due(&fx.db.pool).await;
    let during = row(&fx.db, flow).await.unwrap();
    let executions_during = executions_of(&fx.db, flow).await;
    drop_fault(&fx.db, flow, "workflow.executions").await;

    assert_eq!(
        first.expect("the pass survives a failed start"),
        PassReport {
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_during, 0);
    assert_eq!(
        executions_of(&fx.db, sibling).await,
        1,
        "the rest of the pass ran"
    );
    assert_eq!(
        (during.next_evaluation_date, during.evaluated_at),
        (before.next_evaluation_date, before.evaluated_at),
        "the dates did not move, and the trigger is still on the queue"
    );
    let err = during.last_error.expect("the failure is recorded");
    assert!(err.contains("w12 injected"), "{err}");

    // The fault is gone: the very same tick fires, and the note is cleared.
    let second = fx.pass().await;
    assert_eq!(second.fired, 1, "{second:?}");
    assert_eq!(common::scheduled_dates(&fx.db, flow).await, vec![due]);
    assert_eq!(row(&fx.db, flow).await.unwrap().last_error, None);
}

#[tokio::test]
async fn a_tick_that_already_fired_is_consumed_not_taken_off_the_queue() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("again", "", "")).await;
    common::make_due(&fx.db, flow, "s", 1).await;
    let before = row(&fx.db, flow).await.unwrap();
    let due = before.next_evaluation_date.unwrap();
    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        }
    );
    // Put the row back as it was before the fire, as a lost advance would.
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = $2, evaluated_at = $3 \
         WHERE flow_id = $1",
    )
    .bind(flow)
    .bind(before.next_evaluation_date)
    .bind(before.evaluated_at)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            skipped: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        common::scheduled_dates(&fx.db, flow).await,
        vec![due],
        "no second execution"
    );
    let after = row(&fx.db, flow).await.unwrap();
    assert_eq!(
        after.next_evaluation_date,
        Some(due + Duration::minutes(1)),
        "consumed: advanced past the tick, still on the queue"
    );
    assert_eq!(after.evaluated_at, Some(due));
    let err = after.last_error.expect("the duplicate is noted");
    assert!(err.contains("already has an execution"), "{err}");
}

async fn spec_key_of(db: &Db, flow: Uuid, trig: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT spec_key FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn db_now(db: &Db) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// Final review I2. Under `FOR UPDATE OF t`, a claim that waited on a save
/// re-reads the NEWEST `t` but keeps its snapshot's `f`, so the pass can hold
/// an old cron beside a row `sync` wrote for a new one. Firing then writes the
/// old cron's dates under the new `spec_key`, for good -- a same-key save
/// keeps dates. That race cannot be staged deterministically; its signature
/// can: a row whose `spec_key` disagrees with the cron in the source the claim
/// read. The pass must not fire it, must not advance it, and must re-sync.
#[tokio::test]
async fn a_spec_key_that_disagrees_with_the_source_does_not_fire_and_resyncs() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("stale", "", "")).await;
    let own_key = spec_key_of(&fx.db, flow, "s").await.expect("a row");
    assert_eq!(own_key, "*/1 * * * *|false|", "control: the key sync wrote");
    // Three ticks behind: an advance by `tick` would land in the PAST
    // (due + 1 min), a re-initialisation by `sync` in the future, so the two
    // cannot be confused below.
    common::make_due(&fx.db, flow, "s", 3).await;
    sqlx::query("UPDATE workflow.triggers SET spec_key = '0 2 * * *|false|' WHERE flow_id = $1")
        .bind(flow)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, flow).await, 0, "nothing fired");
    assert_eq!(
        spec_key_of(&fx.db, flow, "s").await.as_deref(),
        Some(own_key.as_str()),
        "sync re-ran and wrote the source's key back"
    );
    let after = row(&fx.db, flow).await.unwrap();
    assert_eq!(after.evaluated_at, None, "no tick was evaluated");
    let next = after.next_evaluation_date.expect("on the queue");
    assert!(
        next > db_now(&fx.db).await,
        "re-initialised by sync to a future tick, not advanced by tick: {next}"
    );
}

/// The other half of spec §4 point 5 that had no test: the claimed row's
/// `trigger_id` is no longer declared by the source (renamed behind the
/// row's back). Frozen, and `sync` replaces the row with the declared one.
#[tokio::test]
async fn an_undeclared_trigger_id_does_not_fire_and_resyncs() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("renamed", "", "")).await;
    common::make_due(&fx.db, flow, "s", 1).await;
    let renamed = flow_yaml("renamed", "", "").replace("  - id: s\n", "  - id: s2\n");
    assert!(renamed.contains("id: s2\n"), "control: the rename landed");
    egret_workflow::spec::parse(&renamed).expect("the renamed source parses");
    // Raw SQL, so the row `s` survives and only the pass can notice.
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(&renamed)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            frozen: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, flow).await, 0);
    assert_eq!(row(&fx.db, flow).await, None, "the undeclared row is gone");
    assert!(
        spec_key_of(&fx.db, flow, "s2").await.is_some(),
        "and the declared one exists"
    );
}

/// Final review I3, the user's decision of 2026-09-25: a workspace whose
/// `workflows` feature is OFF fires nothing, and its schedules FREEZE -- no
/// tick consumed -- so turning the feature back on recovers every missed tick
/// under `ALL`, like every other off switch. "Off" is read from the table the
/// route gate reads (`workspace_features`, `enabled = false`).
#[tokio::test]
async fn a_workspace_with_workflows_off_freezes_its_schedules() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("feature_off", "", "")).await;
    // Another workspace, feature on: the switch is per workspace.
    let (other_ws, other_user) = common::seed_workspace(&fx.db).await;
    let other = common::seed_flow(
        &fx.db,
        other_ws,
        other_user,
        &flow_yaml("feature_on", "", ""),
        "draft",
    )
    .await;
    common::make_due(&fx.db, flow, "s", 3).await;
    common::make_due(&fx.db, other, "s", 1).await;
    let first = due_date(&fx.db, flow).await;
    common::disable_feature(&fx.db, fx.ws, "workflows").await;
    let before = row(&fx.db, flow).await.unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        },
        "only the other workspace's trigger"
    );
    assert_eq!(executions_of(&fx.db, other).await, 1);
    assert_eq!(common::execution_count(&fx.db, fx.ws).await, 0);
    assert_eq!(
        row(&fx.db, flow).await.unwrap(),
        before,
        "frozen: never claimed, so not even `updated_at` moved"
    );

    sqlx::query(
        "UPDATE workspace_features SET enabled = true \
         WHERE workspace_id = $1 AND feature = 'workflows'",
    )
    .bind(fx.ws)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    for i in 1..=3 {
        assert_eq!(
            fx.pass().await,
            PassReport {
                fired: 1,
                ..Default::default()
            },
            "pass {i} after the feature came back"
        );
    }
    let want: Vec<_> = (0..3).map(|k| first + Duration::minutes(k)).collect();
    assert_eq!(
        common::scheduled_dates(&fx.db, flow).await,
        want,
        "every tick missed while off is recovered"
    );
}

/// Companion to `a_workspace_with_workflows_off_freezes_its_schedules`: an
/// archived workspace (`workspaces.status <> 'active'`) freezes its schedules
/// by the same rule `WsContext` (`crates/egret-workspace/src/context.rs`
/// ~line 45: `.filter(|w| w.status == "active")`) and the webhook route
/// (`crates/egret-workflow/src/webhook_routes.rs` ~line 518) already apply to
/// every other entry point. No tick is consumed while archived, so
/// re-activating the workspace recovers every missed tick under `ALL`, like
/// every other off switch.
#[tokio::test]
async fn an_archived_workspace_freezes_its_schedules() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("archived_ws", "", "")).await;
    // Another workspace, active: the switch is per workspace.
    let (other_ws, other_user) = common::seed_workspace(&fx.db).await;
    let other = common::seed_flow(
        &fx.db,
        other_ws,
        other_user,
        &flow_yaml("archived_ws_other", "", ""),
        "draft",
    )
    .await;
    common::make_due(&fx.db, flow, "s", 3).await;
    common::make_due(&fx.db, other, "s", 1).await;
    let first = due_date(&fx.db, flow).await;
    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(fx.ws)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let before = row(&fx.db, flow).await.unwrap();

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            ..Default::default()
        },
        "only the other workspace's trigger"
    );
    assert_eq!(executions_of(&fx.db, other).await, 1);
    assert_eq!(common::execution_count(&fx.db, fx.ws).await, 0);
    assert_eq!(
        row(&fx.db, flow).await.unwrap(),
        before,
        "frozen: never claimed, so not even `updated_at` moved"
    );

    sqlx::query("UPDATE workspaces SET status = 'active' WHERE id = $1")
        .bind(fx.ws)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    for i in 1..=3 {
        assert_eq!(
            fx.pass().await,
            PassReport {
                fired: 1,
                ..Default::default()
            },
            "pass {i} after the workspace was reactivated"
        );
    }
    let want: Vec<_> = (0..3).map(|k| first + Duration::minutes(k)).collect();
    assert_eq!(
        common::scheduled_dates(&fx.db, flow).await,
        want,
        "every tick missed while archived is recovered"
    );
}

/// **The 2026-10-11 ruling, on a schedule fire.** A flow whose `Pause`
/// declares `behavior:` no longer plans, so its tick is refused like any
/// other flow that does not plan: no execution, taken off the queue, and the
/// trigger's `last_error` names the property. Before the ruling it fired and
/// the execution stalled in `RUNNING` for ever. The sibling, a plain `Pause`,
/// fires in the same pass -- the control.
#[tokio::test]
async fn a_pause_egret_does_not_run_is_refused_at_fire_and_its_sibling_still_fires() {
    let Some(fx) = fx().await else { return };
    let pause = |id: &str, extra: &str| {
        format!(
            "id: {id}\nnamespace: w12\ntriggers:\n  - id: s\n    type: {SCHEDULE}\n    \
             cron: \"*/1 * * * *\"\ntasks:\n  - id: gate\n    \
             type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n{extra}"
        )
    };
    let broken = fx
        .flow(&pause("pause-behavior", "    behavior: RESUME\n"))
        .await;
    let sibling = fx.flow(&pause("pause-plain", "")).await;
    common::make_due(&fx.db, broken, "s", 2).await;
    common::make_due(&fx.db, sibling, "s", 1).await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, broken).await, 0);
    assert_eq!(executions_of(&fx.db, sibling).await, 1);
    let after = row(&fx.db, broken).await.unwrap();
    assert_eq!(after.next_evaluation_date, None, "off the queue");
    let err = after.last_error.expect("and says why");
    assert!(
        err.contains("unsupported_pause_property") && err.contains("tasks.gate.behavior"),
        "{err}"
    );
}

/// **The 2026-10-12 ruling, on a schedule fire.** A flow holding a `ForEach`
/// no longer plans, so its tick is refused: no execution, off the queue, and
/// `last_error` names `unsupported_foreach`. The sibling, the same flow with
/// `Loop`, fires in the same pass.
#[tokio::test]
async fn a_foreach_is_refused_at_fire_and_its_loop_sibling_still_fires() {
    let Some(fx) = fx().await else { return };
    let src = |id: &str, task: &str| {
        format!(
            "id: {id}\nnamespace: w12\ntriggers:\n  - id: s\n    type: {SCHEDULE}\n    \
             cron: \"*/1 * * * *\"\ntasks:\n{task}"
        )
    };
    let broken = fx.flow(&src("fe", "  - id: fan\n    type: io.kestra.plugin.core.flow.ForEach\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n")).await;
    let sibling = fx.flow(&src("lp", "  - id: fan\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n")).await;
    common::make_due(&fx.db, broken, "s", 2).await;
    common::make_due(&fx.db, sibling, "s", 1).await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, broken).await, 0);
    // Roots only: the Loop's iterations are child executions.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.executions \
              WHERE flow_id = $1 AND parent_execution_id IS NULL",
        )
        .bind(sibling)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap(),
        1
    );
    let after = row(&fx.db, broken).await.unwrap();
    assert_eq!(after.next_evaluation_date, None, "off the queue");
    let err = after.last_error.expect("and says why");
    assert!(
        err.contains("unsupported_foreach") && err.contains("tasks.fan"),
        "{err}"
    );
}
