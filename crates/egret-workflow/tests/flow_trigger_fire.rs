//! W13a Task 4: `flow_trigger::process_events` drains the outbox `settle`
//! writes and fires every window-free Flow listener that hears an event, one
//! event per transaction. Requires DATABASE_URL.
//!
//! **The tests in this binary run one at a time** (`SERIAL`) and each starts
//! by emptying the outbox, the executions and the listener rows an earlier test
//! left behind. A pass is global -- it processes every event in the database --
//! and the worker `tick` that `common::drive_to` uses claims every runnable run,
//! so without both an `EventReport` would count other tests' events. The
//! binary owns its database (`egret_test_db`: one clone per process), so
//! nothing outside it is touched.
//!
//! Sources are finished through the real worker tick, so every event here was
//! written by the real `settle` hook, never by hand.
//!
//! **Not staged: a source whose `kind` is not normal.** The brief asked for
//! it "by SQL"; `workflow.executions` has no kind column, so every egret
//! execution is normal and the processor passes `kind: None`
//! (`flow_trigger::process`). `evaluate`'s `NotNormalKind` is covered by
//! `trigger::flow`'s own unit tests only.
//!
//! See docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md §4.4-§7.1.

mod common;

use std::sync::{Arc, Mutex as StdMutex};

use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::flow_trigger::{
    process_events, process_events_with, EventReport, PassLimits, MAX_EVENTS_PER_PASS,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";

/// Task 1's measurement, the shape a fired execution's trigger must have.
fn oracle() -> serde_json::Value {
    serde_json::from_str(include_str!("../oracle-exec/w13-flow-trigger.json"))
        .expect("the Task 1 measurement parses")
}

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

/// A listener that fires only on the flow `from`'s executions.
fn listening_to(id: &str, from: &str) -> String {
    listener_yaml(
        id,
        "",
        &format!("    when: \"{{{{ flow.id == '{from}' }}}}\"\n"),
    )
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
    // One statement for the executions, so the NO ACTION foreign key from a
    // fired execution to its source is checked after both are gone.
    for sql in [
        "DELETE FROM workflow.flow_trigger_events",
        "DELETE FROM workflow.executions",
        "DELETE FROM workflow.flow_listeners",
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

    /// Start `flow` and finish it SUCCESS through the real worker.
    async fn run(&self, flow: Uuid, src: &str, inputs: serde_json::Value) -> Uuid {
        run_in(&self.db, self.ws, flow, src, inputs).await
    }

    async fn pass(&self) -> EventReport {
        process_events(&self.db.pool).await.expect("a pass")
    }
}

async fn run_in(db: &Db, ws: Uuid, flow: Uuid, src: &str, inputs: serde_json::Value) -> Uuid {
    let id = egret_workflow::worker::start_execution(&db.pool, ws, Some(flow), src, inputs)
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

/// `(id, done, attempts)` of every event, oldest first.
async fn event_rows(db: &Db) -> Vec<(i64, Vec<String>, i32)> {
    sqlx::query_as("SELECT id, done, attempts FROM workflow.flow_trigger_events ORDER BY id")
        .fetch_all(&db.pool)
        .await
        .unwrap()
}

/// Make every kept event due now, as the next tick would find it once its
/// backoff has passed (final review F2). A retry test calls this between
/// passes; without it the second pass would find nothing due.
async fn make_events_due(db: &Db) {
    sqlx::query("UPDATE workflow.flow_trigger_events SET next_attempt_at = now()")
        .execute(&db.pool)
        .await
        .expect("make the kept events due");
}

fn key(flow: Uuid) -> String {
    format!("{flow}/on_done")
}

/// A `RAISE` on every insert of an execution whose `column` is `flow`, raised
/// `BEFORE INSERT` (`deferred = false`) or at `COMMIT` (`deferred = true`, a
/// deferred constraint trigger). Scoped to one value -- a listener flow, or a
/// source execution -- so a sibling's fire is untouched.
async fn install_fault_on(db: &Db, column: &str, flow: Uuid, deferred: bool) {
    let hex = flow.simple();
    drop_fault(db, flow).await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION public.w13_fail_{hex}() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'w13 injected fault'; END $$"
    )))
    .execute(&db.pool)
    .await
    .unwrap();
    let ddl = if deferred {
        format!(
            "CREATE CONSTRAINT TRIGGER w13_fail_{hex} AFTER INSERT ON workflow.executions \
             DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.{column} = '{flow}') \
             EXECUTE FUNCTION public.w13_fail_{hex}()"
        )
    } else {
        format!(
            "CREATE TRIGGER w13_fail_{hex} BEFORE INSERT ON workflow.executions \
             FOR EACH ROW WHEN (NEW.{column} = '{flow}') EXECUTE FUNCTION public.w13_fail_{hex}()"
        )
    };
    sqlx::query(sqlx::AssertSqlSafe(ddl.clone()))
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn install_fault(db: &Db, flow: Uuid, deferred: bool) {
    install_fault_on(db, "flow_id", flow, deferred).await;
}

async fn drop_fault(db: &Db, flow: Uuid) {
    let hex = flow.simple();
    for ddl in [
        format!("DROP TRIGGER IF EXISTS w13_fail_{hex} ON workflow.executions"),
        format!("DROP FUNCTION IF EXISTS public.w13_fail_{hex}()"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl.clone()))
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// Collects `tracing` output, as `egret-dataapi`'s `tests/serving_auth.rs`
/// does. `set_default` is thread-local and `#[tokio::test]` runs on one
/// thread, so this captures the calling test's events only.
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

// ── A success fires the listener once ──────────────────────────────────────

/// Spec §7.1 row 2. The listener is Task 1's `listener` at revision 2, the
/// source Task 1's `producer`, so the stored trigger can be compared with the
/// measured one key for key: only the values that name THIS run (the source's
/// id and its two dates) and the API-set `system.from` label are substituted, and `flowRevision` -- which egret
/// cannot know -- is the one key removed.
#[tokio::test]
async fn a_success_fires_the_listener_once() {
    let Some(fx) = fx().await else { return };
    let o = oracle();
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener_src = o["flows"]["listener"]["revision_2"]["source"]
        .as_str()
        .unwrap()
        .to_string();
    let listener = fx.flow(&listener_src).await;

    let exec = fx.run(producer, &producer_src, serde_json::json!({})).await;
    assert_eq!(
        common::events(&fx.db).await,
        vec![(exec, "SUCCESS".to_string())],
        "control: the hook wrote one event, for the state the listener hears"
    );

    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            ..Default::default()
        }
    );
    let fired = common::fired_by(&fx.db, exec).await;
    assert_eq!(fired.len(), 1, "{fired:?}");
    let (flow, depth, trigger) = &fired[0];
    assert_eq!(*flow, listener);
    assert_eq!(*depth, 1, "the source's depth (0) plus one");

    let (created, updated): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT created_at, updated_at FROM workflow.executions WHERE id = $1")
            .bind(exec)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    let fmt = |at: chrono::DateTime<chrono::Utc>| {
        at.naive_utc().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    };
    let mut want = o["executions"]["listener_hop1_SUCCESS"]["trigger"].clone();
    let vars = want["variables"].as_object_mut().unwrap();
    vars.remove("flowRevision").expect("measured");
    vars.insert("executionId".into(), exec.to_string().into());
    vars.insert("startDate".into(), fmt(created).into());
    vars.insert("endDate".into(), fmt(updated).into());
    // Kestra's producer was started through its API, which labels it
    // `system.from = api`; egret sets no system label and the producer
    // declares none, so the nearest value is `{}` -- what Kestra itself wrote
    // for a source with no such label (hop 2).
    vars.insert("executionLabels".into(), serde_json::json!({}));
    assert_eq!(trigger, &want);

    let (inputs, event_id, link): (serde_json::Value, Option<i64>, Option<Uuid>) = sqlx::query_as(
        "SELECT inputs, flow_trigger_event_id, trigger_execution_id \
           FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(listener)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(inputs, serde_json::json!({"src": "producer-fired"}));
    assert!(event_id.is_some(), "the fire names the event it consumed");
    assert_eq!(link, Some(exec));

    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "the event is consumed"
    );
    assert_eq!(fx.pass().await, EventReport::default());
    assert_eq!(executions_of(&fx.db, listener).await, 1, "once");
}

