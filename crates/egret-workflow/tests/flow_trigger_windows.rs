//! W13b Task 4: the `dependsOn` arm of `flow_trigger::process_events` -- the
//! window (`workflow.flow_trigger_windows`, migration 0158) that accumulates
//! the dependencies a listener has seen, and fires it once they satisfy its
//! `mode`. Requires DATABASE_URL.
//!
//! **The tests in this binary run one at a time** (`SERIAL`) and each starts
//! by emptying the outbox, the executions and the listener rows (which
//! cascades their windows), as `tests/flow_trigger_fire.rs` does and for its
//! reason: a pass is global.
//!
//! Sources are finished through the real worker tick, so every event here was
//! written by the real `settle` hook. The window's time source is Postgres
//! `now()`; a test that needs a window in the past moves the ROW, never a
//! clock.
//!
//! See docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md §5.2-§5.3
//! and the user's decision D1 (the lifecycle egret implements, which is a
//! deliberate divergence from the artifact's).

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::flow_trigger::{
    process_events, process_events_with, sweep_expired_windows, EventReport, PassLimits,
    MAX_ATTEMPTS,
};
use egret_workflow::models::TriggerKind;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";

/// A one-task flow in namespace `w13` with no trigger.
fn source_yaml(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: w13\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

/// A one-task flow with one Flow trigger `on_done`. `prelude` goes before
/// `triggers:` (flow-level keys), `extra` is appended to the trigger
/// (4-space indented).
fn listener_yaml(id: &str, prelude: &str, extra: &str) -> String {
    let src = format!(
        "id: {id}\nnamespace: w13\n{prelude}triggers:\n  - id: on_done\n    type: {FLOW}\n{extra}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    egret_workflow::spec::parse(&src)
        .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"));
    src
}

/// `dependsOn` on each of `flows` (namespace `w13`, no `when`, no `states`),
/// then `more` (4-space indented trigger keys: `mode`, `window`, ...).
fn depends_on(flows: &[&str], more: &str) -> String {
    let mut s = String::from("    dependsOn:\n");
    for f in flows {
        s.push_str(&format!("      - namespace: w13\n        flowId: {f}\n"));
    }
    s.push_str(more);
    s
}

/// The listener's condition keys, in `dependsOn` order, computed by the
/// crate's own function from the fixture's own source.
fn keys_of(src: &str) -> Vec<String> {
    let flow = egret_workflow::spec::parse(src).expect("fixture parses");
    let TriggerKind::Flow(ft) = &flow.triggers[0].kind else {
        panic!("not a Flow trigger")
    };
    egret_workflow::trigger::condition_keys(&ft.depends_on).expect("distinct keys")
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
    ] {
        sqlx::query(sql).execute(&db.pool).await.expect(sql);
    }
    let windows: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.flow_trigger_windows")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(windows, 0, "control: the listeners' delete cascaded");
    let (ws, user) = common::seed_workspace(&db).await;
    Some(Fx {
        db,
        ws,
        user,
        _serial: serial,
    })
}

/// A source flow: its id and its source.
struct Src {
    id: Uuid,
    yaml: String,
}

impl Fx {
    async fn flow(&self, src: &str) -> Uuid {
        common::seed_flow(&self.db, self.ws, self.user, src, "draft").await
    }

    async fn source(&self, name: &str) -> Src {
        let yaml = source_yaml(name);
        Src {
            id: self.flow(&yaml).await,
            yaml,
        }
    }

    /// Start `src` and finish it SUCCESS through the real worker.
    async fn run(&self, src: &Src) -> Uuid {
        run_in(&self.db, self.ws, src).await
    }

    async fn pass(&self) -> EventReport {
        process_events(&self.db.pool).await.expect("a pass")
    }

    /// Run `src` and process the outbox until it is empty (the listener's own
    /// execution, run by a later `drive_to`, writes events too).
    async fn event(&self, src: &Src) -> EventReport {
        self.run(src).await;
        self.pass().await
    }
}

async fn run_in(db: &Db, ws: Uuid, src: &Src) -> Uuid {
    let id = egret_workflow::worker::start_execution(
        &db.pool,
        ws,
        Some(src.id),
        &src.yaml,
        serde_json::json!({}),
    )
    .await
    .expect("start the source");
    common::drive_to(db, id, "SUCCESS").await;
    id
}

async fn executions_of(db: &Db, flow: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
        .bind(flow)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// A listener's window row.
#[derive(Debug, Clone, PartialEq)]
struct Win {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    results: BTreeMap<String, bool>,
}

impl Win {
    fn keys(&self) -> Vec<String> {
        self.results.keys().cloned().collect()
    }
}

type WinRow = (
    DateTime<Utc>,
    DateTime<Utc>,
    sqlx::types::Json<BTreeMap<String, bool>>,
);

async fn window(db: &Db, flow: Uuid) -> Option<Win> {
    let row: Option<WinRow> = sqlx::query_as(
        "SELECT window_start, window_end, results FROM workflow.flow_trigger_windows \
              WHERE flow_id = $1 AND trigger_id = 'on_done'",
    )
    .bind(flow)
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    row.map(|(start, end, results)| Win {
        start,
        end,
        results: results.0,
    })
}

async fn window_count(db: &Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.flow_trigger_windows")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// Move `flow`'s window a day into the past: expired, and still `start < end`.
async fn expire(db: &Db, flow: Uuid) {
    let n = sqlx::query(
        "UPDATE workflow.flow_trigger_windows \
            SET window_start = now() - interval '2 days', window_end = now() - interval '1 day' \
          WHERE flow_id = $1",
    )
    .bind(flow)
    .execute(&db.pool)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1, "control: the window to expire exists");
}

async fn pg_now(db: &Db) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// Collects `tracing` output (as `tests/flow_trigger_fire.rs` does).
/// `set_default` is thread-local and `#[tokio::test]` runs on one thread, so
/// this captures the calling test's events, including those of futures it
/// joins.
#[derive(Clone, Default)]
struct LogCapture(Arc<StdMutex<Vec<u8>>>);

struct LogWriter(Arc<StdMutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

impl LogCapture {
    fn install(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(self.clone())
                .with_ansi(false)
                .without_time()
                .finish(),
        )
    }
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

// ── The three modes ─────────────────────────────────────────────────────────

/// Spec §5.3 steps 2-5 under `ALL` (the default mode, no `window:` so the
/// default one-day `DURATION_WINDOW`). Neither dependency declares a `when`:
/// the model fills in `"true"`, which the processor must render and pass as
/// `Rendered` -- `NotDeclared` would make both fail closed and this never
/// fire.
///
/// After the fire the window row is GONE (D1: deleted on fire under every
/// mode), so the next `p2` alone opens a fresh window and does not fire;
/// only a new `p1` completes it again.
#[tokio::test]
async fn all_fires_once_both_are_met_and_resets() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let src = listener_yaml("listener", "", &depends_on(&["p1", "p2"], ""));
    let [k1, k2]: [String; 2] = keys_of(&src).try_into().unwrap();
    let listener = fx.flow(&src).await;

    // p1: no fire, one result.
    let r = fx.event(&p1).await;
    assert_eq!(
        r,
        EventReport {
            events: 1,
            accumulated: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, listener).await, 0);
    let w = window(&fx.db, listener).await.expect("p1 opened a window");
    assert_eq!(w.keys(), vec![k1.clone()], "exactly p1's condition is met");
    let now = pg_now(&fx.db).await;
    assert!(w.start < now && now < w.end, "a live window: {w:?}");

    // p2: fires once, with p2 as the source that completed the condition.
    let p2_run = fx.run(&p2).await;
    let r = fx.pass().await;
    assert_eq!(
        r,
        EventReport {
            events: 1,
            fired: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, listener).await, 1);
    let fired = common::fired_by(&fx.db, p2_run).await;
    assert_eq!(
        fired.len(),
        1,
        "the fire names p2's execution as its source"
    );
    assert_eq!(fired[0].0, listener);
    assert_eq!(fired[0].2["variables"]["flowId"], "p2");
    assert_eq!(fired[0].2["type"], FLOW);
    assert_eq!(window(&fx.db, listener).await, None, "deleted on fire");

    // p2 again: a FRESH window, holding p2 only -- no early fire.
    fx.event(&p2).await;
    assert_eq!(
        executions_of(&fx.db, listener).await,
        1,
        "a met condition does not carry into the next window"
    );
    let w = window(&fx.db, listener).await.expect("p2 opened a window");
    assert_eq!(w.keys(), vec![k2.clone()]);

    // p1 again: fires the second time.
    fx.event(&p1).await;
    assert_eq!(executions_of(&fx.db, listener).await, 2);
    assert_eq!(window(&fx.db, listener).await, None);
}

/// `ANY` fires on the first matching event, and -- the window being deleted
/// on every fire (measured on the artifact, D1) -- on every matching event.
#[tokio::test]
async fn any_fires_on_the_first() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let listener = fx
        .flow(&listener_yaml(
            "listener",
            "",
            &depends_on(&["p1", "p2"], "    mode: ANY\n"),
        ))
        .await;

    let r = fx.event(&p1).await;
    assert_eq!(r.fired, 1, "{r:?}");
    assert_eq!(executions_of(&fx.db, listener).await, 1);
    assert_eq!(window(&fx.db, listener).await, None, "deleted on fire");
    fx.event(&p1).await;
    assert_eq!(executions_of(&fx.db, listener).await, 2);
    fx.event(&p2).await;
    assert_eq!(executions_of(&fx.db, listener).await, 3);
    assert_eq!(window(&fx.db, listener).await, None);
}

/// `AT_LEAST` 2 of 3: the first dependency accumulates, the second fires, and
/// the window starts again from nothing (deleted on fire, measured).
#[tokio::test]
async fn at_least_two_of_three() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let p3 = fx.source("p3").await;
    let src = listener_yaml(
        "listener",
        "",
        &depends_on(
            &["p1", "p2", "p3"],
            "    mode: AT_LEAST\n    minSatisfied: 2\n",
        ),
    );
    let keys = keys_of(&src);
    let listener = fx.flow(&src).await;

    fx.event(&p3).await;
    assert_eq!(executions_of(&fx.db, listener).await, 0, "one of three");
    assert_eq!(
        window(&fx.db, listener).await.unwrap().keys(),
        vec![keys[2].clone()]
    );
    fx.event(&p3).await;
    assert_eq!(
        executions_of(&fx.db, listener).await,
        0,
        "the same condition twice is still one"
    );
    fx.event(&p1).await;
    assert_eq!(executions_of(&fx.db, listener).await, 1, "two of three");
    assert_eq!(window(&fx.db, listener).await, None);
    fx.event(&p2).await;
    assert_eq!(
        executions_of(&fx.db, listener).await,
        1,
        "starts again from nothing"
    );
}

