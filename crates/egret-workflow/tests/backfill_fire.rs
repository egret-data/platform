//! W7f Task 4: `scheduler::fire_due` walks a schedule backfill one window tick
//! per pass, and resumes the regular schedule when the window ends. Requires
//! DATABASE_URL.
//!
//! **The tests in this binary run one at a time** (`SERIAL`) and each starts
//! by taking every trigger an earlier test left behind off the queue, for
//! `tests/scheduler_fire.rs`'s reason: a pass is global, so a `PassReport`
//! would otherwise count other tests' triggers.
//!
//! Windows are built from the database clock: `w0` is ten whole minutes
//! before `now()`, on the fixture's `*/1 * * * *`, so every window tick is
//! past and fixed however long a test takes. A backfill is staged by SQL
//! (`common::stage_backfill`) until Task 7's create route exists.
//!
//! See docs/en/specs/2026-10-15-workflow-w7f-backfill-design.md §3.5, §3.6.

mod common;

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, DurationRound, Utc};
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::scheduler::{fire_due, PassReport};
use egret_workflow::trigger::{tick, FixedClock, ScheduleTrigger, TickOutcome, TriggerState};
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";

/// A flow `id` with one `*/1 * * * *` schedule `s` (UTC). `prelude` goes
/// before `triggers:`, `extra` is appended to the trigger (4-space indented).
fn flow_yaml(id: &str, prelude: &str, extra: &str) -> String {
    let src = format!(
        "id: {id}\nnamespace: w7f\n{prelude}triggers:\n  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n{extra}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    egret_workflow::spec::parse(&src)
        .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"));
    src
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
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = NULL")
        .execute(&db.pool)
        .await
        .unwrap();
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

    async fn pass(&self) -> PassReport {
        fire_due(&self.db.pool).await.expect("a pass")
    }

    async fn sql(&self, q: &'static str, flow: Uuid) {
        sqlx::query(q)
            .bind(flow)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    /// `w0`: ten whole minutes before the database's `now()`.
    async fn w0(&self) -> DateTime<Utc> {
        sqlx::query_scalar("SELECT date_trunc('minute', now()) - interval '10 minutes'")
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn now(&self) -> DateTime<Utc> {
        sqlx::query_scalar("SELECT now()")
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }
}

fn fired(n: usize) -> PassReport {
    PassReport {
        fired: n,
        ..Default::default()
    }
}

fn minutes(n: i64) -> Duration {
    Duration::minutes(n)
}

fn trunc_minute(at: DateTime<Utc>) -> DateTime<Utc> {
    at.duration_trunc(minutes(1)).unwrap()
}

/// Everything about a trigger row that a pass could change
/// (`scheduler_fire.rs`'s `Row`).
#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Row {
    next_evaluation_date: Option<DateTime<Utc>>,
    evaluated_at: Option<DateTime<Utc>>,
    disabled: bool,
    last_error: Option<String>,
    updated_at: DateTime<Utc>,
}

async fn row(db: &Db, flow: Uuid) -> Row {
    sqlx::query_as(
        "SELECT next_evaluation_date, evaluated_at, disabled, last_error, updated_at \
         FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// The backfill row's moving parts, typed.
#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Bf {
    current_tick: DateTime<Utc>,
    ticks_done: i32,
    total_ticks: i32,
    updated_at: DateTime<Utc>,
}

async fn bf(db: &Db, flow: Uuid) -> Option<Bf> {
    sqlx::query_as(
        "SELECT current_tick, ticks_done, total_ticks, updated_at \
           FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct Exec {
    scheduled_date: DateTime<Utc>,
    backfill_id: Option<Uuid>,
    inputs: Value,
    labels: Option<Value>,
    execution_trigger: Value,
}

async fn execs(db: &Db, flow: Uuid) -> Vec<Exec> {
    sqlx::query_as(
        "SELECT scheduled_date, backfill_id, inputs, labels, execution_trigger \
           FROM workflow.executions WHERE flow_id = $1 ORDER BY created_at, scheduled_date",
    )
    .bind(flow)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// What a LIVE fire of `date` records as `execution_trigger`, built here from
/// the public model (`ScheduleTrigger::resolve` + `tick` with no backfill) and
/// formatted independently of `schedule_trigger_record` (UTC: `Z`, six
/// fractional digits). §3.2: a backfill execution's record is the oracle's.
fn live_record(src: &str, date: DateTime<Utc>) -> Value {
    let flow = egret_workflow::spec::parse(src).unwrap();
    let resolved = ScheduleTrigger::resolve(&flow.triggers[0]).unwrap();
    let state = TriggerState {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        trigger_id: "s".into(),
        next_evaluation_date: Some(date),
        evaluated_at: None,
        backfill: None,
    };
    let t = tick(&resolved, &state, &BTreeMap::new(), &FixedClock(date));
    let TickOutcome::Fired(e) = t.outcome else {
        panic!("a live tick at its own date fires: {:?}", t.outcome)
    };
    let fmt = |at: DateTime<Utc>| at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    json!({
        "id": "s",
        "type": SCHEDULE,
        "variables": {
            "date": fmt(e.scheduled_date),
            "next": e.next_date.map(fmt),
            "previous": e.previous_date.map(fmt),
        }
    })
}

/// Test 1. A 5-tick window drains in five passes, in order, each execution
/// carrying the backfill's id, the merged inputs, the staged labels and the
/// record a live fire of that tick would have; the row goes with the last.
#[tokio::test]
async fn a_window_drains_tick_by_tick_and_the_row_goes_with_the_last() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml(
        "drain",
        "",
        "    inputs:\n      region: us\n      size: 3\n",
    );
    let flow = fx.flow(&src).await;
    let w0 = fx.w0().await;
    let labels = json!([{"key": "team", "value": "ops"}, {"key": "reason", "value": "test"}]);
    let id = common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({"region": "eu"}),
        labels.clone(),
    )
    .await;

    for pass in 1..=4 {
        assert_eq!(fx.pass().await, fired(1), "pass {pass}");
    }
    let b = bf(&fx.db, flow).await.expect("still running after 4 of 5");
    assert_eq!((b.current_tick, b.ticks_done), (w0 + minutes(4), 4));
    // P4: the last tick's advance would bring `ticks_done` to `total_ticks`;
    // the row is deleted instead of written with it.
    assert_eq!(b.ticks_done + 1, b.total_ticks);
    assert_eq!(fx.pass().await, fired(1), "pass 5");
    assert!(
        common::backfill_row(&fx.db, flow, "s").await.is_none(),
        "the row goes with the last tick"
    );

    let e = execs(&fx.db, flow).await;
    let dates: Vec<_> = e.iter().map(|e| e.scheduled_date).collect();
    assert_eq!(dates, (0..5).map(|i| w0 + minutes(i)).collect::<Vec<_>>());
    for e in &e {
        assert_eq!(e.backfill_id, Some(id));
        assert_eq!(e.inputs, json!({"region": "eu", "size": 3}));
        assert_eq!(e.labels.as_ref(), Some(&labels));
        assert_eq!(e.execution_trigger, live_record(&src, e.scheduled_date));
    }
}

/// Test 2. Completion resumes the regular schedule (§3.6): from
/// `max(previous_next_evaluation_date, first tick after window_end)`, then
/// `recoverMissedSchedules` when that is past.
#[tokio::test]
async fn completion_resumes_the_regular_schedule() {
    let Some(fx) = fx().await else { return };

    // The regular schedule was not behind: it resumes where it stood, and
    // nothing regular fires.
    let flow = fx.flow(&flow_yaml("resume-future", "", "")).await;
    let w0 = fx.w0().await;
    common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0 + minutes(4),
        5,
        json!({}),
        json!([]),
    )
    .await;
    let prev: DateTime<Utc> = sqlx::query_scalar(
        "SELECT previous_next_evaluation_date FROM workflow.trigger_backfills WHERE flow_id = $1",
    )
    .bind(flow)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert!(
        prev > fx.now().await,
        "staged from a trigger due in the future"
    );
    assert_eq!(fx.pass().await, fired(1), "the window's last tick");
    assert_eq!(row(&fx.db, flow).await.next_evaluation_date, Some(prev));
    assert_eq!(
        fx.pass().await,
        PassReport::default(),
        "nothing regular is due"
    );
    let e = execs(&fx.db, flow).await;
    assert_eq!(e.len(), 1);
    assert!(e[0].backfill_id.is_some());
    // An empty label list is stored as NULL.
    assert_eq!(e[0].labels, None);
    // Off the queue, so it cannot come due during the variants below.
    fx.sql(
        "UPDATE workflow.triggers SET next_evaluation_date = NULL WHERE flow_id = $1",
        flow,
    )
    .await;

    // A regular backlog of three ticks before the window: `max` starts at
    // the first tick after the window (w0+5m), so those three are not
    // recovered (the spec's rule; Task 10 reports it), and the policy then
    // decides about w0+5m..now.
    for (name, extra, policy) in [
        ("resume-absent", "", "ALL"),
        ("resume-all", "    recoverMissedSchedules: ALL\n", "ALL"),
        ("resume-last", "    recoverMissedSchedules: LAST\n", "LAST"),
        ("resume-none", "    recoverMissedSchedules: NONE\n", "NONE"),
    ] {
        let flow = fx.flow(&flow_yaml(name, "", extra)).await;
        let w0 = fx.w0().await;
        common::stage_backfill(
            &fx.db,
            flow,
            "s",
            (w0, w0 + minutes(4)),
            w0 + minutes(4),
            5,
            json!({}),
            json!([]),
        )
        .await;
        sqlx::query(
            "UPDATE workflow.trigger_backfills SET previous_next_evaluation_date = $2 \
              WHERE flow_id = $1",
        )
        .bind(flow)
        .bind(w0 - minutes(3))
        .execute(&fx.db.pool)
        .await
        .unwrap();
        let before = fx.now().await;
        assert_eq!(fx.pass().await, fired(1), "{name}: the window's last tick");
        let after = fx.now().await;
        let next = row(&fx.db, flow)
            .await
            .next_evaluation_date
            .unwrap_or_else(|| panic!("{name}: on the queue"));
        match policy {
            "ALL" => assert_eq!(next, w0 + minutes(5), "{name}"),
            "LAST" => assert!(
                trunc_minute(before) <= next && next <= trunc_minute(after),
                "{name}: the latest tick at or before now, got {next} in [{before}, {after}]"
            ),
            _ => assert!(
                trunc_minute(before) + minutes(1) <= next
                    && next <= trunc_minute(after) + minutes(1),
                "{name}: the first tick after now, got {next} in [{before}, {after}]"
            ),
        }
        assert_eq!(next, trunc_minute(next), "{name}: a tick");
        // Not the live clock's date (`tick`'s): that would be the first tick
        // after now, which only NONE asks for.
        if policy != "NONE" {
            assert!(next <= after, "{name}");
        }
        fx.sql(
            "UPDATE workflow.triggers SET next_evaluation_date = NULL WHERE flow_id = $1",
            flow,
        )
        .await;
    }
}

/// Test 3. A backfill re-runs dates that already ran (user decision), and a
/// second backfill over the same window re-runs them again: 3 per date.
#[tokio::test]
async fn a_backfill_reruns_dates_that_already_ran() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("rerun", "", "")).await;
    common::make_due(&fx.db, flow, "s", 3).await;
    for _ in 0..3 {
        assert_eq!(fx.pass().await, fired(1));
    }
    let dates = common::scheduled_dates(&fx.db, flow).await;
    assert_eq!(dates.len(), 3);
    let window = (dates[0], dates[2]);

    let first =
        common::stage_backfill(&fx.db, flow, "s", window, dates[0], 3, json!({}), json!([])).await;
    for _ in 0..3 {
        assert_eq!(fx.pass().await, fired(1));
    }
    assert!(common::backfill_row(&fx.db, flow, "s").await.is_none());
    let second =
        common::stage_backfill(&fx.db, flow, "s", window, dates[0], 3, json!({}), json!([])).await;
    assert_ne!(first, second);
    for _ in 0..3 {
        assert_eq!(fx.pass().await, fired(1));
    }

    let e = execs(&fx.db, flow).await;
    assert_eq!(e.len(), 9);
    for d in &dates {
        let mut ids: Vec<_> = e
            .iter()
            .filter(|e| e.scheduled_date == *d)
            .map(|e| e.backfill_id)
            .collect();
        ids.sort();
        let mut want = vec![None, Some(first), Some(second)];
        want.sort();
        assert_eq!(ids, want, "three executions for {d}");
    }
}