/// W7f spec §3.3: the source's `executionLabels` are its EXECUTION's labels
/// (the flow's, then its own), not only the flow's. The producer declares no
/// label; one execution carries `reason: backfill`, and the fired execution's
/// stored `trigger.variables.executionLabels` shows it. Control: a producer
/// execution without own labels fires with `{}`.
///
/// (A Flow trigger's `inputs:` cannot render `trigger.*` -- Task 1 measured
/// that, see the test below -- so the stored trigger record is where the value
/// is observable.)
#[tokio::test]
async fn the_source_executions_own_labels_reach_the_trigger_variables() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    fx.flow(&listening_to("listener", "producer")).await;
    let with = common::run_with_labels(
        &fx.db,
        fx.ws,
        producer,
        &producer_src,
        Some(serde_json::json!([{"key": "reason", "value": "backfill"}])),
    )
    .await;
    fx.pass().await;
    let without = fx.run(producer, &producer_src, serde_json::json!({})).await;
    fx.pass().await;
    let labels_of = |source: Uuid| {
        let pool = fx.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT execution_trigger->'variables'->'executionLabels' \
                   FROM workflow.executions WHERE trigger_execution_id = $1",
            )
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        labels_of(with).await,
        serde_json::json!({"reason": "backfill"})
    );
    assert_eq!(labels_of(without).await, serde_json::json!({}), "control");
}

/// Task 1 measured it (`listener_run1_FAILED_bad_inputs`): `{{
/// trigger.executionId }}` in a Flow trigger's `inputs` fails with "Unable to
/// find `trigger`", and Kestra STILL creates the listener's execution:
/// `CREATED -> FAILED`, no task run, no `inputs`, the full trigger record,
/// `executionDepth` = source + 1. egret does the same. A sibling with literal
/// inputs fires normally, and the event is consumed.
///
/// The knock-on: the born-FAILED execution's own settle writes a `FAILED`
/// event, and a listener on FAILED hears it -- in the SAME pass, because the
/// event commits with the fire and the pass claims every event it has not seen
/// (spec §4.4 point 4).
#[tokio::test]
async fn inputs_that_fail_to_render_create_a_failed_execution() {
    let Some(fx) = fx().await else { return };
    let o = oracle();
    let rev1 = &o["flows"]["listener"]["revision_1"];
    assert_eq!(
        rev1["triggers"][0]["inputs"]["src"], "{{ trigger.executionId }}",
        "control: the measured revision reads `trigger`"
    );
    let measured = &o["executions"]["listener_run1_FAILED_bad_inputs"];
    assert_eq!(measured["state"]["current"], "FAILED", "control");
    assert_eq!(measured["taskRunList"], serde_json::json!([]), "control");
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let reads_trigger = fx
        .flow(&listener_yaml(
            "listener",
            "inputs:\n  - id: src\n    type: STRING\n",
            "    states: [SUCCESS]\n    inputs:\n      src: \"{{ trigger.executionId }}\"\n",
        ))
        .await;
    let literal = fx
        .flow(&listener_yaml(
            "listener2",
            "inputs:\n  - id: src\n    type: STRING\n",
            "    states: [SUCCESS]\n    inputs:\n      src: listener-fired\n",
        ))
        .await;
    let on_failed = fx
        .flow(&listener_yaml(
            "on_failed",
            "",
            "    states: [FAILED]\n    when: \"{{ flow.id == 'listener' }}\"\n",
        ))
        .await;

    let exec = fx.run(producer, &producer_src, serde_json::json!({})).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 2,
            fired: 2,
            born_failed: 1,
            ..Default::default()
        },
        "the producer's SUCCESS: `listener` born FAILED, `listener2` fired; then \
         the born-FAILED execution's FAILED: `on_failed` fired"
    );

    let (id, state, history, inputs, depth, trigger, event_id, link): (
        Uuid,
        String,
        serde_json::Value,
        serde_json::Value,
        i32,
        serde_json::Value,
        Option<i64>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT id, state, history, inputs, execution_depth, execution_trigger, \
                flow_trigger_event_id, trigger_execution_id \
           FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(reads_trigger)
    .fetch_one(&fx.db.pool)
    .await
    .expect("exactly one execution of the listener whose inputs failed");
    assert_eq!(state, "FAILED");
    assert_eq!(history, serde_json::json!(["CREATED", "FAILED"]));
    assert_eq!(
        inputs,
        serde_json::json!({}),
        "no inputs (the column is NOT NULL)"
    );
    assert_eq!(depth, 1, "the source's depth plus one");
    assert_eq!(link, Some(exec));
    assert!(event_id.is_some());
    assert_eq!(trigger["id"], "on_done");
    assert_eq!(trigger["type"], FLOW);
    assert_eq!(trigger["variables"]["executionId"], exec.to_string());
    assert_eq!(trigger["variables"]["state"], "SUCCESS");
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1")
            .bind(id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(runs, 0, "no task run, as `taskRunList: []`");

    assert_eq!(executions_of(&fx.db, literal).await, 1);
    let knock_on = common::fired_by(&fx.db, id).await;
    assert_eq!(
        knock_on
            .iter()
            .map(|(f, d, _)| (*f, *d))
            .collect::<Vec<_>>(),
        vec![(on_failed, 2)],
        "a listener on FAILED hears the born-FAILED execution"
    );
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "every event is consumed"
    );
}