/// A dependency that declares no `when` is still met: the model's default
/// `"true"` is rendered, not passed as `NotDeclared` (which fails closed).
/// Beside it, one whose `when` renders falsy on the source is not.
#[tokio::test]
async fn a_dependency_with_no_when_is_met() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let bare = fx
        .flow(&listener_yaml("bare", "", &depends_on(&["p1"], "")))
        .await;
    let falsy = fx
        .flow(&listener_yaml(
            "falsy",
            "",
            "    dependsOn:\n      - namespace: w13\n        flowId: p1\n        when: \"false\"\n",
        ))
        .await;
    fx.event(&p1).await;
    assert_eq!(executions_of(&fx.db, bare).await, 1, "no `when` is met");
    assert_eq!(
        executions_of(&fx.db, falsy).await,
        0,
        "control: a falsy `when`"
    );
    assert_eq!(
        window(&fx.db, falsy).await,
        None,
        "a dependency that is not met is not a matching event, so no window opens"
    );
}

// ── Concurrency ─────────────────────────────────────────────────────────────

/// A `BEFORE INSERT` sleep on `table` (when `when` holds), so two passes'
/// transactions overlap for certain rather than by luck.
async fn install_sleep(db: &Db, name: &str, table: &str, when: &str, secs: f64) {
    for ddl in [
        format!(
            "CREATE OR REPLACE FUNCTION public.{name}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN PERFORM pg_sleep({secs}); RETURN NEW; END $$"
        ),
        format!("DROP TRIGGER IF EXISTS {name} ON {table}"),
        format!(
            "CREATE TRIGGER {name} BEFORE INSERT ON {table} FOR EACH ROW WHEN ({when}) \
             EXECUTE FUNCTION public.{name}()"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl))
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

async fn drop_sleep(db: &Db, name: &str, table: &str) {
    for ddl in [
        format!("DROP TRIGGER IF EXISTS {name} ON {table}"),
        format!("DROP FUNCTION IF EXISTS public.{name}()"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl))
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// Counts every attempted insert of a flow-fired execution in a sequence,
/// which a rollback does not undo (W12 Task 5's counter).
async fn install_attempt_counter(db: &Db) {
    for ddl in [
        "DROP TRIGGER IF EXISTS w13b_count_fire ON workflow.executions",
        "DROP FUNCTION IF EXISTS public.w13b_count_fire()",
        "DROP SEQUENCE IF EXISTS public.w13b_fire_attempts",
        "CREATE SEQUENCE public.w13b_fire_attempts",
        "CREATE FUNCTION public.w13b_count_fire() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.flow_trigger_event_id IS NOT NULL THEN \
           PERFORM nextval('public.w13b_fire_attempts'); END IF; RETURN NEW; END $$",
        "CREATE TRIGGER w13b_count_fire BEFORE INSERT ON workflow.executions \
         FOR EACH ROW EXECUTE FUNCTION public.w13b_count_fire()",
    ] {
        sqlx::query(ddl).execute(&db.pool).await.unwrap();
    }
}

async fn attempts(db: &Db) -> i64 {
    sqlx::query_scalar(
        "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM public.w13b_fire_attempts",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn drop_attempt_counter(db: &Db) {
    for ddl in [
        "DROP TRIGGER IF EXISTS w13b_count_fire ON workflow.executions",
        "DROP FUNCTION IF EXISTS public.w13b_count_fire()",
        "DROP SEQUENCE IF EXISTS public.w13b_fire_attempts",
    ] {
        sqlx::query(ddl).execute(&db.pool).await.unwrap();
    }
}

/// Two passes at once, then passes until the outbox is empty (a pass that
/// met the other's lock keeps its event for the next one).
async fn two_passes_then_drain(db: &Db) -> (EventReport, EventReport) {
    let limits = PassLimits {
        retry_backoff: Duration::ZERO,
        ..PassLimits::default()
    };
    let cancel = CancellationToken::new();
    let (a, b) = tokio::join!(
        process_events_with(&db.pool, &limits, &cancel),
        process_events_with(&db.pool, &limits, &cancel),
    );
    let (a, b) = (a.expect("pass a"), b.expect("pass b"));
    for _ in 0..5 {
        if common::events(db).await.is_empty() {
            break;
        }
        process_events_with(&db.pool, &limits, &cancel)
            .await
            .expect("a draining pass");
    }
    assert_eq!(common::events(db).await, vec![], "drained");
    (a, b)
}

/// Spec §5.3 *Concurrency*: two sources that satisfy one listener, processed
/// by two passes at once, fire it ONCE -- asserted on rows **and** on insert
/// attempts, because a duplicate fire that a later statement rolls back leaves
/// the rows right.
///
/// **Phase A, the brief's shape:** `p1` and `p2` together, no window yet. A
/// `BEFORE INSERT` sleep on the window table makes both passes reach the
/// insert-if-absent together. One inserts; the other waits on the key and,
/// at `REPEATABLE READ`, gets `40001` rather than a second row -- or, under a
/// check-then-insert, `23505`, which the log assertion catches.
///
/// **Phase B, the race the row lock is for:** the window already holds `p1`,
/// and TWO `p2` events arrive at once, each of which completes `ALL` alone. A
/// sleep on the fire's insert keeps the first holder's transaction open. The
/// `FOR UPDATE` makes the second wait and then find the window gone; without
/// it both read `{p1}`, both fire, and only the second's `DELETE` fails --
/// after its fire was attempted.
#[tokio::test]
async fn two_processors_satisfying_all_together_fire_once() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let listener = fx
        .flow(&listener_yaml(
            "listener",
            "",
            &depends_on(&["p1", "p2"], ""),
        ))
        .await;
    install_attempt_counter(&fx.db).await;
    let logs = LogCapture::default();
    let _guard = logs.install();

    // Phase A.
    fx.run(&p1).await;
    fx.run(&p2).await;
    assert_eq!(common::events(&fx.db).await.len(), 2, "control: two events");
    install_sleep(
        &fx.db,
        "w13b_slow_window",
        "workflow.flow_trigger_windows",
        "true",
        0.4,
    )
    .await;
    let (a, b) = two_passes_then_drain(&fx.db).await;
    drop_sleep(&fx.db, "w13b_slow_window", "workflow.flow_trigger_windows").await;
    eprintln!("phase A: a = {a:?}, b = {b:?}");
    assert_eq!(
        executions_of(&fx.db, listener).await,
        1,
        "phase A: one fire"
    );
    assert_eq!(attempts(&fx.db).await, 1, "phase A: one attempt");
    assert_eq!(window(&fx.db, listener).await, None);

    // Phase B.
    fx.event(&p1).await;
    assert!(window(&fx.db, listener).await.is_some(), "control: {{p1}}");
    fx.run(&p2).await;
    fx.run(&p2).await;
    let before = executions_of(&fx.db, listener).await;
    let tried = attempts(&fx.db).await;
    install_sleep(
        &fx.db,
        "w13b_slow_fire",
        "workflow.executions",
        "NEW.flow_trigger_event_id IS NOT NULL",
        0.5,
    )
    .await;
    let (a, b) = two_passes_then_drain(&fx.db).await;
    drop_sleep(&fx.db, "w13b_slow_fire", "workflow.executions").await;
    eprintln!("phase B: a = {a:?}, b = {b:?}");
    let fires = executions_of(&fx.db, listener).await - before;
    let tries = attempts(&fx.db).await - tried;
    drop_attempt_counter(&fx.db).await;
    assert_eq!(fires, 1, "phase B: one fire");
    assert_eq!(
        tries, 1,
        "phase B: the rows are right, but {tries} fires were attempted -- a double fire \
         the window's FOR UPDATE should have excluded, rolled back by the second DELETE"
    );
    // The loser, retried, opened a fresh window with its own p2.
    let w = window(&fx.db, listener)
        .await
        .expect("the second p2's window");
    assert_eq!(w.results.len(), 1);

    let text = logs.text();
    for line in text.lines().filter(|l| l.contains("the listener's window")) {
        eprintln!("retried: {line}");
    }
    assert!(
        !text.contains("duplicate key"),
        "a missing row was created twice (check-then-insert):\n{text}"
    );
}

/// **Contention is not a fault** (Task 4 fix round 1). A listener whose
/// window row another transaction keeps changing loses the race on every
/// pass -- `40001` at `REPEATABLE READ` -- more times than [`MAX_ATTEMPTS`].
/// The event must survive every loss without spending an attempt or counting
/// `errored`, and the listener must fire exactly once when the contention
/// stops.
///
/// Each round holds the row `FOR UPDATE` on a side connection, starts a
/// pass, waits until that pass is blocked on the lock, then touches the row
/// and commits: the pass's snapshot predates that commit, so its lock wait
/// ends in `40001`.
#[tokio::test]
async fn a_window_contended_for_more_than_max_attempts_passes_still_fires_once() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let listener = fx
        .flow(&listener_yaml(
            "listener",
            "",
            &depends_on(&["p1", "p2"], ""),
        ))
        .await;
    fx.event(&p1).await;
    assert!(window(&fx.db, listener).await.is_some(), "control: {{p1}}");
    fx.run(&p2).await;
    assert_eq!(common::events(&fx.db).await.len(), 1, "control: p2's event");

    let rounds = MAX_ATTEMPTS + 2;
    for round in 1..=rounds {
        let mut side = fx.db.pool.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM workflow.flow_trigger_windows WHERE flow_id = $1 FOR UPDATE")
            .bind(listener)
            .execute(&mut *side)
            .await
            .unwrap();
        let pool = fx.db.pool.clone();
        let pass = tokio::spawn(async move { process_events(&pool).await.expect("a pass") });
        let start = std::time::Instant::now();
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                  WHERE datname = current_database() AND wait_event_type = 'Lock'",
            )
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
            if waiting > 0 {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "round {round}: the pass never waited on the window lock"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sqlx::query(
            "UPDATE workflow.flow_trigger_windows SET results = results WHERE flow_id = $1",
        )
        .bind(listener)
        .execute(&mut *side)
        .await
        .unwrap();
        side.commit().await.unwrap();
        let r = pass.await.unwrap();
        assert_eq!(
            r,
            EventReport {
                events: 1,
                contended: 1,
                ..Default::default()
            },
            "round {round}"
        );
        let rows: Vec<(i32,)> = sqlx::query_as("SELECT attempts FROM workflow.flow_trigger_events")
            .fetch_all(&fx.db.pool)
            .await
            .unwrap();
        assert_eq!(
            rows,
            vec![(0,)],
            "round {round}: the event survives contention, and no attempt is spent"
        );
        assert_eq!(executions_of(&fx.db, listener).await, 0);
    }

    // The contention stops: the next pass reads {p1}, adds p2, fires once.
    let r = fx.pass().await;
    assert_eq!(r.fired, 1, "{r:?}");
    assert_eq!(executions_of(&fx.db, listener).await, 1);
    assert_eq!(window(&fx.db, listener).await, None);
    assert_eq!(common::events(&fx.db).await, vec![]);
}

// ── The lifecycle (D1) ──────────────────────────────────────────────────────

/// D1: an event that finds an EXPIRED window replaces it -- fresh bounds,
/// empty results -- and is then counted. The expired `p1` does not count.
#[tokio::test]
async fn an_expired_window_does_not_count() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let src = listener_yaml("listener", "", &depends_on(&["p1", "p2"], ""));
    let [k1, k2]: [String; 2] = keys_of(&src).try_into().unwrap();
    let listener = fx.flow(&src).await;

    fx.event(&p1).await;
    assert_eq!(
        window(&fx.db, listener).await.unwrap().keys(),
        vec![k1.clone()]
    );
    expire(&fx.db, listener).await;
    let old = window(&fx.db, listener).await.unwrap();

    fx.event(&p2).await;
    assert_eq!(
        executions_of(&fx.db, listener).await,
        0,
        "the expired p1 did not count"
    );
    let w = window(&fx.db, listener)
        .await
        .expect("replaced, not deleted");
    assert_eq!(w.keys(), vec![k2], "fresh results, and the event counted");
    let now = pg_now(&fx.db).await;
    assert!(
        w.end > now && w.start > old.end,
        "fresh bounds: old {old:?}, new {w:?}"
    );

    fx.event(&p1).await;
    assert_eq!(
        executions_of(&fx.db, listener).await,
        1,
        "control: the fresh window completes"
    );
}

/// Spec §5.3 step 3: an event outside a `DAILY_TIME_WINDOW` is ignored and not
/// stored for later. The window is two hours from now's hour (UTC, the default
/// zone), so no test run is inside it. A default-window listener on the same
/// dependency fires on the same event, which shows it was heard.
#[tokio::test]
async fn an_event_outside_a_daily_window_is_ignored() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let hour: f64 =
        sqlx::query_scalar("SELECT extract(hour FROM now() AT TIME ZONE 'UTC')::float8")
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    let h = (hour as u32 + 2) % 24;
    let daily = fx
        .flow(&listener_yaml(
            "daily",
            "",
            &depends_on(
                &["p1"],
                &format!("    window:\n      from: \"{h:02}:00:00\"\n      to: \"{h:02}:30:00\"\n"),
            ),
        ))
        .await;
    let control = fx
        .flow(&listener_yaml("control", "", &depends_on(&["p1"], "")))
        .await;

    let r = fx.event(&p1).await;
    assert_eq!(
        executions_of(&fx.db, control).await,
        1,
        "control fired: {r:?}"
    );
    assert_eq!(executions_of(&fx.db, daily).await, 0, "outside the window");
    let w = window(&fx.db, daily).await;
    assert!(
        w.as_ref().is_none_or(|w| w.results.is_empty()),
        "nothing stored for later: {w:?}"
    );
    assert_eq!(r.refused, 1, "the daily listener is skipped: {r:?}");
}

