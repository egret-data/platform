//! No-silent-stall spec §4.3 and §4.4: a flow whose task type this
//! deployment does not serve is reported at every validation report, refused
//! at publish and at every start, fails a `Subflow` that calls it, and is not
//! offered by the flow schema.
//!
//! **Its own binary**, because every test here runs with the task types of a
//! `container_runner = none` deployment installed (`runtime::served`), and a
//! test binary serves exactly one set (`install_served_for_tests`). Every test
//! calls [`fx`] (or [`served_none`]) before anything else, so the `none` set
//! is the first one installed whichever test the harness runs first; the
//! `common` helpers install the runner set only when nothing is installed.
//!
//! The probe is `egret.report.send`: it is in `spec::registry`, so validation
//! accepts it by type, and no deployment has a handler for it (spec D10 of
//! package #4 defers it). It was `egret.sql` until package #4 Task C5 served
//! that type on every deployment.
//!
//! DB-gated; self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set.

mod common;

use axum::http::StatusCode;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::worker::Handlers;
use uuid::Uuid;

/// Ticks and fires are global over the database, so the tests that drive
/// them take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NOT_SERVED: &str = "task_type_not_served";
const NO_HANDLER: &str = "this egret deployment has no handler for `egret.report.send`";
const START_REFUSAL: &str = "the flow cannot run on this deployment";

fn served_none() -> &'static egret_workflow::runtime::Served {
    common::install_served_none()
}

/// A flow `id` whose one task is `egret.report.send`. `extra` is appended at the
/// flow level (a `triggers:` block).
fn unserved_flow(ns: &str, id: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: {ns}\n{extra}tasks:\n  - id: q\n    type: egret.report.send\n    \
         chart: monthly\n"
    )
}

/// The same shape with a task this deployment serves.
fn log_flow(ns: &str, id: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: {ns}\n{extra}tasks:\n  - id: a\n    \
         type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

fn fresh_ns() -> String {
    format!("ns{}", Uuid::new_v4().simple())
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
}

async fn fx() -> Option<Fx> {
    served_none();
    let db = common::db().await?;
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    Some(Fx { db, ws, user })
}

async fn executions_of_ws(db: &Db, ws: Uuid) -> i64 {
    common::execution_count(db, ws).await
}

fn not_served_at<'a>(errors: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    errors
        .as_array()
        .expect("errors is an array")
        .iter()
        .find(|e| e["code"] == NOT_SERVED && e["path"] == path)
}

/// The installed set is the `none` one: no `egret.report.send`, no script
/// type, and `core.log` -- and, since package #4 Task C5, `egret.sql`. Every
/// other test here rests on it.
#[test]
fn this_binary_serves_the_none_set() {
    let s = served_none();
    assert!(!s.serves("egret.report.send"));
    assert!(s.serves("egret.sql"));
    assert!(!s.serves("io.kestra.plugin.scripts.shell.Commands"));
    assert!(s.serves("io.kestra.plugin.core.log.Log"));
}

// ── 1, 2: the three validation reports ──────────────────────────────────────

/// `POST`, `PUT` and `GET /flows/{id}` carry `task_type_not_served` at the
/// task's path, and the draft still saves (spec: drafts save with errors).
#[tokio::test]
async fn a_draft_saves_and_every_report_of_it_carries_task_type_not_served() {
    let Some(fx) = fx().await else { return };
    let app = common::app(&fx.db).await;
    let ns = fresh_ns();

    let created = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows", fx.ws),
        fx.user,
        serde_json::json!({ "source": log_flow(&ns, "f", "") }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = common::json(created).await;
    assert!(
        created["validation"]["errors"]
            .as_array()
            .unwrap()
            .is_empty(),
        "control: a served flow has no error: {created}"
    );
    let id = created["id"].as_str().unwrap().to_string();

    let source = unserved_flow(&ns, "f", "");
    let res = common::put(
        &app,
        &format!("/api/ws/{}/workflows/flows/{id}", fx.ws),
        fx.user,
        serde_json::json!({ "source": source, "folder_id": null }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a draft saves whatever its report says"
    );
    let updated = common::json(res).await;
    let e = not_served_at(&updated["validation"]["errors"], "tasks.q")
        .unwrap_or_else(|| panic!("PUT report: {updated}"));
    assert!(e["message"].as_str().unwrap().contains(NO_HANDLER), "{e}");
    assert_eq!(
        updated["source"],
        source.as_str(),
        "and the draft is the one sent"
    );

    let got = common::json(
        common::get(
            &app,
            &format!("/api/ws/{}/workflows/flows/{id}", fx.ws),
            fx.user,
        )
        .await,
    )
    .await;
    assert!(
        not_served_at(&got["validation"]["errors"], "tasks.q").is_some(),
        "GET report: {got}"
    );
}

/// Both validate routes report it, identically.
#[tokio::test]
async fn both_validate_routes_report_task_type_not_served() {
    let Some(fx) = fx().await else { return };
    let app = common::app(&fx.db).await;
    let ns = fresh_ns();
    let flow = common::seed_flow(&fx.db, fx.ws, fx.user, &log_flow(&ns, "f", ""), "draft").await;
    let source = unserved_flow(&ns, "f", "");

    let by_id = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/{flow}/validate", fx.ws),
        fx.user,
        serde_json::json!({ "source": source }),
    )
    .await;
    let idless = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/validate", fx.ws),
        fx.user,
        serde_json::json!({ "source": source }),
    )
    .await;
    assert_eq!(
        (by_id.status(), idless.status()),
        (StatusCode::OK, StatusCode::OK)
    );
    let (by_id, idless) = (common::json(by_id).await, common::json(idless).await);
    assert_eq!(by_id, idless);
    assert!(
        not_served_at(&idless["errors"], "tasks.q").is_some(),
        "{idless}"
    );
}