// ── Refusals ────────────────────────────────────────────────────────────────

/// Spec §7.1: same flow, other workspace, a falsy `when`, a `when` that cannot
/// render -- each no execution, and the event consumed. The positive case is
/// the control for all of them and the killer for "render `when` on the
/// listener's context": it fires only because `inputs.x` is the SOURCE's.
#[tokio::test]
async fn refusals() {
    let Some(fx) = fx().await else { return };
    // The source listens to itself: `evaluate`'s SameFlow.
    let src = listener_yaml("src", "inputs:\n  - id: x\n    type: STRING\n", "");
    let source = fx.flow(&src).await;
    let when_false = fx
        .flow(&listener_yaml(
            "when_false",
            "",
            "    when: \"{{ false }}\"\n",
        ))
        .await;
    let when_bad = fx
        .flow(&listener_yaml(
            "when_bad",
            "",
            "    when: \"x{{ missingVar.someField }}\"\n",
        ))
        .await;
    let when_inputs = fx
        .flow(&listener_yaml(
            "when_inputs",
            "",
            "    when: \"{{ inputs.x == 'yes' }}\"\n",
        ))
        .await;
    let (other_ws, other_user) = common::seed_workspace(&fx.db).await;
    let elsewhere = common::seed_flow(
        &fx.db,
        other_ws,
        other_user,
        &listener_yaml("elsewhere", "", ""),
        "draft",
    )
    .await;

    let exec = fx.run(source, &src, serde_json::json!({"x": "yes"})).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            refused: 3,
            ..Default::default()
        },
        "SameFlow, WhenFalsy and WhenUnrenderable refuse; the other workspace \
         is not even heard"
    );
    for (name, flow, want) in [
        ("same flow", source, 1),
        ("falsy when", when_false, 0),
        ("unrenderable when", when_bad, 0),
        ("when over the source's inputs", when_inputs, 1),
        ("other workspace", elsewhere, 0),
    ] {
        assert_eq!(executions_of(&fx.db, flow).await, want, "{name}");
    }
    assert_eq!(
        common::fired_by(&fx.db, exec).await.len(),
        1,
        "only the positive case fired"
    );
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "the event is consumed"
    );
}

/// Final review M-a: the source's workspace is the EXECUTION's, read from its
/// row, not the event's copy of it. They agree for every event `settle`
/// writes; this stages one where they do not -- an event naming this
/// workspace for a source that lives in another -- and the listener here
/// must not hear it (`DifferentWorkspace`).
#[tokio::test]
async fn the_source_workspace_is_read_from_the_execution_not_the_event() {
    let Some(fx) = fx().await else { return };
    let (other_ws, other_user) = common::seed_workspace(&fx.db).await;
    let producer_src = source_yaml("producer");
    let producer = common::seed_flow(&fx.db, other_ws, other_user, &producer_src, "draft").await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    let foreign = run_in(
        &fx.db,
        other_ws,
        producer,
        &producer_src,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "control: no listener there"
    );
    sqlx::query(
        "INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state) \
         VALUES ($1, $2, 'SUCCESS')",
    )
    .bind(foreign)
    .bind(fx.ws)
    .execute(&fx.db.pool)
    .await
    .expect("stage an event whose workspace disagrees with its source's");

    let logs = LogCapture::default();
    let report = {
        let _guard = logs.install();
        fx.pass().await
    };
    assert_eq!(
        report,
        EventReport {
            events: 1,
            refused: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, listener).await, 0);
    assert!(
        logs.text().contains("DifferentWorkspace"),
        "the refusal is the workspace one:\n{}",
        logs.text()
    );
}

// ── Off switches ────────────────────────────────────────────────────────────

/// Spec §6: the six W12 switches, each with an unaffected sibling that fires.
/// The four flow-level switches share one workspace with their sibling; the
/// two workspace-level ones each get a workspace of their own, and the sibling
/// for both is a fourth, untouched workspace.
///
/// `refused: 0` is part of the assertion: an OFF listener is not HEARD, so a
/// clause dropped from the `SELECT` and caught only by `evaluate`'s
/// FlowDisabled/TriggerDisabled backstop still fails here.
#[tokio::test]
async fn every_off_switch() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let deleted = fx.flow(&listener_yaml("deleted", "", "")).await;
    let state_disabled = fx.flow(&listener_yaml("state_disabled", "", "")).await;
    let yaml_disabled = fx
        .flow(&listener_yaml("yaml_disabled", "disabled: true\n", ""))
        .await;
    let trigger_disabled = fx
        .flow(&listener_yaml(
            "trigger_disabled",
            "",
            "    disabled: true\n",
        ))
        .await;
    let sibling = fx.flow(&listener_yaml("sibling", "", "")).await;

    // One workspace per workspace-level switch, and one untouched.
    let mut ws_cases = Vec::new();
    for name in ["feature_off", "archived", "ws_sibling"] {
        let (ws, user) = common::seed_workspace(&fx.db).await;
        let src_flow = common::seed_flow(&fx.db, ws, user, &producer_src, "draft").await;
        let listener =
            common::seed_flow(&fx.db, ws, user, &listener_yaml(name, "", ""), "draft").await;
        ws_cases.push((name, ws, src_flow, listener));
    }

    // Every source finishes while everything is on, so every event exists.
    fx.run(producer, &producer_src, serde_json::json!({})).await;
    for (_, ws, src_flow, _) in &ws_cases {
        run_in(&fx.db, *ws, *src_flow, &producer_src, serde_json::json!({})).await;
    }
    assert_eq!(
        common::events(&fx.db).await.len(),
        4,
        "control: four events"
    );

    // Raw SQL for the two flow-row switches, so the listener rows survive and
    // only the processor's `SELECT` can stop them.
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(deleted)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(state_disabled)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    common::disable_feature(&fx.db, ws_cases[0].1, "workflows").await;
    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(ws_cases[1].1)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.flow_listeners WHERE flow_id = ANY($1)")
            .bind(vec![
                deleted,
                state_disabled,
                yaml_disabled,
                trigger_disabled,
                ws_cases[0].3,
                ws_cases[1].3,
            ])
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        rows, 6,
        "control: every switched-off listener still has its row"
    );

    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 4,
            fired: 2,
            ..Default::default()
        }
    );
    for (name, flow, want) in [
        ("flow deleted", deleted, 0),
        ("flow state disabled", state_disabled, 0),
        ("flow YAML disabled", yaml_disabled, 0),
        ("trigger disabled", trigger_disabled, 0),
        ("the flow-level sibling", sibling, 1),
        ("workflows feature off", ws_cases[0].3, 0),
        ("workspace archived", ws_cases[1].3, 0),
        ("the workspace-level sibling", ws_cases[2].3, 1),
    ] {
        assert_eq!(executions_of(&fx.db, flow).await, want, "{name}");
    }
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "an event heard by nobody is dropped, not queued (spec §6)"
    );
}