/// Test 4 (D7). A paused backfill is not claimed: no pass counts it, and
/// neither its row nor the trigger's is written. Unpaused, the next pass
/// fires `current_tick`.
#[tokio::test]
async fn a_paused_backfill_is_not_claimed() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_yaml("paused", "", "")).await;
    let w0 = fx.w0().await;
    common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({}),
        json!([]),
    )
    .await;
    fx.sql(
        "UPDATE workflow.trigger_backfills SET paused = true, paused_at = now() WHERE flow_id = $1",
        flow,
    )
    .await;
    let trigger_before = row(&fx.db, flow).await;
    let bf_before = common::backfill_row(&fx.db, flow, "s").await;

    for pass in 1..=3 {
        assert_eq!(fx.pass().await, PassReport::default(), "pass {pass}");
    }
    assert_eq!(row(&fx.db, flow).await, trigger_before);
    assert_eq!(common::backfill_row(&fx.db, flow, "s").await, bf_before);
    assert!(execs(&fx.db, flow).await.is_empty());

    fx.sql(
        "UPDATE workflow.trigger_backfills SET paused = false, paused_at = NULL WHERE flow_id = $1",
        flow,
    )
    .await;
    assert_eq!(fx.pass().await, fired(1));
    let e = execs(&fx.db, flow).await;
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].scheduled_date, w0);
    assert_eq!(
        bf(&fx.db, flow).await.unwrap().current_tick,
        w0 + minutes(1)
    );
}