// ── 3: publish ──────────────────────────────────────────────────────────────

/// Publish answers 400 with the message, and the flow stays a draft.
#[tokio::test]
async fn publish_refuses_with_the_message() {
    let Some(fx) = fx().await else { return };
    let app = common::app(&fx.db).await;
    let ns = fresh_ns();
    let flow = common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &unserved_flow(&ns, "f", ""),
        "draft",
    )
    .await;

    let res = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/{flow}/publish", fx.ws),
        fx.user,
        serde_json::json!({ "note": "" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains(NO_HANDLER) && body.contains("(tasks.q)"),
        "{body}"
    );

    // The control: a served flow publishes in this same deployment.
    let ok = common::seed_flow(&fx.db, fx.ws, fx.user, &log_flow(&ns, "g", ""), "draft").await;
    let res = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/{ok}/publish", fx.ws),
        fx.user,
        serde_json::json!({ "note": "" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

/// With no runner, a script task is refused by `task_type_not_served` and
/// only by it: publish's step 3 runs `unrunnable` before the script gate
/// (step 3b), so the gate's own "no default container runner" message does
/// not join it.
#[tokio::test]
async fn a_script_task_under_none_is_refused_once_by_the_served_check() {
    let Some(fx) = fx().await else { return };
    common::enable_feature(&fx.db, fx.ws, "workflow_containers").await;
    let app = common::app(&fx.db).await;
    let ns = fresh_ns();
    let src = format!(
        "id: f\nnamespace: {ns}\ntasks:\n  - id: sh\n    \
         type: io.kestra.plugin.scripts.shell.Commands\n    containerImage: alpine:3\n    \
         commands:\n      - 'true'\n"
    );
    let flow = common::seed_flow(&fx.db, fx.ws, fx.user, &src, "draft").await;
    let res = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/{flow}/publish", fx.ws),
        fx.user,
        serde_json::json!({ "note": "" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains("has no container runner (`workflow.container_runner`)")
            && body.contains("(tasks.sh)"),
        "{body}"
    );
    assert!(!body.contains("no default container runner"), "{body}");
}

// ── 4, 5: the API start, the webhook, replay ────────────────────────────────

/// The API start answers 400 and writes no execution row.
#[tokio::test]
async fn the_api_start_refuses_and_writes_no_execution() {
    let Some(fx) = fx().await else { return };
    let app = common::app(&fx.db).await;
    let ns = fresh_ns();
    let flow = common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &unserved_flow(&ns, "f", ""),
        "draft",
    )
    .await;

    let res = common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/{flow}/executions", fx.ws),
        fx.user,
        serde_json::json!({ "inputs": {} }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains(START_REFUSAL) && body.contains(NO_HANDLER),
        "{body}"
    );
    assert_eq!(executions_of_ws(&fx.db, fx.ws).await, 0);
}

/// A webhook fire is refused the same way, and starts nothing.
#[tokio::test]
async fn a_webhook_start_is_refused_the_same_way() {
    let Some(fx) = fx().await else { return };
    let ns = fresh_ns();
    let hook = "triggers:\n  - id: hook\n    type: io.kestra.plugin.core.trigger.Webhook\n    \
                key: the-key\n";
    common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &unserved_flow(&ns, "f", hook),
        "draft",
    )
    .await;
    let app = common::webhook_app(&fx.db).await;

    let res = common::anon(
        &app,
        "POST",
        &format!("/workflows/v1/{}/executions/webhook/{ns}/f/the-key", fx.ws),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains(START_REFUSAL) && body.contains(NO_HANDLER),
        "{body}"
    );
    assert_eq!(executions_of_ws(&fx.db, fx.ws).await, 0);
}

/// A replay of an execution of it (one written before this deployment
/// refused the type) is refused the same way, and starts nothing.
#[tokio::test]
async fn a_replay_of_an_execution_of_it_is_refused_the_same_way() {
    let Some(fx) = fx().await else { return };
    let app = common::app(&fx.db).await;
    let ns = fresh_ns();
    let source = unserved_flow(&ns, "f", "");
    let flow = common::seed_flow(&fx.db, fx.ws, fx.user, &source, "draft").await;
    let done: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history) \
         VALUES ($1, $2, $3, 'f', $4, 'SUCCESS', '[\"CREATED\", \"SUCCESS\"]') RETURNING id",
    )
    .bind(fx.ws)
    .bind(flow)
    .bind(&ns)
    .bind(&source)
    .fetch_one(&fx.db.pool)
    .await
    .expect("an execution written before the refusal");

    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{}/workflows/executions/{done}/replay", fx.ws),
        fx.user,
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains(START_REFUSAL) && body.contains(NO_HANDLER),
        "{body}"
    );
    assert_eq!(
        executions_of_ws(&fx.db, fx.ws).await,
        1,
        "only the seeded one"
    );
}