// ── Depth ───────────────────────────────────────────────────────────────────

/// A -> B -> C, depth 1 then 2 (spec §4.4 point 4). One hop per pass HERE
/// because B has a task: its SUCCESS needs the worker, between the passes. A
/// hop whose first settle already emits (see
/// `inputs_that_fail_to_render_create_a_failed_execution`) chains within one.
#[tokio::test]
async fn a_chain_runs_one_hop_per_pass() {
    let Some(fx) = fx().await else { return };
    let a_src = source_yaml("a");
    let a = fx.flow(&a_src).await;
    let b = fx.flow(&listening_to("b", "a")).await;
    let c = fx.flow(&listening_to("c", "b")).await;

    let a_exec = fx.run(a, &a_src, serde_json::json!({})).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            refused: 1,
            ..Default::default()
        },
        "pass 1: B fires, C's `when` is false for A"
    );
    let hop1 = common::fired_by(&fx.db, a_exec).await;
    assert_eq!(
        hop1.iter().map(|(f, d, _)| (*f, *d)).collect::<Vec<_>>(),
        vec![(b, 1)]
    );
    assert_eq!(executions_of(&fx.db, c).await, 0, "no hop 2 in pass 1");

    let b_exec: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE flow_id = $1")
        .bind(b)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    common::drive_to(&fx.db, b_exec, "SUCCESS").await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            refused: 1,
            ..Default::default()
        },
        "pass 2: C fires, B hears itself (SameFlow)"
    );
    let hop2 = common::fired_by(&fx.db, b_exec).await;
    assert_eq!(
        hop2.iter().map(|(f, d, _)| (*f, *d)).collect::<Vec<_>>(),
        vec![(c, 2)]
    );
}

/// A <-> B with the source at depth 99: one more fire happens at 100, and the
/// next is refused `DepthExceeded` (Kestra's `DEFAULT_MAX_EXECUTION_DEPTH`;
/// Task 1 measured 100 as the highest depth reached).
#[tokio::test]
async fn a_cycle_stops_at_depth_100() {
    let Some(fx) = fx().await else { return };
    let a_src = listening_to("a", "b");
    let a = fx.flow(&a_src).await;
    let b = fx.flow(&listening_to("b", "a")).await;
    let a_exec = fx.run(a, &a_src, serde_json::json!({})).await;
    sqlx::query("UPDATE workflow.executions SET execution_depth = 99 WHERE id = $1")
        .bind(a_exec)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            refused: 1,
            ..Default::default()
        }
    );
    let hop = common::fired_by(&fx.db, a_exec).await;
    assert_eq!(
        hop.iter().map(|(f, d, _)| (*f, *d)).collect::<Vec<_>>(),
        vec![(b, 100)],
        "depth 100 still fires"
    );

    let b_exec: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE flow_id = $1")
        .bind(b)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    common::drive_to(&fx.db, b_exec, "SUCCESS").await;
    let logs = LogCapture::default();
    let report = {
        let _guard = logs.install();
        fx.pass().await
    };
    assert_eq!(
        report,
        EventReport {
            events: 1,
            refused: 2,
            ..Default::default()
        }
    );
    assert_eq!(
        common::fired_by(&fx.db, b_exec).await,
        vec![],
        "no depth 101"
    );
    assert_eq!(executions_of(&fx.db, a).await, 1, "only the original A");
    assert!(
        logs.text()
            .contains("DepthExceeded { depth: 101, max: 100 }"),
        "the refusal is the depth one:\n{}",
        logs.text()
    );
}

/// Final review I1: a `Loop` iteration is a sub-execution of kind LOOP on the
/// artifact (`Execution.loopExecution` passes `ExecutionKind.LOOP`), and
/// `computeFlowTriggers` answers an empty list for a kind that is not normal
/// (offset 12, `ExecutionKind.isNormal`). So the iteration's SUCCESS is heard
/// and refused, and only the looping execution's own SUCCESS fires. Before
/// this, egret passed `kind: None` for every source, the iteration fired too,
/// and -- the iteration's depth being reset to 0 -- a cycle through a loop
/// never reached 100.
#[tokio::test]
async fn a_loop_iteration_fires_no_listener() {
    let Some(fx) = fx().await else { return };
    let looper_src = "id: looper\nnamespace: w13\ntasks:\n  - id: loop\n    \
                      type: io.kestra.plugin.core.flow.Loop\n    values: '[\"x\"]'\n    \
                      tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        \
                      message: hi\n";
    let looper = fx.flow(looper_src).await;
    let listener = fx.flow(&listening_to("listener", "looper")).await;
    let exec = fx.run(looper, looper_src, serde_json::json!({})).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index = 0",
    )
    .bind(exec)
    .fetch_one(&fx.db.pool)
    .await
    .expect("control: the loop ran one iteration");
    let mut events = common::events(&fx.db).await;
    events.sort();
    let mut want = vec![
        (iteration, "SUCCESS".to_string()),
        (exec, "SUCCESS".to_string()),
    ];
    want.sort();
    assert_eq!(events, want, "control: both transitions reached the outbox");

    let logs = LogCapture::default();
    let report = {
        let _guard = logs.install();
        fx.pass().await
    };
    assert_eq!(
        report,
        EventReport {
            events: 2,
            fired: 1,
            refused: 1,
            ..Default::default()
        }
    );
    assert_eq!(common::fired_by(&fx.db, iteration).await, vec![]);
    assert_eq!(
        common::fired_by(&fx.db, exec)
            .await
            .iter()
            .map(|(f, d, _)| (*f, *d))
            .collect::<Vec<_>>(),
        vec![(listener, 1)]
    );
    assert!(
        logs.text().contains("NotNormalKind"),
        "the refusal is the kind one:\n{}",
        logs.text()
    );
}