/// Spec §5.3 step 4: a dependency's `when` renders against the SOURCE's
/// context. `flow.id` is the source's (`p1`) there and the listener's own id
/// anywhere else, so the two listeners below split on exactly that.
#[tokio::test]
async fn a_dependency_when_renders_on_the_source() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let on_source = fx
        .flow(&listener_yaml(
            "on_source",
            "",
            "    dependsOn:\n      - namespace: w13\n        flowId: p1\n        when: \"{{ flow.id == 'p1' }}\"\n",
        ))
        .await;
    let on_listener = fx
        .flow(&listener_yaml(
            "on_listener",
            "",
            "    dependsOn:\n      - namespace: w13\n        flowId: p1\n        when: \"{{ flow.id == 'on_listener' }}\"\n",
        ))
        .await;
    fx.event(&p1).await;
    assert_eq!(executions_of(&fx.db, on_source).await, 1, "rendered on p1");
    assert_eq!(executions_of(&fx.db, on_listener).await, 0);
}

/// W7f spec §3.3: a dependency's `labels:` match the source EXECUTION's labels
/// (the flow's, then its own). `p1` declares none; one execution carries
/// `reason: backfill` and meets the dependency, a plain execution of the same
/// flow does not (control).
#[tokio::test]
async fn a_dependency_matches_the_source_executions_own_labels() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let listener = fx
        .flow(&listener_yaml(
            "on_labelled",
            "",
            "    dependsOn:\n      - namespace: w13\n        flowId: p1\n        labels:\n          reason: backfill\n",
        ))
        .await;
    common::run_with_labels(&fx.db, fx.ws, p1.id, &p1.yaml, None).await;
    fx.pass().await;
    assert_eq!(
        executions_of(&fx.db, listener).await,
        0,
        "control: no own labels, no match"
    );
    common::run_with_labels(
        &fx.db,
        fx.ws,
        p1.id,
        &p1.yaml,
        Some(serde_json::json!([{"key": "reason", "value": "backfill"}])),
    )
    .await;
    fx.pass().await;
    assert_eq!(executions_of(&fx.db, listener).await, 1);
}