// ── 6: a schedule fire and a Flow-trigger fire ──────────────────────────────

/// A schedule fire does what any refused start does there
/// (`scheduler::fire`'s `Started::Refused`): no execution, the trigger off
/// the queue with the reason in `last_error`, counted `errored`. A sibling
/// schedule on a served flow still fires.
#[tokio::test]
async fn a_schedule_fire_is_refused_and_taken_off_the_queue() {
    let _serial = SERIAL.lock().await;
    let Some(fx) = fx().await else { return };
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = NULL")
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let ns = fresh_ns();
    let cron = "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    \
                cron: \"*/1 * * * *\"\n";
    let refused = common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &unserved_flow(&ns, "f", cron),
        "draft",
    )
    .await;
    let sibling =
        common::seed_flow(&fx.db, fx.ws, fx.user, &log_flow(&ns, "g", cron), "draft").await;
    common::make_due(&fx.db, refused, "s", 2).await;
    common::make_due(&fx.db, sibling, "s", 1).await;

    let report = egret_workflow::scheduler::fire_due(&fx.db.pool)
        .await
        .expect("a pass");
    assert_eq!(
        report,
        egret_workflow::scheduler::PassReport {
            fired: 1,
            errored: 1,
            ..Default::default()
        }
    );
    let count = |flow: Uuid| {
        let pool = fx.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM workflow.executions WHERE flow_id = $1",
            )
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        count(refused).await,
        0,
        "no execution, so none reaches RUNNING"
    );
    assert_eq!(count(sibling).await, 1);
    let (next, last_error): (Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
        sqlx::query_as(
            "SELECT next_evaluation_date, last_error FROM workflow.triggers \
              WHERE flow_id = $1 AND trigger_id = 's'",
        )
        .bind(refused)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(next, None, "off the queue");
    let why = last_error.expect("and says why");
    assert!(
        why.contains(START_REFUSAL) && why.contains(NO_HANDLER),
        "{why}"
    );
}

/// A Flow-trigger fire does what any refused start does there
/// (`flow_trigger::process`'s `Fate::Broken`): no execution, counted
/// `errored`, the event done with for that listener.
#[tokio::test]
async fn a_flow_trigger_fire_is_refused_and_the_event_is_done_with() {
    let _serial = SERIAL.lock().await;
    let Some(fx) = fx().await else { return };
    for sql in [
        "DELETE FROM workflow.flow_trigger_events",
        "DELETE FROM workflow.flow_listeners",
    ] {
        sqlx::query(sql).execute(&fx.db.pool).await.expect(sql);
    }
    let ns = fresh_ns();
    let producer_src = log_flow(&ns, "producer", "");
    let producer = common::seed_flow(&fx.db, fx.ws, fx.user, &producer_src, "draft").await;
    let listen = "triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n";
    let listener = common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &unserved_flow(&ns, "listener", listen),
        "draft",
    )
    .await;

    let ex = egret_workflow::worker::start_execution(
        &fx.db.pool,
        fx.ws,
        Some(producer),
        &producer_src,
        serde_json::json!({}),
    )
    .await
    .expect("the producer is served and starts");
    common::drive_to(&fx.db, ex, "SUCCESS").await;

    let report = egret_workflow::flow_trigger::process_events(&fx.db.pool)
        .await
        .expect("a pass");
    assert_eq!(
        report,
        egret_workflow::flow_trigger::EventReport {
            events: 1,
            errored: 1,
            ..Default::default()
        }
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
        .bind(listener)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "no execution, so none reaches RUNNING");
    assert_eq!(
        common::events(&fx.db).await,
        vec![],
        "the event is consumed"
    );
}