// ── A cycle that alternates a Flow trigger with a Subflow ──────────────────

/// **`subflow::chain_holds` must catch a repeat reached through a Flow-trigger
/// link, not only through a Subflow one.**
///
/// The walk is `COALESCE(trigger_execution_id, parent_execution_id)`, and a
/// Flow-trigger fire populates the SAME `trigger_execution_id` column a
/// `Subflow` spawn does (`trigger_run_ix` is the only thing that tells them
/// apart -- see `trigger_of`'s doc comment). `tests/worker.rs`'s
/// `a_cycle_of_subflows_stops_instead_of_crashing` and
/// `a_cycle_whose_every_hop_crosses_a_loop_boundary_stops_too` prove the walk
/// crosses a Subflow link and a Loop's `parent_execution_id` link respectively;
/// neither ever manufactures a genuine Flow-trigger link in the chain a
/// refusal has to cross, so that half of `COALESCE` was unexercised.
///
/// `b` has one task, a `Subflow` naming `a`, always started with
/// `close_loop: "false"`. `a` carries the Flow trigger `on_done` (`when:
/// flow.id == 'b'`), which fires with `close_loop: "true"`, and one task, an
/// `If` gated on that input: `"false"` takes a no-op `Log`, `"true"` takes a
/// `Subflow` back to `b`.
///
/// So starting `b` reaches the interesting branch exactly once. `b`'s direct
/// Subflow child (`close_loop: "false"`) takes the no-op and never calls
/// `chain_holds`. It is `b`'s own SUCCESS the trigger hears; firing it links
/// the new `a` execution to `b` by `trigger_execution_id` ALONE -- a
/// Flow-trigger link, with no Subflow anywhere between them.
///
/// **The whole thing resolves inside one `process_events` pass, not two.**
/// Firing an execution drives its first settle inline, the same way
/// `spawn_one` does for a `Subflow` child (spec §4.4 point 4): the
/// Flow-trigger-fired execution's `If` takes the `Subflow` branch, `back`
/// hits `chain_holds` and is refused, and `transmitFailed` carries that to
/// the execution itself -- all before the pass returns. Its own FAILED settle
/// then queues a THIRD event, which the SAME pass claims and refuses too
/// (`on_done` cannot hear its own flow's executions, `SameFlow`) -- exactly
/// the chaining `inputs_that_fail_to_render_create_a_failed_execution`
/// documents, met here from the Subflow side instead of a bad render.
#[tokio::test]
async fn a_cycle_through_a_flow_trigger_and_a_subflow_stops_at_the_first_repeat() {
    let Some(fx) = fx().await else { return };
    let b_src = "id: b\nnamespace: w13\ntasks:\n  - id: sub\n    \
                 type: io.kestra.plugin.core.flow.Subflow\n    namespace: w13\n    \
                 flowId: a\n    inputs:\n      close_loop: \"false\"\n";
    let a_src = format!(
        "id: a\nnamespace: w13\ninputs:\n  - id: close_loop\n    type: STRING\n\
         triggers:\n  - id: on_done\n    type: {FLOW}\n    \
         when: \"{{{{ flow.id == 'b' }}}}\"\n    inputs:\n      close_loop: \"true\"\n\
         tasks:\n  - id: gate\n    type: io.kestra.plugin.core.flow.If\n    \
         condition: \"{{{{ inputs.close_loop == 'true' }}}}\"\n    then:\n      \
         - id: back\n        type: io.kestra.plugin.core.flow.Subflow\n        \
         namespace: w13\n        flowId: b\n    else:\n      \
         - id: noop\n        type: io.kestra.plugin.core.log.Log\n        message: leaf\n"
    );
    egret_workflow::spec::parse(&a_src)
        .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{a_src}"));
    let b = fx.flow(b_src).await;
    let a = fx.flow(&a_src).await;

    let exec_b = fx.run(b, b_src, serde_json::json!({})).await;
    assert_eq!(
        executions_of(&fx.db, a).await,
        1,
        "control: b's direct Subflow child"
    );
    let direct_child: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE flow_id = $1 AND trigger_execution_id = $2",
    )
    .bind(a)
    .bind(exec_b)
    .fetch_one(&fx.db.pool)
    .await
    .expect("the direct Subflow child");
    let (direct_state, direct_depth): (String, i32) =
        sqlx::query_as("SELECT state, execution_depth FROM workflow.executions WHERE id = $1")
            .bind(direct_child)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        (direct_state.as_str(), direct_depth),
        ("SUCCESS", 1),
        "control: the direct child took the no-op branch and never called chain_holds"
    );

    // events: direct_child's own SUCCESS (SameFlow-refused -- `on_done`
    // belongs to `a` too), b's SUCCESS (fired), and the Flow-trigger-fired
    // execution's own FAILED settle, queued and claimed within this same
    // pass (SameFlow-refused again).
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 3,
            fired: 1,
            refused: 2,
            ..Default::default()
        },
        "b's own SUCCESS fires a's Flow trigger, which resolves to FAILED inline"
    );
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "every event this pass touched is consumed"
    );
    let triggered: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE flow_id = $1 AND trigger_execution_id = $2 \
           AND id != $3",
    )
    .bind(a)
    .bind(exec_b)
    .bind(direct_child)
    .fetch_one(&fx.db.pool)
    .await
    .expect("the Flow-trigger-fired execution");

    let (state, depth): (String, i32) =
        sqlx::query_as("SELECT state, execution_depth FROM workflow.executions WHERE id = $1")
            .bind(triggered)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), depth),
        ("FAILED", 1),
        "the Flow-trigger-fired execution resolved to FAILED already, inside the pass"
    );
    let message: Option<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'back'",
    )
    .bind(triggered)
    .fetch_one(&fx.db.pool)
    .await
    .expect("the refused Subflow hop's own task run");
    assert_eq!(
        message.as_deref(),
        Some(
            "subflow 'w13'.'b' is already running higher up this execution's \
             own chain; starting it would loop"
        ),
        "the first Subflow hop whose target is already upstream refuses, crossing \
         the Flow-trigger link that put 'b' there"
    );

    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(
        all, 3,
        "b, its direct Subflow child, and the Flow-trigger-fired execution -- \
         the refused hop spawns nothing"
    );
    let max_depth: i32 = sqlx::query_scalar("SELECT max(execution_depth) FROM workflow.executions")
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert!(
        max_depth <= 2,
        "no execution went past depth 2: {max_depth}"
    );
}

