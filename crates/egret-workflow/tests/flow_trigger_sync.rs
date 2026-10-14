//! W13a Task 2: `workflow.flow_listeners` is kept in step with a flow's
//! declared `Flow` triggers by `scheduler::sync`, in the same pass that keeps
//! `workflow.triggers` (W12) -- called inside every `FlowRepo` write that can
//! change what fires, plus `reconcile` at scheduler start. Requires
//! DATABASE_URL.
//!
//! Every test goes through `FlowRepo`, never a raw SQL write to
//! `workflow.flows`, for the same reason `tests/scheduler_sync.rs` gives: what
//! is under test is that the WRITERS call `sync`, and a test that bypassed
//! them would pass even if a writer forgot to. `reconcile_fills_listener_rows_for_a_flow_inserted_by_raw_sql`
//! is the one deliberate exception, and it says why in its own name: it
//! exercises `reconcile`'s OWN reason for existing -- a flow saved before this
//! migration existed, or otherwise written outside a `FlowRepo` call.
//!
//! See docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md §4.1-4.2.

mod common;

use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::models::{State, TriggerKind};
use egret_workflow::{CreateFlowInput, FlowRepo, FlowRow, UpdateFlowInput};
use uuid::Uuid;

/// A flow with the given `triggers:` block and flow-level prelude.
fn flow_yaml(prelude: &str, triggers: &str, message: &str) -> String {
    format!(
        "id: f\nnamespace: t\n{prelude}triggers:\n{triggers}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: {message}\n"
    )
}

/// `State`'s own Kestra spelling (`"SUCCESS"`, `"PAUSED"`, ...) -- the same
/// serialisation `scheduler::sync`'s private `state_wire_name` uses, so the
/// test's expectations cannot silently diverge from what the enum actually
/// produces.
fn state_names(states: &[State]) -> Vec<String> {
    states
        .iter()
        .map(|s| match serde_json::to_value(s).unwrap() {
            serde_json::Value::String(name) => name,
            other => panic!("State must serialise to a string, got {other:?}"),
        })
        .collect()
}

type ListenerRow = (Vec<String>, bool, bool); // (states, depends_on, disabled)

async fn listener_row(db: &Db, flow: Uuid, trig: &str) -> Option<ListenerRow> {
    sqlx::query_as(
        "SELECT states, depends_on, disabled FROM workflow.flow_listeners \
         WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn listener_ids(db: &Db, flow: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT trigger_id FROM workflow.flow_listeners WHERE flow_id = $1 ORDER BY trigger_id",
    )
    .bind(flow)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// Parsing is asserted before every write, so that "no row" can never be the
/// silent consequence of a YAML typo (`sync` treats an unparseable source as
/// declaring nothing).
fn parses(src: &str) -> egret_workflow::models::Flow {
    egret_workflow::spec::parse(src).unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"))
}

struct Fx {
    db: Db,
    repo: FlowRepo,
    ws: Uuid,
    user: UserId,
}

async fn fx() -> Option<Fx> {
    let db = common::db().await?;
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = FlowRepo::new(db.clone());
    Some(Fx { db, repo, ws, user })
}

impl Fx {
    async fn create(&self, src: &str) -> FlowRow {
        parses(src);
        self.repo
            .create(
                self.ws,
                self.user,
                &CreateFlowInput {
                    namespace: "t".into(),
                    flow_id: "f".into(),
                    source: src.into(),
                    plan: serde_json::json!({}),
                    folder_id: None,
                },
            )
            .await
            .expect("create")
    }

    async fn update(&self, id: Uuid, src: &str) -> FlowRow {
        parses(src);
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
            .expect("update")
    }
}

#[tokio::test]
async fn a_flow_trigger_gets_one_listener_row_with_its_states() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS, FAILED]\n",
        "hi",
    );
    let flow = fx.create(&src).await;

    let (states, depends_on, disabled) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("a listener row");
    assert_eq!(states, vec!["SUCCESS".to_string(), "FAILED".to_string()]);
    assert!(!depends_on);
    assert!(!disabled);
}