/// Test 5 (§3.4). A templated schedule input the backfill does not override
/// is refused after the merge, and the refusal consumes the window tick.
#[tokio::test]
async fn templated_merged_inputs_consume_the_tick() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_yaml(
            "bf-templated",
            "inputs:\n  - id: x\n    type: STRING\n",
            "    inputs:\n      x: \"{{ now() }}\"\n      y: plain\n",
        ))
        .await;
    let w0 = fx.w0().await;
    common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({"y": "other"}),
        json!([]),
    )
    .await;

    assert_eq!(
        fx.pass().await,
        PassReport {
            skipped: 1,
            ..Default::default()
        }
    );
    assert!(execs(&fx.db, flow).await.is_empty());
    let err = row(&fx.db, flow).await.last_error.expect("the refusal");
    assert!(err.contains("inputs.x"), "{err}");
    let b = bf(&fx.db, flow).await.unwrap();
    assert_eq!((b.current_tick, b.ticks_done), (w0 + minutes(1), 1));
}

/// Test 6. A window tick that already has this backfill's execution is
/// consumed (`AlreadyFired` via `workflow_executions_one_per_backfill_tick`)
/// and the backfill advances.
#[tokio::test]
async fn already_fired_advances_the_backfill() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml("bf-already", "", "");
    let flow = fx.flow(&src).await;
    let w0 = fx.w0().await;
    let id = common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({}),
        json!([]),
    )
    .await;
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
            execution_trigger, scheduled_date, backfill_id) \
         SELECT workspace_id, id, namespace, flow_id, source, 'SUCCESS', '[]'::jsonb, '{}'::jsonb, \
                $2, $3, $4 \
           FROM workflow.flows WHERE id = $1",
    )
    .bind(flow)
    .bind(live_record(&src, w0))
    .bind(w0)
    .bind(id)
    .execute(&fx.db.pool)
    .await
    .expect("an execution for the backfill's first tick");

    assert_eq!(
        fx.pass().await,
        PassReport {
            skipped: 1,
            ..Default::default()
        }
    );
    let err = row(&fx.db, flow).await.last_error.expect("the reason");
    assert!(
        err.contains("this backfill tick already has an execution"),
        "{err}"
    );
    assert_eq!(execs(&fx.db, flow).await.len(), 1);
    let b = bf(&fx.db, flow).await.unwrap();
    assert_eq!((b.current_tick, b.ticks_done), (w0 + minutes(1), 1));
}