// ── Failure isolation ───────────────────────────────────────────────────────

/// A listener whose YAML no longer parses (errored), and one whose fire the
/// unique index refuses (refused) -- the second fails INSIDE Postgres, which
/// is what makes the savepoint load-bearing -- both sort before a healthy
/// sibling. The sibling fires and the event is consumed.
#[tokio::test]
async fn a_broken_listener_does_not_stop_its_siblings() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let broken = fx.flow(&listener_yaml("a_broken", "", "")).await;
    let already = fx.flow(&listener_yaml("b_already", "", "")).await;
    let sibling = fx.flow(&listener_yaml("z_sibling", "", "")).await;

    fx.run(producer, &producer_src, serde_json::json!({})).await;
    let ev: i64 = sqlx::query_scalar("SELECT id FROM workflow.flow_trigger_events")
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow.flows SET source = 'id: a_broken\nnamespace: w13\ntasks: [' WHERE id = $1",
    )
    .bind(broken)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            execution_trigger, flow_trigger_event_id) \
         VALUES ($1, $2, 'w13', 'b_already', 'x', 'SUCCESS', '[\"SUCCESS\"]', \
                 jsonb_build_object('id', 'on_done', 'type', $3::text, 'variables', '{}'::jsonb), $4)",
    )
    .bind(fx.ws)
    .bind(already)
    .bind(FLOW)
    .bind(ev)
    .execute(&fx.db.pool)
    .await
    .expect("stage an earlier fire of this (event, listener)");

    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            refused: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, sibling).await, 1);
    assert_eq!(executions_of(&fx.db, broken).await, 0);
    assert_eq!(
        executions_of(&fx.db, already).await,
        1,
        "only the staged one"
    );
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "the event is consumed"
    );
}

/// Spec §4.4 point 1: retries are per listener. A `BEFORE INSERT` fault on
/// one listener's executions rolls back ITS savepoint; the healthy sibling's
/// fire commits in pass 1 and is not repeated in pass 2.
#[tokio::test]
async fn a_transient_failure_keeps_the_event_for_that_listener_only() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let failing = fx.flow(&listener_yaml("failing", "", "")).await;
    let healthy = fx.flow(&listener_yaml("healthy", "", "")).await;
    fx.run(producer, &producer_src, serde_json::json!({})).await;

    install_fault(&fx.db, failing, false).await;
    let first = fx.pass().await;
    drop_fault(&fx.db, failing).await;
    assert_eq!(
        first,
        EventReport {
            events: 1,
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, healthy).await, 1, "the sibling fired");
    assert_eq!(executions_of(&fx.db, failing).await, 0);
    let rows = event_rows(&fx.db).await;
    assert_eq!(rows.len(), 1, "the event is kept");
    assert_eq!((rows[0].1.clone(), rows[0].2), (vec![key(healthy)], 1));

    make_events_due(&fx.db).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            ..Default::default()
        },
        "pass 2 evaluates only the listener not yet done"
    );
    assert_eq!(executions_of(&fx.db, failing).await, 1);
    assert_eq!(executions_of(&fx.db, healthy).await, 1, "not fired twice");
    assert_eq!(common::events(&fx.db).await, vec![]);
}

/// Final review F2: `attempts` counts TICKS, not passes. A kept event is
/// given a `next_attempt_at` one backoff ahead, and the claim skips an event
/// that is not due -- so a second pass straight after the first (another
/// replica's, or the same ticker's next one inside the backoff) finds nothing,
/// and the event's `attempts` does not move. Once it is due, it is claimed.
#[tokio::test]
async fn a_kept_event_waits_for_its_backoff() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let failing = fx.flow(&listener_yaml("failing", "", "")).await;
    fx.run(producer, &producer_src, serde_json::json!({})).await;
    install_fault(&fx.db, failing, false).await;

    let errored = EventReport {
        events: 1,
        errored: 1,
        ..Default::default()
    };
    assert_eq!(fx.pass().await, errored, "control: pass 1 keeps the event");
    assert_eq!(
        fx.pass().await,
        EventReport::default(),
        "a pass inside the backoff claims nothing"
    );
    make_events_due(&fx.db).await;
    let third_after_due = fx.pass().await;
    drop_fault(&fx.db, failing).await;
    assert_eq!(third_after_due, errored, "a due event is claimed again");
    let rows = event_rows(&fx.db).await;
    assert_eq!(
        rows.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![2],
        "two ticks, two attempts -- not three passes"
    );
    let ahead: bool =
        sqlx::query_scalar("SELECT next_attempt_at > now() FROM workflow.flow_trigger_events")
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(ahead, "the kept event is due one backoff from now");
}

/// A listener that fails on every pass: the event is kept for passes 1-4 and
/// deleted on the 5th, with a `warn` naming the listener given up.
#[tokio::test]
async fn a_listener_that_keeps_failing_is_given_up_after_five_attempts() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let failing = fx.flow(&listener_yaml("failing", "", "")).await;
    fx.run(producer, &producer_src, serde_json::json!({})).await;
    install_fault(&fx.db, failing, false).await;

    let errored = EventReport {
        events: 1,
        errored: 1,
        ..Default::default()
    };
    for attempt in 1..=4 {
        make_events_due(&fx.db).await;
        assert_eq!(fx.pass().await, errored, "pass {attempt}");
        let rows = event_rows(&fx.db).await;
        assert_eq!(
            rows.iter().map(|r| r.2).collect::<Vec<_>>(),
            vec![attempt],
            "kept after pass {attempt}"
        );
    }
    make_events_due(&fx.db).await;
    let logs = LogCapture::default();
    let fifth = {
        let _guard = logs.install();
        fx.pass().await
    };
    drop_fault(&fx.db, failing).await;
    assert_eq!(fifth, errored, "pass 5");
    assert_eq!(common::events(&fx.db).await, vec![], "given up: deleted");
    let text = logs.text();
    assert!(
        text.contains("WARN") && text.contains("gave up") && text.contains(&key(failing)),
        "a warn naming the listener given up:\n{text}"
    );
    assert_eq!(fx.pass().await, EventReport::default());
    assert_eq!(executions_of(&fx.db, failing).await, 0);
}