#[tokio::test]
async fn default_states_are_the_terminated_states_and_paused() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n",
        "hi",
    );
    // Control: derive the expectation from the parsed model's own default,
    // never hand-write the list -- if `default_flow_trigger_states()` ever
    // changes, this test's expectation moves with it instead of silently
    // diverging.
    let parsed = parses(&src);
    let TriggerKind::Flow(spec) = &parsed.triggers[0].kind else {
        panic!("control: fixture must declare a Flow trigger");
    };
    assert!(
        spec.states.contains(&State::Paused),
        "control: the parsed default must include PAUSED"
    );
    let expected = state_names(&spec.states);

    let flow = fx.create(&src).await;
    let (states, depends_on, disabled) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("a listener row");
    assert_eq!(states, expected);
    assert!(states.contains(&"PAUSED".to_string()));
    assert!(!depends_on);
    assert!(!disabled);
}

#[tokio::test]
async fn a_depends_on_trigger_gets_a_row_marked_depends_on() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    dependsOn:\n      - namespace: t\n        flowId: other\n",
        "hi",
    );
    let parsed = parses(&src);
    let TriggerKind::Flow(spec) = &parsed.triggers[0].kind else {
        panic!("control: fixture must declare a Flow trigger");
    };
    assert!(
        !spec.depends_on.is_empty(),
        "control: the fixture really declares dependsOn"
    );

    let flow = fx.create(&src).await;
    let (_, depends_on, disabled) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("a row even though resolve() refuses dependsOn");
    assert!(depends_on);
    assert!(!disabled);
}

#[tokio::test]
async fn removing_the_trigger_or_deleting_the_flow_deletes_the_row() {
    let Some(fx) = fx().await else { return };
    let two = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n  - id: on_fail\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [FAILED]\n",
        "hi",
    );
    let flow = fx.create(&two).await;
    assert_eq!(
        listener_ids(&fx.db, flow.id).await,
        vec!["on_done".to_string(), "on_fail".to_string()],
        "control: both triggers had a row before the removal"
    );

    fx.update(
        flow.id,
        &flow_yaml(
            "",
            "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
            "hi",
        ),
    )
    .await;
    assert_eq!(
        listener_ids(&fx.db, flow.id).await,
        vec!["on_done".to_string()],
        "the undeclared trigger's row must go, and only it"
    );

    fx.repo.soft_delete(fx.ws, flow.id).await.expect("delete");
    assert_eq!(
        listener_ids(&fx.db, flow.id).await,
        Vec::<String>::new(),
        "a trashed flow keeps no listener state (the FK cascade does not \
         fire on a soft delete, same as workflow.triggers)"
    );
}

/// **An `update` rewrites `states` and `depends_on` on the row that is already
/// there** -- W13a Task 3, a Task 2 review carry-over. Every other test here
/// either creates a row or deletes one, so the upsert's `DO UPDATE SET states =
/// EXCLUDED.states` and `depends_on = EXCLUDED.depends_on` survived being
/// removed: a first `INSERT` writes both whatever the conflict arm says.
///
/// One trigger id across both saves, so the second write can only reach the
/// row through `ON CONFLICT`; both columns change in the same save, and each
/// is asserted on its own so one surviving arm cannot hide behind the other.
#[tokio::test]
async fn an_update_rewrites_states_and_depends_on_on_the_existing_row() {
    let Some(fx) = fx().await else { return };
    let before = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
        "hi",
    );
    let flow = fx.create(&before).await;
    let (states, depends_on, _) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("control: the row exists before the update");
    assert_eq!(
        states,
        vec!["SUCCESS".to_string()],
        "control: states before"
    );
    assert!(!depends_on, "control: not dependsOn before");

    let after = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [FAILED]\n    \
         dependsOn:\n      - namespace: t\n        flowId: other\n",
        "hi",
    );
    let TriggerKind::Flow(spec) = &parses(&after).triggers[0].kind else {
        panic!("control: the updated fixture still declares a Flow trigger");
    };
    assert!(
        !spec.depends_on.is_empty() && spec.states == vec![State::Failed],
        "control: the updated fixture parses to FAILED + dependsOn"
    );
    fx.update(flow.id, &after).await;

    let (states, depends_on, _) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("the row survives the update under the same trigger id");
    assert_eq!(
        states,
        vec!["FAILED".to_string()],
        "the conflict arm must rewrite `states`"
    );
    assert!(depends_on, "the conflict arm must rewrite `depends_on`");
}