/// DDL naming the test's own flow. The only interpolation is a `Uuid` the
/// test generated, never input.
async fn ddl(db: &Db, sql: String) {
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&db.pool)
        .await
        .unwrap();
}

/// `scheduler_fire.rs`'s fault: RAISE on `event` on `table` for `flow`'s
/// rows only.
async fn install_fault(db: &Db, flow: Uuid, event: &str, table: &str) {
    let hex = flow.simple();
    drop_fault(db, flow, table).await;
    ddl(
        db,
        format!(
            "CREATE FUNCTION public.w12_fail_{hex}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN \
                  IF NEW.flow_id = '{flow}' THEN RAISE EXCEPTION 'w7f injected'; END IF; \
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

/// Test 7. The early-return paths leave the backfill row alone: a source
/// that no longer parses takes the trigger off the queue; a failed start
/// records the error and the next pass retries the same window tick.
#[tokio::test]
async fn the_error_paths_leave_the_backfill_alone() {
    let Some(fx) = fx().await else { return };

    let broken = fx.flow(&flow_yaml("bf-broken", "", "")).await;
    let w0 = fx.w0().await;
    common::stage_backfill(
        &fx.db,
        broken,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({}),
        json!([]),
    )
    .await;
    let before = common::backfill_row(&fx.db, broken, "s").await;
    sqlx::query("UPDATE workflow.flows SET source = 'id: broken\ntasks: [\n' WHERE id = $1")
        .bind(broken)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(
        fx.pass().await,
        PassReport {
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(row(&fx.db, broken).await.next_evaluation_date, None);
    assert_eq!(common::backfill_row(&fx.db, broken, "s").await, before);

    let failing = fx.flow(&flow_yaml("bf-failing", "", "")).await;
    common::stage_backfill(
        &fx.db,
        failing,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({}),
        json!([]),
    )
    .await;
    let before = common::backfill_row(&fx.db, failing, "s").await;
    install_fault(&fx.db, failing, "BEFORE INSERT", "workflow.executions").await;
    let report = fx.pass().await;
    drop_fault(&fx.db, failing, "workflow.executions").await;
    assert_eq!(
        report,
        PassReport {
            errored: 1,
            ..Default::default()
        }
    );
    let err = row(&fx.db, failing).await.last_error.expect("recorded");
    assert!(err.contains("w7f injected"), "{err}");
    assert_eq!(common::backfill_row(&fx.db, failing, "s").await, before);
    assert_eq!(row(&fx.db, failing).await.next_evaluation_date, Some(w0));

    assert_eq!(fx.pass().await, fired(1), "the retry");
    let e = execs(&fx.db, failing).await;
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].scheduled_date, w0, "the same window tick");
    assert_eq!(
        bf(&fx.db, failing).await.unwrap().current_tick,
        w0 + minutes(1)
    );
}

/// Review follow-up 1 (default decision, 2026-10-15). A backfill tick whose
/// start is REFUSED (here: the backfill's overlaid input is not a FILE URI)
/// ends the backfill in the same transaction, as a delete does: the row goes,
/// the regular schedule resumes at `max(previous_next_evaluation_date,
/// current_tick)` under its policy, and `last_error` names the backfill. The
/// trigger stays on the queue: taking it off would leave it stuck, because a
/// save re-derives the same tick with the same inputs.
#[tokio::test]
async fn a_refused_backfill_start_ends_the_backfill() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_yaml(
            "bf-refused",
            "inputs:\n  - id: f\n    type: FILE\n    required: false\n",
            "    inputs:\n      f: null\n",
        ))
        .await;
    let w0 = fx.w0().await;
    common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({"f": 42}),
        json!([]),
    )
    .await;
    let prev: DateTime<Utc> = sqlx::query_scalar(
        "SELECT previous_next_evaluation_date FROM workflow.trigger_backfills WHERE flow_id = $1",
    )
    .bind(flow)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert!(
        prev > w0,
        "the regular schedule stood after the window's tick"
    );

    assert_eq!(
        fx.pass().await,
        PassReport {
            errored: 1,
            ..Default::default()
        }
    );
    assert!(execs(&fx.db, flow).await.is_empty());
    assert!(
        common::backfill_row(&fx.db, flow, "s").await.is_none(),
        "the backfill ended"
    );
    let after = row(&fx.db, flow).await;
    assert_eq!(
        after.next_evaluation_date,
        Some(prev),
        "queued at the resume: max(prev, current_tick) = prev"
    );
    let err = after.last_error.expect("the reason");
    assert!(err.starts_with("backfill ended: "), "{err}");
    assert!(err.contains("FILE input"), "{err}");
}

/// Review follow-up 3 (D7). The claim's `NOT EXISTS` is per trigger: a paused
/// backfill on one schedule does not hold back its sibling in the same flow.
#[tokio::test]
async fn a_paused_backfill_does_not_hold_back_a_sibling_schedule() {
    let Some(fx) = fx().await else { return };
    let src = format!(
        "id: siblings\nnamespace: w7f\ntriggers:\n  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n  - id: t\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let flow = fx.flow(&src).await;
    let w0 = fx.w0().await;
    common::stage_backfill(
        &fx.db,
        flow,
        "s",
        (w0, w0 + minutes(4)),
        w0,
        5,
        json!({}),
        json!([]),
    )
    .await;
    fx.sql(
        "UPDATE workflow.trigger_backfills SET paused = true, paused_at = now() WHERE flow_id = $1",
        flow,
    )
    .await;
    common::make_due(&fx.db, flow, "t", 1).await;

    assert_eq!(fx.pass().await, fired(1), "the sibling fires");
    let fired_by: Vec<String> = sqlx::query_scalar(
        "SELECT execution_trigger->>'id' FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(flow)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(fired_by, vec!["t".to_string()]);
}