/// Spec §4.4 point 1: the fire and the event's delete commit together. A
/// fault raised at COMMIT -- after the fire and after the delete -- rolls back
/// both: no execution, and the event still there, with nothing done (only its
/// `attempts`, counted apart since final review F1, moves).
///
/// Fix round 1 (M2): that failure does not end the pass. Two sources, the
/// fault scoped to the FIRST one's fires: the older event rolls back, and the
/// newer event behind it still fires in the same pass.
#[tokio::test]
async fn fire_and_delete_are_atomic() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    let first = fx.run(producer, &producer_src, serde_json::json!({})).await;
    let second = fx.run(producer, &producer_src, serde_json::json!({})).await;
    let before = event_rows(&fx.db).await;
    assert_eq!(before.len(), 2);

    install_fault_on(&fx.db, "trigger_execution_id", first, true).await;
    let logs = LogCapture::default();
    let report = {
        let _guard = logs.install();
        fx.pass().await
    };
    drop_fault(&fx.db, first).await;
    assert_eq!(
        report,
        EventReport {
            events: 2,
            fired: 1,
            errored: 1,
            ..Default::default()
        },
        "the failed event counts once as errored, and its fire is not counted"
    );
    assert!(
        logs.text().contains("w13 injected fault"),
        "the COMMIT's error is logged:\n{}",
        logs.text()
    );
    assert_eq!(
        common::fired_by(&fx.db, first).await,
        vec![],
        "the fire rolled back"
    );
    assert_eq!(
        common::fired_by(&fx.db, second).await.len(),
        1,
        "the newer event was not blocked"
    );
    let (id, done, attempts) = before[0].clone();
    assert_eq!(
        event_rows(&fx.db).await,
        vec![(id, done, attempts + 1)],
        "only the failed event remains, its fires undone and nothing marked \
         done; its attempt is counted outside the rolled-back transaction (F1)"
    );

    make_events_due(&fx.db).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            ..Default::default()
        },
        "control: with the fault gone the same event fires"
    );
    assert_eq!(executions_of(&fx.db, listener).await, 2);
}

// ── A busy source (final review I2) ─────────────────────────────────────────

/// A source that a worker settles AFTER the claim's snapshot and BEFORE the
/// fire's foreign-key check must not fail the fire.
///
/// The fire's `INSERT` checks `trigger_execution_id` with `FOR KEY SHARE` on
/// the source. At `REPEATABLE READ` that check fails `40001` when the source
/// row was changed after the snapshot by a transaction that held it
/// `FOR UPDATE` (the reviewer measured it on PG16; a plain `UPDATE` or one
/// under `FOR NO KEY UPDATE` passes). Every `settle` loads its execution
/// through `claim::load_with`, so the source's lock mode is the one under test.
///
/// The window is held open by a `BEFORE INSERT` trigger on the listener's
/// executions that counts itself into a sequence and sleeps: the pass has
/// claimed and read the source, and its FK check has not run yet. The test
/// waits for the count, then `worker::resume`s the source -- the worker's own
/// `load` + `settle`, which rewrites the row's `seq` -- and commits it.
#[tokio::test]
async fn a_source_settled_between_the_claim_and_the_fire_still_fires() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    let exec = fx.run(producer, &producer_src, serde_json::json!({})).await;

    let hex = listener.simple();
    for ddl in [
        format!("DROP TRIGGER IF EXISTS w13_hold_{hex} ON workflow.executions"),
        format!("DROP FUNCTION IF EXISTS public.w13_hold_{hex}()"),
        format!("DROP SEQUENCE IF EXISTS public.w13_hold_{hex}"),
        format!("CREATE SEQUENCE public.w13_hold_{hex}"),
        format!(
            "CREATE FUNCTION public.w13_hold_{hex}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN PERFORM nextval('public.w13_hold_{hex}'); PERFORM pg_sleep(1.5); \
             RETURN NEW; END $$"
        ),
        format!(
            "CREATE TRIGGER w13_hold_{hex} BEFORE INSERT ON workflow.executions \
             FOR EACH ROW WHEN (NEW.flow_id = '{listener}') \
             EXECUTE FUNCTION public.w13_hold_{hex}()"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl.clone()))
            .execute(&fx.db.pool)
            .await
            .unwrap_or_else(|e| panic!("{ddl}: {e}"));
    }

    let seq_before: i64 = sqlx::query_scalar("SELECT seq FROM workflow.executions WHERE id = $1")
        .bind(exec)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    let pool = fx.db.pool.clone();
    let pass = tokio::spawn(async move { process_events(&pool).await });
    let entered = format!("SELECT is_called FROM public.w13_hold_{hex}");
    let mut inside = false;
    for _ in 0..200 {
        let called: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(entered.clone()))
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
        if called {
            inside = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(inside, "control: the pass reached the listener's insert");
    egret_workflow::worker::resume(&fx.db.pool, exec)
        .await
        .expect("the worker settles the source while the fire waits");
    let seq_after: i64 = sqlx::query_scalar("SELECT seq FROM workflow.executions WHERE id = $1")
        .bind(exec)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert!(
        seq_after > seq_before,
        "control: the settle committed a new version of the source row \
         ({seq_before} -> {seq_after}) while the pass was inside the fire"
    );

    let logs = LogCapture::default();
    let report = {
        let _guard = logs.install();
        pass.await.expect("the pass task").expect("a pass")
    };
    for ddl in [
        format!("DROP TRIGGER IF EXISTS w13_hold_{hex} ON workflow.executions"),
        format!("DROP FUNCTION IF EXISTS public.w13_hold_{hex}()"),
        format!("DROP SEQUENCE IF EXISTS public.w13_hold_{hex}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl.clone()))
            .execute(&fx.db.pool)
            .await
            .unwrap();
    }
    assert_eq!(
        report,
        EventReport {
            events: 1,
            fired: 1,
            ..Default::default()
        },
        "the fire survived the source's settle; logs:\n{}",
        logs.text()
    );
    assert_eq!(executions_of(&fx.db, listener).await, 1);
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "the event is consumed"
    );
}