#[tokio::test]
async fn flow_or_trigger_disabled_sets_the_flag() {
    let Some(fx) = fx().await else { return };

    // Flow-level `disabled: true`.
    let src = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
        "hi",
    );
    let flow = fx.create(&src).await;
    let (_, _, disabled) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("control: a row exists before disabling");
    assert!(!disabled, "control: not disabled yet");

    let off = flow_yaml(
        "disabled: true\n",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
        "hi",
    );
    assert!(
        parses(&off).disabled,
        "control: the flow-level switch parsed"
    );
    fx.update(flow.id, &off).await;
    let (_, _, disabled) = listener_row(&fx.db, flow.id, "on_done")
        .await
        .expect("row kept");
    assert!(disabled, "flow-level disabled must set the flag");

    // Trigger-level `disabled: true`, on a second flow, so the flow-level
    // switch cannot be the one setting it.
    let ws2 = common::seed_workspace(&fx.db).await;
    let fx2 = Fx {
        db: fx.db.clone(),
        repo: fx.repo.clone(),
        ws: ws2.0,
        user: ws2.1,
    };
    let trig_off = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n    disabled: true\n",
        "hi",
    );
    let parsed = parses(&trig_off);
    assert!(
        parsed.triggers[0].disabled && !parsed.disabled,
        "control: only the trigger-level switch parsed"
    );
    let flow2 = fx2.create(&trig_off).await;
    let (_, _, disabled) = listener_row(&fx2.db, flow2.id, "on_done")
        .await
        .expect("row kept");
    assert!(disabled, "trigger-level disabled must set the flag too");
}

#[tokio::test]
async fn schedule_and_flow_triggers_coexist_in_one_flow() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml(
        "",
        "  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 2 * * *\"\n  \
         - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
        "hi",
    );
    let parsed = parses(&src);
    assert_eq!(parsed.triggers.len(), 2, "control: both kinds declared");

    let flow = fx.create(&src).await;

    let trig_ids: Vec<String> = sqlx::query_scalar(
        "SELECT trigger_id FROM workflow.triggers WHERE flow_id = $1 ORDER BY trigger_id",
    )
    .bind(flow.id)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(trig_ids, vec!["s".to_string()], "one workflow.triggers row");

    assert_eq!(
        listener_ids(&fx.db, flow.id).await,
        vec!["on_done".to_string()],
        "one workflow.flow_listeners row"
    );
}

#[tokio::test]
async fn reconcile_fills_listener_rows_for_a_flow_inserted_by_raw_sql() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let src = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
        "hi",
    );
    parses(&src);

    // Deliberately bypasses `FlowRepo` -- this is `reconcile`'s own reason
    // for existing: a flow saved before this migration, or otherwise written
    // outside a `FlowRepo::create`/`update` call, never got a `sync` pass.
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan, created_by) \
         VALUES ($1, 't', 'f', $2, '{}', $3) RETURNING id",
    )
    .bind(ws)
    .bind(&src)
    .bind(user.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    assert_eq!(
        listener_ids(&db, id).await,
        Vec::<String>::new(),
        "control: no listener row before reconcile"
    );

    let done = egret_workflow::scheduler::reconcile(&db.pool)
        .await
        .expect("reconcile");
    assert_eq!(done.failed, 0);

    assert_eq!(
        listener_ids(&db, id).await,
        vec!["on_done".to_string()],
        "reconcile must call sync for the raw-SQL-inserted flow"
    );
}

/// W13b Task 2 fix round 1: a draft saves with validation errors, and `sync`
/// builds listener rows from any source that PARSES. A `window` with no
/// `dependsOn` is refused at save (`window_without_depends_on`) but that only
/// blocks publish -- so `WindowFreeFlowTrigger::resolve` must refuse it too,
/// or the draft would get a `depends_on = false` row and fire with its window
/// ignored. The control is the same trigger without the window.
#[tokio::test]
async fn a_window_without_depends_on_gets_no_listener_row() {
    let Some(fx) = fx().await else { return };
    let windowed = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    window:\n      every: P1M\n",
        "hi",
    );
    let flow = fx.create(&windowed).await;
    assert_eq!(
        listener_row(&fx.db, flow.id, "on_done").await,
        None,
        "a window on the window-free arm is refused, not ignored"
    );

    let plain = flow_yaml(
        "",
        "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n",
        "hi",
    );
    fx.update(flow.id, &plain).await;
    assert!(
        listener_row(&fx.db, flow.id, "on_done").await.is_some(),
        "control: without the window the same trigger gets its row"
    );
}

