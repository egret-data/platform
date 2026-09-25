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
use egret_workflow::flow_trigger::{process_events, EventReport, DEPENDS_ON_IS_W13B};
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

fn key(flow: Uuid) -> String {
    format!("{flow}/on_done")
}

/// A `RAISE` on every insert of an execution of `flow`, raised `BEFORE
/// INSERT` (`deferred = false`) or at `COMMIT` (`deferred = true`, a
/// deferred constraint trigger). Scoped to one flow, so a sibling's fire is
/// untouched.
async fn install_fault(db: &Db, flow: Uuid, deferred: bool) {
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
             DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.flow_id = '{flow}') \
             EXECUTE FUNCTION public.w13_fail_{hex}()"
        )
    } else {
        format!(
            "CREATE TRIGGER w13_fail_{hex} BEFORE INSERT ON workflow.executions \
             FOR EACH ROW WHEN (NEW.flow_id = '{flow}') EXECUTE FUNCTION public.w13_fail_{hex}()"
        )
    };
    sqlx::query(sqlx::AssertSqlSafe(ddl.clone()))
        .execute(&db.pool)
        .await
        .unwrap();
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

/// Task 1 measured it (`listener_run1_FAILED_bad_inputs`): `{{
/// trigger.executionId }}` in a Flow trigger's `inputs` fails with "Unable to
/// find `trigger`" and the listener does not run. egret refuses the fire; a
/// sibling with literal inputs is unaffected, and the event is consumed.
#[tokio::test]
async fn inputs_referencing_trigger_refuse_the_fire() {
    let Some(fx) = fx().await else { return };
    let o = oracle();
    let rev1 = &o["flows"]["listener"]["revision_1"];
    assert_eq!(
        rev1["triggers"][0]["inputs"]["src"], "{{ trigger.executionId }}",
        "control: the measured revision reads `trigger`"
    );
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

    let exec = fx.run(producer, &producer_src, serde_json::json!({})).await;
    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            refused: 1,
            ..Default::default()
        }
    );
    assert_eq!(executions_of(&fx.db, reads_trigger).await, 0);
    assert_eq!(executions_of(&fx.db, literal).await, 1);
    assert_eq!(common::fired_by(&fx.db, exec).await.len(), 1);
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "the event is consumed"
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

/// A -> B -> C, one hop per pass, depth 1 then 2 (spec §4.4 point 4).
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
        assert_eq!(fx.pass().await, errored, "pass {attempt}");
        let rows = event_rows(&fx.db).await;
        assert_eq!(
            rows.iter().map(|r| r.2).collect::<Vec<_>>(),
            vec![attempt],
            "kept after pass {attempt}"
        );
    }
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
/// both: no execution, and the event still there, untouched.
#[tokio::test]
async fn fire_and_delete_are_atomic() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let listener = fx.flow(&listener_yaml("listener", "", "")).await;
    fx.run(producer, &producer_src, serde_json::json!({})).await;
    let before = event_rows(&fx.db).await;
    assert_eq!(before.len(), 1);

    install_fault(&fx.db, listener, true).await;
    let r = process_events(&fx.db.pool).await;
    drop_fault(&fx.db, listener).await;
    let err = r.expect_err("the COMMIT raised");
    assert!(err.to_string().contains("w13 injected fault"), "{err}");
    assert_eq!(
        executions_of(&fx.db, listener).await,
        0,
        "the fire rolled back"
    );
    assert_eq!(event_rows(&fx.db).await, before, "the event is untouched");

    assert_eq!(
        fx.pass().await,
        EventReport {
            events: 1,
            fired: 1,
            ..Default::default()
        },
        "control: with the fault gone the same event fires"
    );
    assert_eq!(executions_of(&fx.db, listener).await, 1);
}

// ── Draft and dependsOn ─────────────────────────────────────────────────────

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

/// `settle`'s `EXISTS` does not read `depends_on`, so a `dependsOn` listener's
/// event reaches the processor. W13a skips it, counts it `refused`, and the
/// log says W13b owns it.
#[tokio::test]
async fn a_depends_on_listener_is_skipped_with_a_named_reason() {
    let Some(fx) = fx().await else { return };
    let producer_src = source_yaml("producer");
    let producer = fx.flow(&producer_src).await;
    let window = fx
        .flow(&listener_yaml(
            "window",
            "",
            "    dependsOn:\n      - namespace: w13\n        flowId: producer\n",
        ))
        .await;
    let depends_on: bool =
        sqlx::query_scalar("SELECT depends_on FROM workflow.flow_listeners WHERE flow_id = $1")
            .bind(window)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(depends_on, "control: sync marked the row");
    fx.run(producer, &producer_src, serde_json::json!({})).await;
    assert_eq!(
        common::events(&fx.db).await.len(),
        1,
        "control: the event exists"
    );

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
    assert_eq!(executions_of(&fx.db, window).await, 0);
    assert_eq!(common::events(&fx.db).await, vec![]);
    let text = logs.text();
    assert!(DEPENDS_ON_IS_W13B.contains("W13b"));
    assert!(
        text.contains(DEPENDS_ON_IS_W13B) && text.contains(&key(window)),
        "the skip names W13b and the listener:\n{text}"
    );
}