/// Final review F1: an event whose whole TRANSACTION fails every time -- here
/// a fault at `COMMIT`, after the fire and the event's own write -- used to be
/// retried for ever, because the `attempts + 1` rolled back with it. The
/// failure path now counts the attempt in a statement of its own, outside the
/// rolled-back transaction, and gives the event up at `MAX_ATTEMPTS` with the
/// same `warn` as a listener that keeps failing.
#[tokio::test]
async fn an_event_whose_transaction_always_fails_is_given_up() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    let exec = fx.run(producer, &producer_src, serde_json::json!({})).await;
    install_fault_on(&fx.db, "trigger_execution_id", exec, true).await;

    let errored = EventReport {
        events: 1,
        errored: 1,
        ..Default::default()
    };
    for attempt in 1..=4 {
        make_events_due(&fx.db).await;
        assert_eq!(fx.pass().await, errored, "pass {attempt}");
        let rows = event_rows(&fx.db).await;
        assert_eq!(
            rows.iter().map(|r| (r.1.clone(), r.2)).collect::<Vec<_>>(),
            vec![(Vec::<String>::new(), attempt)],
            "kept after pass {attempt}, its attempt counted, nothing done"
        );
    }
    make_events_due(&fx.db).await;
    let logs = LogCapture::default();
    let fifth = {
        let _guard = logs.install();
        fx.pass().await
    };
    drop_fault(&fx.db, exec).await;
    assert_eq!(fifth, errored, "pass 5");
    assert_eq!(common::events(&fx.db).await, vec![], "given up: deleted");
    let text = logs.text();
    assert!(
        text.contains("WARN") && text.contains("gave up"),
        "a warn saying the event was given up:\n{text}"
    );
    assert_eq!(executions_of(&fx.db, listener).await, 0);
}

// ── A bounded pass (final review I3) ────────────────────────────────────────

/// A pass stops at `MAX_PASS_DURATION` (1 s) and leaves the rest to the next
/// one. Three events whose fires are each held ~0.6 s: the pass that has
/// spent 1.2 s after its second event stops there, and the next pass takes the
/// third. Before the cap, one pass took all three, however long they took.
#[tokio::test]
async fn a_pass_stops_at_its_time_cap_and_the_next_pass_goes_on() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    for _ in 0..3 {
        fx.run(producer, &producer_src, serde_json::json!({})).await;
    }
    let hex = listener.simple();
    let ddl = [
        format!(
            "CREATE FUNCTION public.w13_slow_{hex}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN PERFORM pg_sleep(0.6); RETURN NEW; END $$"
        ),
        format!(
            "CREATE TRIGGER w13_slow_{hex} BEFORE INSERT ON workflow.executions \
             FOR EACH ROW WHEN (NEW.flow_id = '{listener}') EXECUTE FUNCTION public.w13_slow_{hex}()"
        ),
    ];
    for d in ddl {
        sqlx::query(sqlx::AssertSqlSafe(d))
            .execute(&fx.db.pool)
            .await
            .unwrap();
    }
    let first = fx.pass().await;
    let second = fx.pass().await;
    for d in [
        format!("DROP TRIGGER w13_slow_{hex} ON workflow.executions"),
        format!("DROP FUNCTION public.w13_slow_{hex}()"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(d))
            .execute(&fx.db.pool)
            .await
            .unwrap();
    }
    assert_eq!(
        (first.events, first.fired),
        (2, 2),
        "the first pass stops once it has run past its time cap"
    );
    assert_eq!(
        (second.events, second.fired),
        (1, 1),
        "the next pass goes on"
    );
    assert_eq!(executions_of(&fx.db, listener).await, 3);
    assert_eq!(common::events(&fx.db).await, vec![]);
}

/// The count cap: `max_events` per pass, the rest to the next pass, oldest
/// first. Five events drain as 2, 2, 1, then nothing.
#[tokio::test]
async fn a_pass_stops_at_its_event_cap_and_the_next_pass_goes_on() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    for _ in 0..5 {
        fx.run(producer, &producer_src, serde_json::json!({})).await;
    }
    let limits = PassLimits {
        max_events: 2,
        ..Default::default()
    };
    let mut drained = Vec::new();
    for _ in 0..4 {
        let r = process_events_with(&fx.db.pool, &limits, &CancellationToken::new())
            .await
            .expect("a pass");
        drained.push((r.events, r.fired));
    }
    assert_eq!(drained, vec![(2, 2), (2, 2), (1, 1), (0, 0)]);
    assert_eq!(executions_of(&fx.db, listener).await, 5);
    assert_eq!(
        PassLimits::default().max_events,
        MAX_EVENTS_PER_PASS,
        "the runtime's default is the named cap"
    );
    assert_eq!(MAX_EVENTS_PER_PASS, 500);
}

/// Shutdown is checked between events: a pass whose token is already
/// cancelled claims nothing and leaves the outbox as it was.
#[tokio::test]
async fn a_cancelled_pass_claims_nothing() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    let exec = fx.run(producer, &producer_src, serde_json::json!({})).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let report = process_events_with(&fx.db.pool, &PassLimits::default(), &cancel)
        .await
        .expect("a pass");
    assert_eq!(report, EventReport::default());
    assert_eq!(
        common::events(&fx.db).await,
        vec![(exec, "SUCCESS".to_string())]
    );
    assert_eq!(
        fx.pass().await.fired,
        1,
        "control: an uncancelled pass fires it"
    );
    assert_eq!(executions_of(&fx.db, listener).await, 1);
}

// ── Draft ──────────────────────────────────────────────────────────────────

/// The user's decision (spec header): a draft listener fires, beside a live
/// one.
#[tokio::test]
async fn a_draft_listener_fires() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let draft = fx.flow(&listener_yaml("draft", "", "")).await;
    let live = common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &listener_yaml("live", "", ""),
        "live",
    )
    .await;
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.flows WHERE id = $1")
        .bind(draft)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "draft", "control");
    fx.run(producer, &producer_src, serde_json::json!({})).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 2,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, draft).await, 1);
    assert_eq!(executions_of(&fx.db, live).await, 1);
}

/// **The 2026-10-12 ruling, on a Flow trigger.** A listener holding a
/// `ForEach` no longer plans, so the event does not start it (counted
/// `errored`), while its sibling listener, the same flow with `Loop`, fires.
#[tokio::test]
async fn a_foreach_listener_is_refused_and_its_loop_sibling_fires() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = |id: &str, task: &str| {
        format!(
            "id: {id}\nnamespace: w13\ntriggers:\n  - id: on_done\n    type: {FLOW}\ntasks:\n{task}"
        )
    };
    let broken = fx.flow(&listener("a_foreach", "  - id: fan\n    type: io.kestra.plugin.core.flow.ForEach\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n")).await;
    let sibling = fx.flow(&listener("z_loop", "  - id: fan\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n")).await;

    fx.run(producer, &producer_src, serde_json::json!({})).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
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
}