/// Final-review fix wave, I1: the same hole as the window test above, for the
/// `mode`/`minSatisfied` shapes save refuses on a trigger with NO `dependsOn`
/// (`min_satisfied_required`, `min_satisfied_exceeds_conditions` -- n = 0,
/// "could never fire" -- and `min_satisfied_not_positive`). A draft carrying
/// one must get no listener row, or the window-free arm would fire it on
/// every matching execution. The control trigger in the same flow gets its
/// row; the update that drops the `mode` lines gives all four theirs.
#[tokio::test]
async fn a_mode_shape_save_refuses_without_depends_on_gets_no_listener_row() {
    let Some(fx) = fx().await else { return };
    let flow_trigger = "    type: io.kestra.plugin.core.trigger.Flow\n";
    let refused = flow_yaml(
        "",
        &format!(
            "  - id: m_required\n{flow_trigger}    mode: AT_LEAST\n\
             \x20 - id: m_exceeds\n{flow_trigger}    mode: AT_LEAST\n    minSatisfied: 1\n\
             \x20 - id: m_zero\n{flow_trigger}    minSatisfied: 0\n\
             \x20 - id: ok\n{flow_trigger}    mode: ANY\n"
        ),
        "hi",
    );
    let flow = fx.create(&refused).await;
    assert_eq!(
        listener_ids(&fx.db, flow.id).await,
        vec!["ok".to_string()],
        "a mode shape save refuses gets no row on the window-free arm"
    );

    let plain = flow_yaml(
        "",
        &format!(
            "  - id: m_required\n{flow_trigger}\
             \x20 - id: m_exceeds\n{flow_trigger}\
             \x20 - id: m_zero\n{flow_trigger}\
             \x20 - id: ok\n{flow_trigger}    mode: ANY\n"
        ),
        "hi",
    );
    fx.update(flow.id, &plain).await;
    assert_eq!(
        listener_ids(&fx.db, flow.id).await,
        vec![
            "m_exceeds".to_string(),
            "m_required".to_string(),
            "m_zero".to_string(),
            "ok".to_string()
        ],
        "control: without the mode lines the same triggers get their rows"
    );
}

/// W7e Task 2 (spec 2026-10-14 §3.2): a row in `workflow.disabled_triggers`
/// is the third cause of `disabled`, copied by the next save onto the listener
/// row of a window-free and of a `dependsOn` Flow trigger alike. The switch
/// row is staged by SQL (its route arrives in a later task); the flow is
/// written through `FlowRepo` only.
#[tokio::test]
async fn the_runtime_switch_disables_the_listener_row() {
    for (label, trigger) in [
        (
            "window-free",
            "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n",
        ),
        (
            "dependsOn",
            "  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
             dependsOn:\n      - namespace: t\n        flowId: other\n",
        ),
    ] {
        let Some(fx) = fx().await else { return };
        let src = flow_yaml("", trigger, "hi");
        let flow = fx.create(&src).await;
        let (_, depends_on, disabled) = listener_row(&fx.db, flow.id, "on_done")
            .await
            .unwrap_or_else(|| panic!("{label}: control: a row"));
        assert_eq!(depends_on, label == "dependsOn", "{label}: control: the kind of row");
        assert!(!disabled, "{label}: control: no cause, not disabled");

        common::switch_off(&fx.db, flow.id, "on_done").await;
        fx.update(flow.id, &flow_yaml("", trigger, "saved")).await;
        let (_, _, disabled) = listener_row(&fx.db, flow.id, "on_done")
            .await
            .unwrap_or_else(|| panic!("{label}: row kept"));
        assert!(disabled, "{label}: the switch row must disable the listener");

        // Control: the same save without the switch row writes `false`.
        common::switch_on(&fx.db, flow.id, "on_done").await;
        fx.update(flow.id, &src).await;
        let (_, _, disabled) = listener_row(&fx.db, flow.id, "on_done")
            .await
            .unwrap_or_else(|| panic!("{label}: row kept"));
        assert!(!disabled, "{label}: switched back on");
    }
}