// ── 7: a Subflow to it ──────────────────────────────────────────────────────

async fn settle(pool: &sqlx::PgPool) {
    let reg = Handlers::with_core_tasks(egret_workflow::storage::current());
    for _ in 0..64 {
        if egret_workflow::worker::tick(pool, 16, &reg)
            .await
            .expect("tick")
            == 0
        {
            return;
        }
    }
    panic!("never settled in 64 rounds");
}

/// A `Subflow` whose target uses `egret.report.send` creates no child: its run fails
/// with "cannot execute an invalid flow" naming the type, and the parent ends
/// `FAILED` instead of waiting on a child that would stall.
#[tokio::test]
async fn a_subflow_to_it_fails_its_run_with_the_message() {
    let _serial = SERIAL.lock().await;
    let Some(fx) = fx().await else { return };
    let ns = fresh_ns();
    common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &unserved_flow(&ns, "target", ""),
        "draft",
    )
    .await;
    let parent_src = format!(
        "id: caller\nnamespace: {ns}\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: {ns}\n    flowId: target\n  \
         - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n"
    );
    let parent = egret_workflow::worker::start_execution(
        &fx.db.pool,
        fx.ws,
        None,
        &parent_src,
        serde_json::json!({}),
    )
    .await
    .expect("the parent is served and starts");
    settle(&fx.db.pool).await;

    let children: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions WHERE workspace_id = $1 AND flow_key = 'target'",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(children, 0, "no child of a flow this deployment cannot run");
    let (state, message): (String, Option<String>) = sqlx::query_as(
        "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'sub'",
    )
    .bind(parent)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(state, "FAILED");
    let message = message.unwrap_or_default();
    assert!(
        message.contains(&format!("cannot execute an invalid flow '{ns}'.'target'"))
            && message.contains(NO_HANDLER),
        "{message}"
    );
    let parent_state: String =
        sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
            .bind(parent)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(parent_state, "FAILED");
}

// ── 8: the schema ───────────────────────────────────────────────────────────

/// The schema offers none of the five unwired types, still offers `Log` and
/// (since package #4 Task C5) `egret.sql`, and its `ETag` differs from the
/// one built for a set that serves `egret.report.send`.
#[test]
fn the_schema_offers_no_unserved_type() {
    let served = served_none();
    let features = egret_workflow::spec::schema::SchemaFeatures {
        kestra_plugins: true,
        containers: true,
    };
    let deployment = egret_workflow::spec::schema::permissive_deployment();
    let schema = egret_workflow::spec::schema::served(&features, &deployment);
    let defs = schema.value["$defs"].as_object().expect("$defs");
    for t in [
        "core.http",
        "core.slack",
        "egret.dataapi.call",
        "egret.dataset.refresh",
        "egret.report.send",
    ] {
        assert!(!defs.contains_key(t), "{t} is offered");
    }
    assert!(defs.contains_key("io.kestra.plugin.core.log.Log"));
    assert!(defs.contains_key("egret.sql"), "served on every deployment");
    // Nor any script or plugin type: this deployment has no runner, whatever
    // the deployment's script settings say.
    assert!(!defs.contains_key("io.kestra.plugin.scripts.shell.Commands"));
    assert!(!defs
        .keys()
        .any(|k| k.starts_with(egret_workflow::plugins::PREFIX)));

    let with_report = egret_workflow::runtime::Served::from_type_ids(
        served
            .type_ids()
            .map(str::to_string)
            .chain(["egret.report.send".to_string()]),
    );
    let other = egret_workflow::spec::schema::served_with(&features, &deployment, &with_report);
    assert!(
        other.value["$defs"]["egret.report.send"].is_object(),
        "control"
    );
    assert_ne!(schema.etag, other.etag);
}

// ── 9: a served flow still runs ─────────────────────────────────────────────

#[tokio::test]
async fn a_served_flow_still_runs_to_success() {
    let _serial = SERIAL.lock().await;
    let Some(fx) = fx().await else { return };
    let ns = fresh_ns();
    let ex = egret_workflow::worker::start_execution(
        &fx.db.pool,
        fx.ws,
        None,
        &log_flow(&ns, "f", ""),
        serde_json::json!({}),
    )
    .await
    .expect("a served flow starts");
    settle(&fx.db.pool).await;
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "SUCCESS");
}