/// D1: only an event that matches at least one dependency touches a window.
/// An execution of an unrelated flow -- heard by the listener (same
/// workspace, a listed state) -- opens none.
#[tokio::test]
async fn an_unrelated_execution_does_not_open_a_window() {
    let Some(fx) = fx().await else { return };
    let other = fx.source("other").await;
    let listener = fx
        .flow(&listener_yaml(
            "listener",
            "",
            &depends_on(&["p1", "p2"], ""),
        ))
        .await;
    let r = fx.event(&other).await;
    assert_eq!(
        r,
        EventReport {
            events: 1,
            refused: 1,
            ..Default::default()
        },
        "control: the event was heard"
    );
    assert_eq!(window(&fx.db, listener).await, None);
    assert_eq!(window_count(&fx.db).await, 0);
}

/// The six off switches stop accumulation: a switched-off listener's window,
/// already holding `p1`, is untouched by `p2` -- neither fired, nor merged,
/// nor replaced. A sibling in the same workspace fires.
#[tokio::test]
async fn off_switches_stop_accumulation() {
    let Some(fx) = fx().await else { return };
    let dep = depends_on(&["p1", "p2"], "");
    let p1 = fx.source("p1").await;
    let p2 = fx.source("p2").await;
    let row_disabled = fx.flow(&listener_yaml("row_disabled", "", &dep)).await;
    let deleted = fx.flow(&listener_yaml("deleted", "", &dep)).await;
    let state_disabled = fx.flow(&listener_yaml("state_disabled", "", &dep)).await;
    let yaml_disabled = fx.flow(&listener_yaml("yaml_disabled", "", &dep)).await;
    let trigger_disabled = fx.flow(&listener_yaml("trigger_disabled", "", &dep)).await;
    let sibling = fx.flow(&listener_yaml("sibling", "", &dep)).await;

    // One workspace per workspace-level switch.
    let mut ws_cases = Vec::new();
    for name in ["feature_off", "archived"] {
        let (ws, user) = common::seed_workspace(&fx.db).await;
        let mut srcs = Vec::new();
        for p in ["p1", "p2"] {
            let yaml = source_yaml(p);
            let id = common::seed_flow(&fx.db, ws, user, &yaml, "draft").await;
            srcs.push(Src { id, yaml });
        }
        let listener =
            common::seed_flow(&fx.db, ws, user, &listener_yaml(name, "", &dep), "draft").await;
        ws_cases.push((ws, srcs, listener));
    }

    // p1 everywhere, while everything is on.
    fx.run(&p1).await;
    for (ws, srcs, _) in &ws_cases {
        run_in(&fx.db, *ws, &srcs[0]).await;
    }
    fx.pass().await;
    let off: Vec<Uuid> = [
        row_disabled,
        deleted,
        state_disabled,
        yaml_disabled,
        trigger_disabled,
    ]
    .into_iter()
    .chain(ws_cases.iter().map(|c| c.2))
    .collect();
    let mut held = Vec::new();
    for l in off.iter().copied().chain([sibling]) {
        let w = window(&fx.db, l)
            .await
            .expect("control: p1 opened every window");
        assert_eq!(w.results.len(), 1);
        held.push(w);
    }

    // p2 everywhere, while everything is still on, so every event exists.
    fx.run(&p2).await;
    for (ws, srcs, _) in &ws_cases {
        run_in(&fx.db, *ws, &srcs[1]).await;
    }

    // Then switch them off, before the pass. Raw SQL, so the listener rows survive; the YAML ones
    // edit the stored source only, so the row still says "on" and the
    // processor's own reading of the YAML must stop them.
    let exec = |sql: &'static str, id: Uuid| {
        let pool = fx.db.pool.clone();
        async move {
            sqlx::query(sql).bind(id).execute(&pool).await.unwrap();
        }
    };
    exec(
        "UPDATE workflow.flow_listeners SET disabled = true WHERE flow_id = $1",
        row_disabled,
    )
    .await;
    exec(
        "UPDATE workflow.flows SET deleted_at = now() WHERE id = $1",
        deleted,
    )
    .await;
    exec(
        "UPDATE workflow.flows SET state = 'disabled' WHERE id = $1",
        state_disabled,
    )
    .await;
    exec(
        "UPDATE workflow.flows SET source = replace(source, 'namespace: w13\ntriggers:', \
         'namespace: w13\ndisabled: true\ntriggers:') WHERE id = $1",
        yaml_disabled,
    )
    .await;
    exec(
        "UPDATE workflow.flows SET source = replace(source, '    dependsOn:', \
         '    disabled: true\n    dependsOn:') WHERE id = $1",
        trigger_disabled,
    )
    .await;
    for (flow, needle) in [
        (yaml_disabled, "\ndisabled: true\n"),
        (trigger_disabled, "    disabled: true\n"),
    ] {
        let s: String = sqlx::query_scalar("SELECT source FROM workflow.flows WHERE id = $1")
            .bind(flow)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
        assert!(s.contains(needle), "control: the edit landed:\n{s}");
        egret_workflow::spec::parse(&s).expect("control: the edited source parses");
    }
    common::disable_feature(&fx.db, ws_cases[0].0, "workflows").await;
    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(ws_cases[1].0)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    fx.pass().await;

    assert_eq!(
        executions_of(&fx.db, sibling).await,
        1,
        "control: the sibling fired"
    );
    for (l, before) in off.iter().zip(&held) {
        assert_eq!(executions_of(&fx.db, *l).await, 0, "{l} fired");
        assert_eq!(
            window(&fx.db, *l).await.as_ref(),
            Some(before),
            "{l}'s window was touched"
        );
    }
}

/// `sweep_expired_windows` deletes the windows whose end has passed, and only
/// those.
#[tokio::test]
async fn the_sweep_deletes_only_expired_windows() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let expired = fx
        .flow(&listener_yaml(
            "expired",
            "",
            &depends_on(&["p1", "p2"], ""),
        ))
        .await;
    let live = fx
        .flow(&listener_yaml("live", "", &depends_on(&["p1", "p2"], "")))
        .await;
    fx.event(&p1).await;
    assert!(window(&fx.db, live).await.is_some(), "control");
    expire(&fx.db, expired).await;

    assert_eq!(sweep_expired_windows(&fx.db.pool).await.expect("sweep"), 1);
    assert_eq!(window(&fx.db, expired).await, None);
    assert!(
        window(&fx.db, live).await.is_some(),
        "a live window is kept"
    );
    assert_eq!(sweep_expired_windows(&fx.db.pool).await.expect("sweep"), 0);
    let listeners: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.flow_listeners")
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(listeners, 2, "the sweep deletes windows, not listeners");
}

/// D1: deleting a listener deletes its window (Kestra keeps it), through
/// 0158's `ON DELETE CASCADE` -- whatever deletes the row (`scheduler::sync`
/// when the trigger leaves the flow, or 0157's cascade from the flow row).
#[tokio::test]
async fn deleting_a_listener_cascades_its_window() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let listener = fx
        .flow(&listener_yaml(
            "listener",
            "",
            &depends_on(&["p1", "p2"], ""),
        ))
        .await;
    let kept = fx
        .flow(&listener_yaml("kept", "", &depends_on(&["p1", "p2"], "")))
        .await;
    fx.event(&p1).await;
    assert!(window(&fx.db, listener).await.is_some(), "control");

    sqlx::query("DELETE FROM workflow.flow_listeners WHERE flow_id = $1")
        .bind(listener)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(window(&fx.db, listener).await, None, "cascaded");
    assert!(window(&fx.db, kept).await.is_some(), "only its own");
}

/// Drafts save despite validation errors and still get a listener row, so the
/// processor applies the save-time refusals itself (`depends_on_refusals`,
/// the whole list, and `TimeWindow::from_model`): such a listener never
/// fires, never opens a window, and is skipped with a named reason.
#[tokio::test]
async fn a_draft_with_an_invalid_window_or_dependency_never_fires() {
    let Some(fx) = fx().await else { return };
    let p1 = fx.source("p1").await;
    let cases = [
        (
            "bad_zone",
            depends_on(&["p1"], "    window:\n      timezone: Mars/Olympus\n"),
        ),
        (
            "too_long",
            depends_on(&["p1"], "    window:\n      lookback: P400D\n"),
        ),
        ("no_min", depends_on(&["p1"], "    mode: AT_LEAST\n")),
        (
            "no_flow_id",
            "    dependsOn:\n      - namespace: w13\n      - namespace: w13\n        flowId: p1\n"
                .to_string(),
        ),
        ("duplicate", depends_on(&["p1", "p1"], "")),
    ];
    let mut ids = Vec::new();
    for (name, extra) in &cases {
        let src = listener_yaml(name, "", extra);
        let flow = egret_workflow::spec::parse(&src).unwrap();
        let TriggerKind::Flow(ft) = &flow.triggers[0].kind else {
            unreachable!()
        };
        assert!(
            !egret_workflow::trigger::depends_on_refusals(ft).is_empty(),
            "control: {name} is refused at save"
        );
        ids.push((*name, fx.flow(&src).await));
    }
    let control = fx
        .flow(&listener_yaml("control", "", &depends_on(&["p1"], "")))
        .await;

    let logs = LogCapture::default();
    let r = {
        let _guard = logs.install();
        fx.event(&p1).await
    };
    assert_eq!(
        executions_of(&fx.db, control).await,
        1,
        "control fired: {r:?}"
    );
    assert_eq!(r.refused, cases.len(), "{r:?}");
    let text = logs.text();
    for (name, id) in ids {
        assert_eq!(executions_of(&fx.db, id).await, 0, "{name} fired");
        assert_eq!(window(&fx.db, id).await, None, "{name} opened a window");
        let line = text
            .lines()
            .find(|l| l.contains(&format!("{id}/on_done")))
            .unwrap_or_else(|| panic!("{name} is not named in the log:\n{text}"));
        assert!(
            line.contains("refused at run time as at save"),
            "{name}'s reason: {line}"
        );
    }
}
