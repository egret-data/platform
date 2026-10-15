//! W7e Task 4: reading triggers (spec §3.5, §3.6) --
//! `GET /api/ws/{workspace}/workflows/flows/{id}/triggers` and
//! `GET /api/ws/{workspace}/workflows/triggers`.
//!
//! Flows are saved through `FlowRepo` (so `sync` writes the schedule and
//! listener rows a real save writes); the switch rows, windows and executions
//! are seeded by SQL, since their writers are other packages' (or a later
//! task's). Every test seeds its own workspace. Requires DATABASE_URL.

mod common;

use axum::http::StatusCode;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::routes::TriggerFlags;
use serde_json::{json, Value};
use uuid::Uuid;

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";
const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";
const LEAF: &str = "io.kestra.plugin.example.Poll";

const FLAGS: TriggerFlags = TriggerFlags {
    scheduler_enabled: true,
    worker_enabled: false,
    webhooks_enabled: true,
};

fn src(ns: &str, id: &str, triggers: &str) -> String {
    let s = format!(
        "id: {id}\nnamespace: {ns}\nvariables:\n  env: prod\ntriggers:\n{triggers}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    egret_workflow::spec::parse(&s).unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{s}"));
    s
}

fn schedule(id: &str) -> String {
    format!("  - id: {id}\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n")
}

fn webhook(id: &str, key: &str) -> String {
    format!("  - id: {id}\n    type: {WEBHOOK}\n    key: \"{key}\"\n")
}

fn window_free(id: &str) -> String {
    format!("  - id: {id}\n    type: {FLOW}\n    states: [SUCCESS]\n")
}

fn depends_on(id: &str) -> String {
    format!(
        "  - id: {id}\n    type: {FLOW}\n    dependsOn:\n      - namespace: company.a\n        flowId: up1\n      - namespace: company.a\n        flowId: up2\n        states: [FAILED]\n"
    )
}

fn leaf(id: &str) -> String {
    format!("  - id: {id}\n    type: {LEAF}\n")
}

/// One of each kind, in this order: `sched`, `hook`, `on_done`, `dep`, `poll`.
fn every_kind() -> String {
    src(
        "company.a",
        "all",
        &format!(
            "{}{}{}{}{}",
            schedule("sched"),
            webhook("hook", "k-{{ flow.id }}"),
            window_free("on_done"),
            depends_on("dep"),
            leaf("poll"),
        ),
    )
}

async fn manager(db: &Db) -> (Uuid, UserId) {
    let (ws, user) = common::seed_workspace(db).await;
    common::grant(db, ws, user, &["workflow:read", "workflow:manage"]).await;
    (ws, user)
}

async fn flagged_app(db: &Db) -> common::App {
    common::app_with_trigger_flags(db, FLAGS).await
}

async fn flow_res(
    app: &common::App,
    ws: Uuid,
    user: UserId,
    flow: Uuid,
) -> axum::response::Response {
    common::get(
        app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/triggers"),
        user,
    )
    .await
}

async fn flow_triggers(app: &common::App, ws: Uuid, user: UserId, flow: Uuid) -> Value {
    let res = flow_res(app, ws, user, flow).await;
    assert_eq!(res.status(), StatusCode::OK);
    common::json(res).await
}

async fn list_res(
    app: &common::App,
    ws: Uuid,
    user: UserId,
    query: &str,
) -> axum::response::Response {
    common::get(
        app,
        &format!("/api/ws/{ws}/workflows/triggers{query}"),
        user,
    )
    .await
}

async fn list(app: &common::App, ws: Uuid, user: UserId, query: &str) -> Value {
    let res = list_res(app, ws, user, query).await;
    let status = res.status();
    let body = common::json(res).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// `"flow_id/trigger_id"` of every row, in order.
fn names(page: &Value) -> Vec<String> {
    page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{}/{}",
                r["flow_id"].as_str().unwrap(),
                r["trigger_id"].as_str().unwrap()
            )
        })
        .collect()
}

fn entry<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["triggers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["trigger_id"] == json!(id))
        .unwrap_or_else(|| panic!("no trigger {id} in {body}"))
}

fn ts(v: &Value) -> chrono::DateTime<chrono::Utc> {
    v.as_str()
        .unwrap_or_else(|| panic!("not a timestamp: {v}"))
        .parse()
        .unwrap()
}

async fn schedule_row(
    db: &Db,
    flow: Uuid,
    trig: &str,
) -> (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
) {
    sqlx::query_as(
        "SELECT next_evaluation_date, evaluated_at, last_error FROM workflow.triggers \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn sql(db: &Db, q: &'static str, flow: Uuid) {
    sqlx::query(q).bind(flow).execute(&db.pool).await.expect(q);
}

/// An execution of `flow` started by `trigger` (`None`: from the API), `ago`
/// seconds back.
async fn exec(db: &Db, ws: Uuid, flow: Uuid, trigger: Option<&str>, ago: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            execution_trigger, created_at) \
         VALUES ($1, $2, 'company.a', 'all', 's', 'SUCCESS', '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]', \
                 CASE WHEN $3::text IS NULL THEN NULL \
                      ELSE jsonb_build_object('id', $3::text, 'type', 't', 'variables', '{}'::jsonb) END, \
                 now() - make_interval(secs => $4)) \
         RETURNING id",
    )
    .bind(ws)
    .bind(flow)
    .bind(trigger)
    .bind(ago)
    .fetch_one(&db.pool)
    .await
    .expect("seed execution")
}

/// A `dependsOn` window on `(flow, trig)` from `start` to `end` holding `met`.
async fn window(db: &Db, flow: Uuid, trig: &str, start_mins: i32, end_mins: i32, met: Value) {
    sqlx::query(
        "INSERT INTO workflow.flow_trigger_windows (flow_id, trigger_id, window_start, window_end, results) \
         VALUES ($1, $2, now() + make_interval(mins => $3), now() + make_interval(mins => $4), $5)",
    )
    .bind(flow)
    .bind(trig)
    .bind(start_mins)
    .bind(end_mins)
    .bind(met)
    .execute(&db.pool)
    .await
    .expect("seed window");
}

/// The condition key egret computes for `dep`'s first `dependsOn` entry.
fn first_condition_key() -> String {
    let flow = egret_workflow::spec::parse(&every_kind()).unwrap();
    let t = flow.triggers.iter().find(|t| t.id == "dep").unwrap();
    let egret_workflow::models::TriggerKind::Flow(spec) = &t.kind else {
        unreachable!()
    };
    egret_workflow::trigger::condition_key(&spec.depends_on[0])
}

// ── 1. per flow, one of each kind ─────────────────────────────────────────────

#[tokio::test]
async fn each_kind_reads_every_field_in_declaration_order() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    let app = flagged_app(&db).await;

    let body = flow_triggers(&app, ws, user, flow).await;
    let ids: Vec<&str> = body["triggers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["trigger_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["sched", "hook", "on_done", "dep", "poll"]);
    assert_eq!(body["flow_disabled"], json!(false));
    assert_eq!(body["flow_deleted"], json!(false));

    let (next, _, _) = schedule_row(&db, flow, "sched").await;
    let mut sched = entry(&body, "sched").clone();
    assert_eq!(
        ts(&sched["schedule"]["next_evaluation_date"]),
        next.unwrap()
    );
    sched["schedule"]["next_evaluation_date"] = Value::Null;
    assert_eq!(
        sched,
        json!({
            "trigger_id": "sched", "type": SCHEDULE, "kind": "schedule",
            "yaml_disabled": false, "runtime_disabled": false,
            "disabled_by": null, "disabled_by_name": null, "disabled_at": null,
            "effective_disabled": false, "can_toggle": true, "toggle_refusal": null,
            "duplicate_id": false,
            "schedule": {
                "cron": "*/1 * * * *", "with_seconds": false, "timezone": "UTC",
                "next_evaluation_date": null, "evaluated_at": null, "last_error": null,
                "off_queue": false, "missed_ticks": null, "recover_missed_schedules": null
            },
            "flow": null, "webhook": null, "last_execution": null,
            "backfill": null, "can_backfill": true, "backfill_refusal": null
        })
    );
    assert_eq!(
        entry(&body, "hook"),
        &json!({
            "trigger_id": "hook", "type": WEBHOOK, "kind": "webhook",
            "yaml_disabled": false, "runtime_disabled": false,
            "disabled_by": null, "disabled_by_name": null, "disabled_at": null,
            "effective_disabled": false, "can_toggle": true, "toggle_refusal": null,
            "duplicate_id": false, "schedule": null, "flow": null,
            "webhook": {
                "url_path": format!("/workflows/v1/{ws}/executions/webhook/company.a/all/k-all"),
                "key_error": null, "unsupported": null
            },
            "last_execution": null,
            "backfill": null, "can_backfill": false, "backfill_refusal": "not_schedule"
        })
    );
    assert_eq!(
        entry(&body, "on_done"),
        &json!({
            "trigger_id": "on_done", "type": FLOW, "kind": "flow",
            "yaml_disabled": false, "runtime_disabled": false,
            "disabled_by": null, "disabled_by_name": null, "disabled_at": null,
            "effective_disabled": false, "can_toggle": true, "toggle_refusal": null,
            "duplicate_id": false, "schedule": null,
            "flow": { "states": ["SUCCESS"], "listening": true, "not_listening_reason": null, "depends_on": null },
            "webhook": null, "last_execution": null,
            "backfill": null, "can_backfill": false, "backfill_refusal": "not_schedule"
        })
    );
    assert_eq!(
        entry(&body, "dep"),
        &json!({
            "trigger_id": "dep", "type": FLOW, "kind": "flow",
            "yaml_disabled": false, "runtime_disabled": false,
            "disabled_by": null, "disabled_by_name": null, "disabled_at": null,
            "effective_disabled": false, "can_toggle": true, "toggle_refusal": null,
            "duplicate_id": false, "schedule": null,
            "flow": {
                "states": ["SUCCESS", "WARNING", "FAILED", "KILLED", "CANCELLED", "RETRIED", "SKIPPED", "RESUBMITTED", "PAUSED"],
                "listening": true, "not_listening_reason": null,
                "depends_on": {
                    "mode": "ALL", "min_satisfied": null,
                    "conditions": [
                        { "namespace": "company.a", "flow_id": "up1", "states": null, "labels": null, "when": "true", "met": false },
                        { "namespace": "company.a", "flow_id": "up2", "states": ["FAILED"], "labels": null, "when": "true", "met": false }
                    ],
                    "window": null
                }
            },
            "webhook": null, "last_execution": null,
            "backfill": null, "can_backfill": false, "backfill_refusal": "not_schedule"
        })
    );
    assert_eq!(
        entry(&body, "poll"),
        &json!({
            "trigger_id": "poll", "type": LEAF, "kind": "other",
            "yaml_disabled": false, "runtime_disabled": false,
            "disabled_by": null, "disabled_by_name": null, "disabled_at": null,
            "effective_disabled": false, "can_toggle": false, "toggle_refusal": "not_supported",
            "duplicate_id": false, "schedule": null, "flow": null, "webhook": null,
            "last_execution": null,
            "backfill": null, "can_backfill": false, "backfill_refusal": "not_schedule"
        })
    );
}

// ── 2. schedule ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn schedule_fields_come_from_its_row() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let triggers = format!(
        "{}  - id: tz\n    type: {SCHEDULE}\n    cron: \"0 * * * *\"\n    timezone: Asia/Ho_Chi_Minh\n    recoverMissedSchedules: NONE\n",
        schedule("s")
    );
    let flow = common::seed_flow(&db, ws, user, &src("company.a", "sch", &triggers), "draft").await;
    let app = flagged_app(&db).await;

    // The row's fields, as written by the loop.
    sqlx::query(
        "UPDATE workflow.triggers SET evaluated_at = '2026-10-01T00:00:00Z', last_error = 'boom' \
          WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .execute(&db.pool)
    .await
    .unwrap();
    let body = flow_triggers(&app, ws, user, flow).await;
    let s = &entry(&body, "s")["schedule"];
    let (next, evaluated, err) = schedule_row(&db, flow, "s").await;
    assert_eq!(ts(&s["next_evaluation_date"]), next.unwrap());
    assert_eq!(ts(&s["evaluated_at"]), evaluated.unwrap());
    assert_eq!(s["last_error"], json!(err.unwrap()));
    assert_eq!(s["timezone"], json!("UTC"), "absent is UTC");
    assert_eq!(s["recover_missed_schedules"], json!(null));
    assert_eq!(s["off_queue"], json!(false));
    assert_eq!(s["missed_ticks"], json!(null), "not yet due");
    let tz = &entry(&body, "tz")["schedule"];
    assert_eq!(tz["timezone"], json!("Asia/Ho_Chi_Minh"));
    assert_eq!(
        tz["recover_missed_schedules"],
        json!("NONE"),
        "echoes the YAML"
    );

    common::make_due(&db, flow, "s", 3).await;
    let body = flow_triggers(&app, ws, user, flow).await;
    assert_eq!(
        entry(&body, "s")["schedule"]["missed_ticks"],
        json!({"count": 3, "more": false})
    );

    sql(
        &db,
        "UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) - interval '2000 minutes' \
          WHERE flow_id = $1 AND trigger_id = 's'",
        flow,
    )
    .await;
    let body = flow_triggers(&app, ws, user, flow).await;
    assert_eq!(
        entry(&body, "s")["schedule"]["missed_ticks"],
        json!({"count": 1000, "more": true})
    );

    sql(
        &db,
        "UPDATE workflow.triggers SET next_evaluation_date = NULL WHERE flow_id = $1 AND trigger_id = 's'",
        flow,
    )
    .await;
    let body = flow_triggers(&app, ws, user, flow).await;
    let s = &entry(&body, "s")["schedule"];
    assert_eq!(s["off_queue"], json!(true));
    assert_eq!(s["next_evaluation_date"], json!(null));
    assert_eq!(
        s["missed_ticks"],
        json!(null),
        "off the queue is never overdue"
    );
}

// ── 3. the switch ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_switch_row_reads_with_its_switcher() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src("company.a", "sw", &webhook("h", "k")),
        "draft",
    )
    .await;
    let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO workflow.disabled_triggers (flow_id, trigger_id, workspace_id, disabled_by) \
         VALUES ($1, 'h', $2, $3) RETURNING disabled_at",
    )
    .bind(flow)
    .bind(ws)
    .bind(user.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let app = flagged_app(&db).await;

    let body = flow_triggers(&app, ws, user, flow).await;
    let e = entry(&body, "h");
    assert_eq!(e["runtime_disabled"], json!(true));
    assert_eq!(e["yaml_disabled"], json!(false));
    assert_eq!(e["effective_disabled"], json!(true));
    assert_eq!(e["disabled_by"], json!(user.0));
    assert_eq!(
        e["disabled_by_name"],
        json!("t"),
        "seed_workspace's user name"
    );
    assert_eq!(ts(&e["disabled_at"]), at);
    assert_eq!(
        e["can_toggle"],
        json!(true),
        "a runtime-off trigger can be switched back on"
    );
}

// ── 4. flow trigger ───────────────────────────────────────────────────────────

#[tokio::test]
async fn a_depends_on_window_labels_its_conditions() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    let app = flagged_app(&db).await;

    // The window also holds a key no condition has: it must not count.
    window(
        &db,
        flow,
        "dep",
        -10,
        50,
        json!({ first_condition_key(): true, "stray": true }),
    )
    .await;
    let body = flow_triggers(&app, ws, user, flow).await;
    let d = &entry(&body, "dep")["flow"]["depends_on"];
    let met: Vec<&Value> = d["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["met"])
        .collect();
    assert_eq!(met, vec![&json!(true), &json!(false)]);
    assert_eq!(d["window"]["kind"], json!("duration"));
    let (start, end): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as(
            "SELECT window_start, window_end FROM workflow.flow_trigger_windows WHERE flow_id = $1",
        )
        .bind(flow)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(ts(&d["window"]["start"]), start);
    assert_eq!(ts(&d["window"]["end"]), end);
    assert_eq!(entry(&body, "dep")["flow"]["listening"], json!(true));

    // Ended: reported as none, and nothing in it is met.
    sql(
        &db,
        "UPDATE workflow.flow_trigger_windows SET window_start = now() - interval '2 hours', \
                window_end = now() - interval '1 minute' WHERE flow_id = $1",
        flow,
    )
    .await;
    let body = flow_triggers(&app, ws, user, flow).await;
    let d = &entry(&body, "dep")["flow"]["depends_on"];
    assert_eq!(d["window"], json!(null));
    assert!(
        d["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["met"] == json!(false)),
        "{d}"
    );
}

#[tokio::test]
async fn listening_is_the_listener_row() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    let app = flagged_app(&db).await;
    sql(
        &db,
        "DELETE FROM workflow.flow_listeners WHERE flow_id = $1 AND trigger_id = 'on_done'",
        flow,
    )
    .await;
    let body = flow_triggers(&app, ws, user, flow).await;
    assert_eq!(entry(&body, "on_done")["flow"]["listening"], json!(false));
    assert_eq!(entry(&body, "dep")["flow"]["listening"], json!(true));
}

// ── 5. webhook ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_webhook_path_key_error_and_unsupported() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let triggers = format!(
        "{}{}  - id: waits\n    type: {WEBHOOK}\n    key: plain\n    wait: true\n",
        webhook("ok", "k-{{ flow.id }}-{{ vars.env }}"),
        webhook("broken", "{{ nosuchroot.nothing }}"),
    );
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src("company.a", "hooks", &triggers),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;

    let body = flow_triggers(&app, ws, user, flow).await;
    assert_eq!(
        entry(&body, "ok")["webhook"],
        json!({
            "url_path": format!("/workflows/v1/{ws}/executions/webhook/company.a/hooks/k-hooks-prod"),
            "key_error": null, "unsupported": null
        })
    );
    assert_eq!(
        entry(&body, "broken")["webhook"],
        json!({ "url_path": null, "key_error": "the key does not render", "unsupported": null })
    );
    let w = &entry(&body, "waits")["webhook"];
    assert_eq!(
        w["url_path"],
        json!(format!(
            "/workflows/v1/{ws}/executions/webhook/company.a/hooks/plain"
        ))
    );
    assert!(
        w["unsupported"].as_str().unwrap().contains("wait: true"),
        "{w}"
    );
}

// ── 6. last execution ─────────────────────────────────────────────────────────

#[tokio::test]
async fn the_last_execution_is_the_triggers_newest() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    let _old = exec(&db, ws, flow, Some("hook"), 7200).await;
    let new = exec(&db, ws, flow, Some("hook"), 3600).await;
    let other = exec(&db, ws, flow, Some("sched"), 60).await;
    // The flow's newest execution, started from the API: it carries no
    // trigger, so it is no trigger's last (`hook` still answers `new`).
    let _api = exec(&db, ws, flow, None, 0).await;
    let app = flagged_app(&db).await;

    let body = flow_triggers(&app, ws, user, flow).await;
    let last = &entry(&body, "hook")["last_execution"];
    assert_eq!(
        last["id"],
        json!(new),
        "not the older one, nor the newer API start"
    );
    assert_eq!(last["state"], json!("SUCCESS"));
    assert_eq!(entry(&body, "sched")["last_execution"]["id"], json!(other));
    assert_eq!(
        entry(&body, "poll")["last_execution"],
        json!(null),
        "a trigger that never fired has none"
    );

    let page = list(&app, ws, user, "?kind=webhook").await;
    assert_eq!(
        page["rows"][0]["last_execution"]["id"],
        json!(new),
        "the page reads it too"
    );
}

// ── 7. the top ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_top_flags_and_a_trashed_flow() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let app = flagged_app(&db).await;

    let yaml_off = format!(
        "id: yoff\nnamespace: company.a\ndisabled: true\ntriggers:\n{}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        schedule("s")
    );
    let yoff = common::seed_flow(&db, ws, user, &yaml_off, "draft").await;
    let body = flow_triggers(&app, ws, user, yoff).await;
    assert_eq!(body["flow_disabled"], json!(true));
    assert_eq!(entry(&body, "s")["yaml_disabled"], json!(true));
    assert_eq!(entry(&body, "s")["toggle_refusal"], json!("yaml_disabled"));
    assert_eq!(
        (
            &body["scheduler_enabled"],
            &body["worker_enabled"],
            &body["webhooks_enabled"]
        ),
        (&json!(true), &json!(false), &json!(true)),
        "the flags echo TriggerFlags"
    );

    let soff = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "soff",
            &format!("{}{}", schedule("s"), webhook("h", "k")),
        ),
        "disabled",
    )
    .await;
    let body = flow_triggers(&app, ws, user, soff).await;
    assert_eq!(body["flow_disabled"], json!(true), "state = 'disabled'");
    // `state = 'disabled'` fires nothing (the claim, the listener select and
    // the webhook's 409 all read it), so every entry is effective-disabled,
    // though neither the YAML nor the switch says so and the switch stays
    // usable.
    for e in body["triggers"].as_array().unwrap() {
        assert_eq!(e["effective_disabled"], json!(true), "{e}");
        assert_eq!(e["yaml_disabled"], json!(false), "{e}");
        assert_eq!(e["runtime_disabled"], json!(false), "{e}");
        assert_eq!(e["can_toggle"], json!(true), "{e}");
    }
    common::seed_flow(
        &db,
        ws,
        user,
        &src("company.a", "fon", &schedule("s")),
        "draft",
    )
    .await;
    let off = list(&app, ws, user, "?state=disabled").await;
    assert_eq!(names(&off), vec!["soff/s", "soff/h", "yoff/s"]);
    // The row explains an effective-off trigger that neither the YAML of the
    // trigger nor the switch disables: its flow is off (YAML or `state`).
    for r in off["rows"].as_array().unwrap() {
        assert_eq!(r["flow_disabled"], json!(true), "{r}");
    }
    assert_eq!(
        off["rows"][0]["yaml_disabled"],
        json!(false),
        "soff: by state"
    );
    let on = list(&app, ws, user, "?state=enabled").await;
    assert_eq!(names(&on), vec!["fon/s"]);
    assert_eq!(on["rows"][0]["flow_disabled"], json!(false));

    let gone = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    egret_workflow::FlowRepo::new(db.clone())
        .soft_delete(ws, gone)
        .await
        .unwrap();
    let body = flow_triggers(&app, ws, user, gone).await;
    assert_eq!(body["flow_deleted"], json!(true));
    assert_eq!(body["triggers"].as_array().unwrap().len(), 5);
    for e in body["triggers"].as_array().unwrap() {
        assert_eq!(e["effective_disabled"], json!(true), "{e}");
        assert_eq!(e["can_toggle"], json!(false), "{e}");
        assert_eq!(e["toggle_refusal"], json!("flow_deleted"), "{e}");
    }

    let other = common::app_with_trigger_flags(&db, TriggerFlags::default()).await;
    let body = flow_triggers(&other, ws, user, soff).await;
    assert_eq!(
        (
            &body["scheduler_enabled"],
            &body["worker_enabled"],
            &body["webhooks_enabled"]
        ),
        (&json!(false), &json!(false), &json!(false))
    );
}

// ── 8. the global list ────────────────────────────────────────────────────────

#[tokio::test]
async fn the_list_filters_pages_and_counts_the_unparsable() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let f1 = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "f1",
            &format!("{}{}", schedule("s1"), webhook("h1", "k")),
        ),
        "draft",
    )
    .await;
    let f2 = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "f2",
            &format!("{}{}{}", window_free("on"), depends_on("dep"), leaf("p")),
        ),
        "draft",
    )
    .await;
    common::seed_flow(
        &db,
        ws,
        user,
        &src("companyx", "f3", &schedule("s3")),
        "draft",
    )
    .await;
    let gone = common::seed_flow(
        &db,
        ws,
        user,
        &src("company.b", "gone", &schedule("sg")),
        "draft",
    )
    .await;
    egret_workflow::FlowRepo::new(db.clone())
        .soft_delete(ws, gone)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan) \
         VALUES ($1, 'company.c', 'bad', 'tasks: [unclosed', '{}'::jsonb)",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .unwrap();
    common::switch_off(&db, f1, "h1").await;
    let app = flagged_app(&db).await;

    let all = list(&app, ws, user, "").await;
    assert_eq!(
        names(&all),
        vec!["f1/s1", "f1/h1", "f2/on", "f2/dep", "f2/p", "f3/s3"]
    );
    assert_eq!(all["total"], json!(6));
    assert_eq!(all["unparsable"], json!(1));
    assert_eq!((&all["limit"], &all["offset"]), (&json!(50), &json!(0)));
    assert_eq!(
        (
            &all["scheduler_enabled"],
            &all["worker_enabled"],
            &all["webhooks_enabled"]
        ),
        (&json!(true), &json!(false), &json!(true))
    );
    for r in all["rows"].as_array().unwrap() {
        assert_eq!(r["webhook"], json!(null), "{r}");
        assert!(
            r["namespace"].as_str().unwrap().starts_with("company"),
            "{r}"
        );
    }
    let dep = &all["rows"][3];
    assert_eq!(dep["flow_uuid"], json!(f2));
    assert_eq!(dep["flow_id"], json!("f2"));
    assert_eq!(dep["namespace"], json!("company.a"));
    assert_eq!(dep["kind"], json!("flow"));
    assert_eq!(
        dep["flow"]["depends_on"]["mode"],
        json!("ALL"),
        "the entry's own `flow` survives"
    );
    assert_eq!(
        dep["flow"]["depends_on"]["conditions"],
        json!(null),
        "no conditions in a row"
    );
    assert_eq!(all["rows"][0]["flow_uuid"], json!(f1));

    let ns = list(&app, ws, user, "?namespace_prefix=company").await;
    assert_eq!(
        names(&ns),
        vec!["f1/s1", "f1/h1", "f2/on", "f2/dep", "f2/p"],
        "dot-anchored: companyx is out"
    );
    assert_eq!(ns["unparsable"], json!(1), "company.c/bad is in scope");
    let ns = list(&app, ws, user, "?namespace_prefix=companyx").await;
    assert_eq!(
        (names(&ns), &ns["unparsable"]),
        (vec!["f3/s3".to_string()], &json!(0))
    );

    let one = list(&app, ws, user, &format!("?flow_id={f2}")).await;
    assert_eq!(names(&one), vec!["f2/on", "f2/dep", "f2/p"]);
    assert_eq!(one["unparsable"], json!(0));

    let kinds = list(&app, ws, user, "?kind=schedule,webhook").await;
    assert_eq!(names(&kinds), vec!["f1/s1", "f1/h1", "f3/s3"]);
    let other = list(&app, ws, user, "?kind=other").await;
    assert_eq!(names(&other), vec!["f2/p"]);

    let off = list(&app, ws, user, "?state=disabled").await;
    assert_eq!(names(&off), vec!["f1/h1"]);
    // Present but empty is no filter, as if absent.
    for empty in ["?kind=", "?state=", "?kind=,&state="] {
        assert_eq!(
            list(&app, ws, user, empty).await["total"],
            json!(6),
            "{empty}"
        );
    }
    let on = list(&app, ws, user, "?state=enabled").await;
    assert_eq!(on["total"], json!(5));

    assert_eq!(
        names(&list(&app, ws, user, "?q=COMPANYX").await),
        vec!["f3/s3"],
        "namespace"
    );
    assert_eq!(
        names(&list(&app, ws, user, "?q=H1").await),
        vec!["f1/h1"],
        "trigger id"
    );
    assert_eq!(
        names(&list(&app, ws, user, "?q=f2").await),
        vec!["f2/on", "f2/dep", "f2/p"],
        "flow id"
    );

    let page = list(&app, ws, user, "?limit=1&offset=1").await;
    assert_eq!(names(&page), vec!["f1/h1"]);
    assert_eq!(
        (&page["total"], &page["limit"], &page["offset"]),
        (&json!(6), &json!(1), &json!(1))
    );
    let big = list(&app, ws, user, "?limit=500").await;
    assert_eq!(big["limit"], json!(200), "clamped and echoed");

    for bad in ["?namespace_prefix=Bad!", "?kind=cron", "?state=off"] {
        let res = list_res(&app, ws, user, bad).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn list_rows_carry_no_conditions_and_no_webhook() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    let app = flagged_app(&db).await;
    // A row's schedule counts its missed ticks too (for the page only).
    common::make_due(&db, flow, "sched", 3).await;
    let page = list(&app, ws, user, "").await;
    let rows = page["rows"].as_array().unwrap();
    let dep = rows
        .iter()
        .find(|r| r["trigger_id"] == json!("dep"))
        .unwrap();
    let d = &dep["flow"]["depends_on"];
    assert_eq!(d["conditions"], json!(null), "{dep}");
    assert_eq!(d["mode"], json!("ALL"));
    let hook = rows
        .iter()
        .find(|r| r["trigger_id"] == json!("hook"))
        .unwrap();
    assert_eq!(hook["webhook"], json!(null), "{hook}");
    let sched = rows
        .iter()
        .find(|r| r["trigger_id"] == json!("sched"))
        .unwrap();
    assert_eq!(
        sched["schedule"]["missed_ticks"],
        json!({"count": 3, "more": false}),
        "{sched}"
    );
    assert!(
        sched["schedule"]["next_evaluation_date"].is_string(),
        "{sched}"
    );
}

// ── 9. the bound ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn more_than_two_thousand_flows_in_scope_answer_422() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    sqlx::query(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan) \
         SELECT $1, ns, 'f' || i, \
                'id: f' || i || E'\\nnamespace: ' || ns || E'\\ntasks:\\n  - id: t\\n    type: io.kestra.plugin.core.log.Log\\n    message: hi\\n', \
                '{}'::jsonb \
           FROM generate_series(1, 2001) AS i, \
                LATERAL (SELECT CASE WHEN i <= 10 THEN 'bulk.ten' ELSE 'bulk.rest' END AS ns) n",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("bulk flows");
    let app = flagged_app(&db).await;

    let res = list_res(&app, ws, user, "").await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = common::json(res).await;
    assert_eq!(body["code"], json!("triggers_too_large"));
    assert_eq!(
        body["error"],
        json!("more than 2000 flows match; narrow by namespace")
    );

    let ten = list(&app, ws, user, "?namespace_prefix=bulk.ten").await;
    assert_eq!(
        ten["total"],
        json!(0),
        "no triggers, but answered: the bound is after the filters"
    );
    assert_eq!(ten["unparsable"], json!(0), "the minimal source parses");

    // Exactly 2,000 in scope (one trashed) is still answered: the 422 is for
    // MORE than the bound.
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE workspace_id = $1 AND flow_id = 'f2001'")
        .bind(ws)
        .execute(&db.pool)
        .await
        .unwrap();
    let all = list(&app, ws, user, "").await;
    assert_eq!((&all["total"], &all["unparsable"]), (&json!(0), &json!(0)));
}

// ── 10. isolation and permissions ─────────────────────────────────────────────

#[tokio::test]
async fn another_workspace_and_the_permissions() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let mine = common::seed_flow(&db, ws, user, &every_kind(), "draft").await;
    let (other_ws, other_user) = manager(&db).await;
    let theirs = common::seed_flow(
        &db,
        other_ws,
        other_user,
        &src("company.a", "theirs", &schedule("x")),
        "draft",
    )
    .await;
    common::switch_off(&db, theirs, "x").await;
    let app = flagged_app(&db).await;

    let res = flow_res(&app, ws, user, theirs).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let page = list(&app, ws, user, "").await;
    assert!(
        page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["flow_uuid"] == json!(mine)),
        "{page}"
    );
    assert_eq!(page["total"], json!(5));
    assert_eq!(page["can_manage"], json!(true), "a manager may switch");
    let page = list(&app, ws, user, &format!("?flow_id={theirs}")).await;
    assert_eq!(page["total"], json!(0));

    let reader = egret_core::UserId(
        sqlx::query_scalar(
            "INSERT INTO users (email, name, provider, status, password_hash) \
             VALUES ($1, 'r', 'local', 'active', 'x') RETURNING id",
        )
        .bind(format!("r_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
    );
    common::grant(&db, ws, reader, &["workflow:read"]).await;
    // The PDP reads the grants when it is built: a fresh app sees the new one.
    let app = flagged_app(&db).await;
    let body = flow_triggers(&app, ws, reader, mine).await;
    let e = entry(&body, "sched");
    assert_eq!(
        (&e["can_toggle"], &e["toggle_refusal"]),
        (&json!(false), &json!("forbidden"))
    );
    assert_eq!(
        entry(&body, "poll")["toggle_refusal"],
        json!("not_supported"),
        "the kind comes first"
    );
    let page = list(&app, ws, reader, "?kind=schedule").await;
    assert_eq!(page["rows"][0]["toggle_refusal"], json!("forbidden"));
    assert_eq!(page["can_manage"], json!(false), "a reader may not");
    // Said per caller, even where no row says `forbidden` (a Leaf comes first).
    let page = list(&app, ws, reader, "?kind=other").await;
    assert_eq!(page["rows"][0]["toggle_refusal"], json!("not_supported"));
    assert_eq!(page["can_manage"], json!(false));

    let nobody = egret_core::UserId(
        sqlx::query_scalar(
            "INSERT INTO users (email, name, provider, status, password_hash) \
             VALUES ($1, 'n', 'local', 'active', 'x') RETURNING id",
        )
        .bind(format!("n_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
    );
    common::grant(&db, ws, nobody, &["workflow:manage"]).await;
    let app = flagged_app(&db).await;
    assert_eq!(
        flow_res(&app, ws, nobody, mine).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        list_res(&app, ws, nobody, "").await.status(),
        StatusCode::FORBIDDEN
    );

    let anon = common::anon_get(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{mine}/triggers"),
    )
    .await;
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    let anon = common::anon_get(&app, &format!("/api/ws/{ws}/workflows/triggers")).await;
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_source_that_does_not_parse_is_a_400_per_flow() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let bad: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan) \
         VALUES ($1, 'company.c', 'bad', 'tasks: [unclosed', '{}'::jsonb) RETURNING id",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let app = flagged_app(&db).await;
    assert_eq!(
        flow_res(&app, ws, user, bad).await.status(),
        StatusCode::BAD_REQUEST
    );
}

// ── 11. the switch (W7e Task 5, spec §3.3, §3.7) ──────────────────────────────

fn switch_path(ws: Uuid, flow: Uuid, trig: &str) -> String {
    format!("/api/ws/{ws}/workflows/flows/{flow}/triggers/{trig}/disabled")
}

fn bulk_path(ws: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/triggers/disabled")
}

async fn switch_res(
    app: &common::App,
    ws: Uuid,
    user: UserId,
    flow: Uuid,
    trig: &str,
    body: Value,
) -> axum::response::Response {
    common::put(app, &switch_path(ws, flow, trig), user, body).await
}

/// Switch `trig` and expect a 200; the answer (the trigger's entry).
async fn switch(
    app: &common::App,
    ws: Uuid,
    user: UserId,
    flow: Uuid,
    trig: &str,
    disabled: bool,
    recover_missed: bool,
) -> Value {
    let res = switch_res(
        app,
        ws,
        user,
        flow,
        trig,
        json!({"disabled": disabled, "recover_missed": recover_missed}),
    )
    .await;
    let status = res.status();
    let body = common::json(res).await;
    assert_eq!(status, StatusCode::OK, "{trig}: {body}");
    body
}

/// The status and the JSON body of a response.
async fn answer(res: axum::response::Response) -> (StatusCode, Value) {
    let status = res.status();
    (status, common::json(res).await)
}

/// `workflow.triggers.disabled` (the effective copy) of a schedule.
async fn schedule_copy(db: &Db, flow: Uuid, trig: &str) -> bool {
    sqlx::query_scalar(
        "SELECT disabled FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_one(&db.pool)
    .await
    .expect("a schedule row")
}

/// `workflow.flow_listeners.disabled` (the effective copy) of a listener.
async fn listener_copy(db: &Db, flow: Uuid, trig: &str) -> bool {
    sqlx::query_scalar(
        "SELECT disabled FROM workflow.flow_listeners WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_one(&db.pool)
    .await
    .expect("a listener row")
}

async fn db_now(db: &Db) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(&db.pool)
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

/// A `*/1` schedule with `recoverMissedSchedules: r` when given.
fn schedule_recovering(id: &str, r: Option<&str>) -> String {
    let mut s = schedule(id);
    if let Some(r) = r {
        s.push_str(&format!("    recoverMissedSchedules: {r}\n"));
    }
    s
}

fn yaml_disabled_schedule(id: &str) -> String {
    format!("{}    disabled: true\n", schedule(id))
}

fn is_minute(t: chrono::DateTime<chrono::Utc>) -> bool {
    t.timestamp() % 60 == 0 && t.timestamp_subsec_nanos() == 0
}

/// A user holding `perms` in `ws`.
async fn user_with(db: &Db, ws: Uuid, perms: &[&str]) -> UserId {
    let u = egret_core::UserId(
        sqlx::query_scalar(
            "INSERT INTO users (email, name, provider, status, password_hash) \
             VALUES ($1, 'r', 'local', 'active', 'x') RETURNING id",
        )
        .bind(format!("r_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
    );
    common::grant(db, ws, u, perms).await;
    u
}

#[tokio::test]
async fn off_then_on_for_each_kind() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "sw",
            &format!(
                "{}{}{}",
                schedule("sched"),
                webhook("hook", "k"),
                window_free("on_done")
            ),
        ),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    assert!(!schedule_copy(&db, flow, "sched").await);
    assert!(!listener_copy(&db, flow, "on_done").await);

    for trig in ["sched", "hook", "on_done"] {
        let e = switch(&app, ws, user, flow, trig, true, false).await;
        assert_eq!(e["trigger_id"], json!(trig), "{e}");
        assert_eq!(e["runtime_disabled"], json!(true), "{e}");
        assert_eq!(e["effective_disabled"], json!(true), "{e}");
        assert_eq!(e["disabled_by"], json!(user.0), "{e}");
        assert_eq!(e["disabled_by_name"], json!("t"), "{e}");
        assert_eq!(
            common::audit_mutations(&app),
            vec![(
                "workflow_trigger:disable".to_string(),
                Some(format!("{flow}/{trig}")),
                json!({"recover_missed": false, "next_evaluation_date": null, "backfill_resume_from": null}),
            )]
        );
    }
    assert_eq!(
        common::switched_off(&db, flow).await,
        vec!["hook", "on_done", "sched"]
    );
    let by: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT disabled_by FROM workflow.disabled_triggers WHERE flow_id = $1")
            .bind(flow)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert!(by.iter().all(|b| *b == Some(user.0)), "{by:?}");
    assert!(
        schedule_copy(&db, flow, "sched").await,
        "the schedule's copy"
    );
    assert!(
        listener_copy(&db, flow, "on_done").await,
        "the listener's copy"
    );
    // The read route agrees with the answers.
    let body = flow_triggers(&app, ws, user, flow).await;
    for trig in ["sched", "hook", "on_done"] {
        assert_eq!(entry(&body, trig)["runtime_disabled"], json!(true));
    }

    for trig in ["sched", "hook", "on_done"] {
        let e = switch(&app, ws, user, flow, trig, false, false).await;
        assert_eq!(e["runtime_disabled"], json!(false), "{e}");
        assert_eq!(e["effective_disabled"], json!(false), "{e}");
        assert_eq!(e["disabled_by"], json!(null), "{e}");
        assert_eq!(
            common::audit_mutations(&app),
            vec![(
                "workflow_trigger:enable".to_string(),
                Some(format!("{flow}/{trig}")),
                json!({"recover_missed": false, "next_evaluation_date": null, "backfill_resume_from": null}),
            )]
        );
    }
    assert!(common::switched_off(&db, flow).await.is_empty());
    assert!(!schedule_copy(&db, flow, "sched").await);
    assert!(!listener_copy(&db, flow, "on_done").await);
}

/// §3.3's outcomes on a schedule overdue by five ticks, switched off, then on.
#[tokio::test]
async fn re_enabling_an_overdue_schedule_skips_or_recovers() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let ids: [(&str, Option<&str>); 8] = [
        ("skip", None),
        ("absent", None),
        ("all", Some("ALL")),
        ("none", Some("NONE")),
        ("last", Some("LAST")),
        ("last_never", Some("LAST")),
        ("some", Some("SOME")),
        ("some_skip", Some("SOME")),
    ];
    let triggers: String = ids
        .iter()
        .map(|(id, r)| schedule_recovering(id, *r))
        .collect();
    let flow = common::seed_flow(&db, ws, user, &src("company.a", "re", &triggers), "draft").await;
    let app = flagged_app(&db).await;
    for (id, _) in ids {
        common::make_due(&db, flow, id, 5).await;
        switch(&app, ws, user, flow, id, true, false).await;
    }
    // `evaluated_at` and `last_error` are never written by the switch.
    sql(
        &db,
        "UPDATE workflow.triggers SET evaluated_at = next_evaluation_date - interval '1 minute', \
                last_error = 'kept' \
          WHERE flow_id = $1 AND trigger_id IN ('skip', 'last')",
        flow,
    )
    .await;
    common::audit_mutations(&app);

    // A skip: the first tick after the transaction's now().
    let assert_skipped = |id: &'static str,
                          before: chrono::DateTime<chrono::Utc>,
                          after: chrono::DateTime<chrono::Utc>,
                          next: Option<chrono::DateTime<chrono::Utc>>| {
        let next = next.unwrap_or_else(|| panic!("{id}: off the queue"));
        assert!(
            next > before && next <= after + chrono::Duration::seconds(60) && is_minute(next),
            "{id}: {next} is not the first tick after a now() in [{before}, {after}]"
        );
        next
    };

    for (id, recover) in [
        ("skip", false),
        ("none", true),
        ("last_never", true),
        ("some_skip", false),
    ] {
        let (_, evaluated, err) = schedule_row(&db, flow, id).await;
        let before = db_now(&db).await;
        let e = switch(&app, ws, user, flow, id, false, recover).await;
        let after = db_now(&db).await;
        let (next, evaluated_after, err_after) = schedule_row(&db, flow, id).await;
        let next = assert_skipped(id, before, after, next);
        assert_eq!(
            ts(&e["schedule"]["next_evaluation_date"]),
            next,
            "{id}: the answer is what was committed"
        );
        assert_eq!((evaluated_after, err_after), (evaluated, err), "{id}");
        assert_eq!(
            common::audit_mutations(&app),
            vec![(
                "workflow_trigger:enable".to_string(),
                Some(format!("{flow}/{id}")),
                json!({"recover_missed": recover, "next_evaluation_date": next, "backfill_resume_from": null}),
            )],
            "{id}"
        );
    }

    // Recovered: every missed tick stays owed.
    for id in ["absent", "all"] {
        let row = schedule_row(&db, flow, id).await;
        switch(&app, ws, user, flow, id, false, true).await;
        assert_eq!(schedule_row(&db, flow, id).await, row, "{id}: dates kept");
        assert_eq!(
            common::audit_mutations(&app)[0].2,
            json!({"recover_missed": true, "next_evaluation_date": null, "backfill_resume_from": null}),
            "{id}"
        );
    }

    // LAST: the latest tick at or before now(), after `evaluated_at`.
    let (old, evaluated, err) = schedule_row(&db, flow, "last").await;
    let before = db_now(&db).await;
    switch(&app, ws, user, flow, "last", false, true).await;
    let after = db_now(&db).await;
    let (next, evaluated_after, err_after) = schedule_row(&db, flow, "last").await;
    let next = next.expect("LAST stays on the queue");
    assert!(
        next <= after && next > before - chrono::Duration::seconds(60) && is_minute(next),
        "LAST: {next} is not the latest tick at or before a now() in [{before}, {after}]"
    );
    assert!(next > old.unwrap(), "LAST moved the date forward");
    assert_eq!((evaluated_after, err_after), (evaluated, err));
    common::audit_mutations(&app);

    // An unknown value with recover_missed: 409, and nothing written.
    let row = schedule_row(&db, flow, "some").await;
    let (status, body) = answer(
        switch_res(
            &app,
            ws,
            user,
            flow,
            "some",
            json!({"disabled": false, "recover_missed": true}),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("recover_missed_unknown"));
    assert_eq!(
        body["error"],
        json!("recoverMissedSchedules is \"SOME\"; egret knows ALL, NONE and LAST")
    );
    assert_eq!(schedule_row(&db, flow, "some").await, row);
    assert!(schedule_copy(&db, flow, "some").await, "still off");
    assert_eq!(common::switched_off(&db, flow).await, vec!["some"]);
    assert!(common::audit_mutations(&app).is_empty());

    assert_eq!(
        executions_of(&db, flow).await,
        0,
        "the switch fires nothing"
    );
}

/// A schedule not yet due keeps its date whatever `recover_missed` says, and
/// a row off the queue stays off.
#[tokio::test]
async fn a_future_date_and_an_off_queue_row_are_kept() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "keep",
            &format!("{}{}", schedule("fut"), schedule("offq")),
        ),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    // An hour ahead: no minute boundary crossed by the test makes it due.
    sql(
        &db,
        "UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) + interval '1 hour' \
          WHERE flow_id = $1 AND trigger_id = 'fut'",
        flow,
    )
    .await;
    sql(
        &db,
        "UPDATE workflow.triggers SET next_evaluation_date = NULL, last_error = 'gone' \
          WHERE flow_id = $1 AND trigger_id = 'offq'",
        flow,
    )
    .await;
    let fut = schedule_row(&db, flow, "fut").await;
    for recover in [false, true] {
        for id in ["fut", "offq"] {
            switch(&app, ws, user, flow, id, true, false).await;
            switch(&app, ws, user, flow, id, false, recover).await;
        }
        assert_eq!(
            schedule_row(&db, flow, "fut").await,
            fut,
            "recover {recover}"
        );
        assert_eq!(
            schedule_row(&db, flow, "offq").await,
            (None, None, Some("gone".to_string())),
            "recover {recover}: an off-queue row stays off"
        );
    }
}

/// The backfill row's dates: `(current_tick, previous_next_evaluation_date)`.
async fn backfill_dates(
    db: &Db,
    flow: Uuid,
    trig: &str,
) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    sqlx::query_as(
        "SELECT current_tick, previous_next_evaluation_date FROM workflow.trigger_backfills \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_one(&db.pool)
    .await
    .expect("a backfill row")
}

/// W7f §3.8 (D11): re-enabling a switched-off schedule during a backfill keeps
/// `next_evaluation_date` (the backfill's `current_tick`) for every choice;
/// the choice moves the regular schedule's resume position instead. The fire
/// that follows is `tests/backfill_routes.rs`'s
/// `a_re_enabled_backfill_fires_where_it_stopped` (a pass is global).
#[tokio::test]
async fn re_enabling_during_a_backfill_keeps_its_place() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let ids: [(&str, Option<&str>, bool); 6] = [
        ("skip", None, false),
        ("all", Some("ALL"), true),
        ("absent", None, true),
        ("last", Some("LAST"), true),
        ("none", Some("NONE"), true),
        ("some", Some("SOME"), true),
    ];
    let triggers: String = ids
        .iter()
        .map(|(id, r, _)| schedule_recovering(id, *r))
        .collect();
    let triggers = format!("{triggers}{}", schedule("ahead"));
    let flow = common::seed_flow(&db, ws, user, &src("company.a", "bf", &triggers), "draft").await;
    let app = flagged_app(&db).await;
    let minute: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT date_trunc('minute', now())")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let (w0, w1) = (
        minute - chrono::Duration::hours(3),
        minute - chrono::Duration::hours(2),
    );
    for (id, _, _) in ids {
        // The regular schedule three ticks overdue, then a window hours back.
        common::make_due(&db, flow, id, 3).await;
        common::stage_backfill(&db, flow, id, (w0, w1), w0, 61, json!({}), json!([])).await;
        switch(&app, ws, user, flow, id, true, false).await;
    }
    // A regular schedule not yet due: an hour ahead.
    sql(
        &db,
        "UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) + interval '1 hour' \
          WHERE flow_id = $1 AND trigger_id = 'ahead'",
        flow,
    )
    .await;
    common::stage_backfill(&db, flow, "ahead", (w0, w1), w0, 61, json!({}), json!([])).await;
    switch(&app, ws, user, flow, "ahead", true, false).await;
    common::audit_mutations(&app);

    // Skip (`false`, or `NONE`): the date is kept; the resume position moves
    // to the first tick after now.
    for (id, recover) in [("skip", false), ("none", true)] {
        let (_, prev) = backfill_dates(&db, flow, id).await;
        assert!(prev <= minute, "{id}: the regular schedule is overdue");
        let before = db_now(&db).await;
        let e = switch(&app, ws, user, flow, id, false, recover).await;
        let after = db_now(&db).await;
        let (next, _, _) = schedule_row(&db, flow, id).await;
        assert_eq!(next, Some(w0), "{id}: the backfill resumes at its tick");
        let (tick, resume) = backfill_dates(&db, flow, id).await;
        assert_eq!(tick, w0, "{id}");
        assert!(
            resume > before && resume <= after + chrono::Duration::seconds(60) && is_minute(resume),
            "{id}: {resume} is not the first tick after a now() in [{before}, {after}]"
        );
        assert_eq!(ts(&e["schedule"]["next_evaluation_date"]), w0, "{id}: {e}");
        assert_eq!(
            ts(&e["backfill"]["previous_next_evaluation_date"]),
            resume,
            "{id}: the answer is what was committed"
        );
        assert_eq!(e["schedule"]["missed_ticks"], json!(null), "{id}: {e}");
        assert_eq!(
            common::audit_mutations(&app),
            vec![(
                "workflow_trigger:enable".to_string(),
                Some(format!("{flow}/{id}")),
                json!({"recover_missed": recover, "next_evaluation_date": null, "backfill_resume_from": resume}),
            )],
            "{id}"
        );
    }

    // Recover (`ALL`, absent, `LAST`): both dates kept; the policy applies
    // when the backfill ends.
    for id in ["all", "absent", "last"] {
        let row = schedule_row(&db, flow, id).await;
        let dates = backfill_dates(&db, flow, id).await;
        let e = switch(&app, ws, user, flow, id, false, true).await;
        assert_eq!(schedule_row(&db, flow, id).await, row, "{id}");
        assert_eq!(backfill_dates(&db, flow, id).await, dates, "{id}");
        assert_eq!(ts(&e["schedule"]["next_evaluation_date"]), w0, "{id}: {e}");
        assert_eq!(
            common::audit_mutations(&app)[0].2,
            json!({"recover_missed": true, "next_evaluation_date": null, "backfill_resume_from": null}),
            "{id}"
        );
    }

    // A skip whose resume position has not passed leaves it, as W7e's skip
    // leaves a date not yet due.
    let dates = backfill_dates(&db, flow, "ahead").await;
    switch(&app, ws, user, flow, "ahead", false, false).await;
    assert_eq!(backfill_dates(&db, flow, "ahead").await, dates);
    assert_eq!(
        common::audit_mutations(&app)[0].2,
        json!({"recover_missed": false, "next_evaluation_date": null, "backfill_resume_from": null})
    );

    // An unknown value: 409, nothing written.
    let row = schedule_row(&db, flow, "some").await;
    let dates = backfill_dates(&db, flow, "some").await;
    let (status, body) = answer(
        switch_res(
            &app,
            ws,
            user,
            flow,
            "some",
            json!({"disabled": false, "recover_missed": true}),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("recover_missed_unknown"));
    assert_eq!(schedule_row(&db, flow, "some").await, row);
    assert_eq!(backfill_dates(&db, flow, "some").await, dates);
    assert_eq!(common::switched_off(&db, flow).await, vec!["some"]);
    assert!(common::audit_mutations(&app).is_empty());
    assert_eq!(
        executions_of(&db, flow).await,
        0,
        "the switch fires nothing"
    );
}

/// A later save does not undo a skip; a new cron re-initialises it.
#[tokio::test]
async fn a_later_save_keeps_a_skip() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let source = src("company.a", "later", &schedule("s"));
    let flow = common::seed_flow(&db, ws, user, &source, "draft").await;
    let app = flagged_app(&db).await;
    common::make_due(&db, flow, "s", 5).await;
    switch(&app, ws, user, flow, "s", true, false).await;
    switch(&app, ws, user, flow, "s", false, false).await;
    let (moved, _, _) = schedule_row(&db, flow, "s").await;

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let save = |source: String| {
        let repo = repo.clone();
        async move {
            repo.update(
                ws,
                flow,
                user,
                &egret_workflow::UpdateFlowInput {
                    source: Some(source),
                    plan: Some(json!({})),
                    ..Default::default()
                },
            )
            .await
            .expect("save")
        }
    };
    save(source.replace("message: hi", "message: edited")).await;
    assert_eq!(schedule_row(&db, flow, "s").await.0, moved, "a task edit");

    save(src(
        "company.a",
        "later",
        &format!("  - id: s\n    type: {SCHEDULE}\n    cron: \"0 0 1 1 *\"\n"),
    ))
    .await;
    let next = schedule_row(&db, flow, "s").await.0.unwrap();
    use chrono::Datelike;
    assert!(
        (next.month(), next.day()) == (1, 1) && next > db_now(&db).await,
        "a new cron re-initialises to its next tick: {next}"
    );
}

#[tokio::test]
async fn the_switch_refusals() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "ref",
            &format!(
                "{}{}{}",
                yaml_disabled_schedule("yoff"),
                leaf("poll"),
                schedule("s")
            ),
        ),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    let on = json!({"disabled": false});
    let off = json!({"disabled": true});

    // A trigger the YAML disables: on is refused, off is accepted.
    let (status, body) = answer(switch_res(&app, ws, user, flow, "yoff", on.clone()).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("trigger_disabled_in_source"));
    assert_eq!(
        body["error"],
        json!("This trigger is disabled in the flow's YAML; remove `disabled: true` there")
    );
    assert!(common::switched_off(&db, flow).await.is_empty());
    switch(&app, ws, user, flow, "yoff", true, false).await;
    assert_eq!(common::switched_off(&db, flow).await, vec!["yoff"]);
    let (status, _) = answer(switch_res(&app, ws, user, flow, "yoff", on.clone()).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(common::switched_off(&db, flow).await, vec!["yoff"], "kept");

    // A Leaf has no switch, either way.
    for body in [&off, &on] {
        let (status, body) =
            answer(switch_res(&app, ws, user, flow, "poll", body.clone()).await).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], json!("trigger_not_supported"));
        assert_eq!(
            body["error"],
            json!("egret does not run this trigger type, so it has no switch")
        );
    }

    // An unknown id.
    assert_eq!(
        switch_res(&app, ws, user, flow, "nope", off.clone())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // The flow's YAML disables it.
    let yaml_off = format!(
        "id: fd\nnamespace: company.a\ndisabled: true\ntriggers:\n{}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        schedule("s")
    );
    let fd = common::seed_flow(&db, ws, user, &yaml_off, "draft").await;
    let (status, body) = answer(switch_res(&app, ws, user, fd, "s", on.clone()).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("trigger_disabled_in_source"));
    assert_eq!(
        body["error"],
        json!("This flow is disabled in its YAML; remove `disabled: true` there")
    );

    // Another workspace's flow: 404, never 403, and nothing written.
    let (other_ws, other_user) = manager(&db).await;
    let theirs = common::seed_flow(
        &db,
        other_ws,
        other_user,
        &src("company.a", "theirs", &schedule("s")),
        "draft",
    )
    .await;
    assert_eq!(
        switch_res(&app, ws, user, theirs, "s", off.clone())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(common::switched_off(&db, theirs).await.is_empty());
    assert!(!schedule_copy(&db, theirs, "s").await);

    // A trashed flow.
    let gone = common::seed_flow(
        &db,
        ws,
        user,
        &src("company.a", "gone", &webhook("h", "k")),
        "draft",
    )
    .await;
    egret_workflow::FlowRepo::new(db.clone())
        .soft_delete(ws, gone)
        .await
        .unwrap();
    assert_eq!(
        switch_res(&app, ws, user, gone, "h", off.clone())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(common::switched_off(&db, gone).await.is_empty());

    // A source that does not parse (as a parser change would leave it).
    let bad: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan) \
         VALUES ($1, 'company.c', 'bad', 'tasks: [unclosed', '{}'::jsonb) RETURNING id",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        switch_res(&app, ws, user, bad, "s", off.clone())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(common::switched_off(&db, bad).await.is_empty());
    assert!(
        common::audit_mutations(&app).len() == 1,
        "only the accepted switch-off of `yoff` was audited"
    );

    // `workflow:read` only: 403, both routes; no session: 401.
    let reader = user_with(&db, ws, &["workflow:read"]).await;
    let app = flagged_app(&db).await;
    assert_eq!(
        switch_res(&app, ws, reader, flow, "s", off.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let bulk = json!({"triggers": [{"flow_id": flow, "trigger_id": "s"}], "disabled": true});
    assert_eq!(
        common::put(&app, &bulk_path(ws), reader, bulk.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        common::anon_put(&app, &switch_path(ws, flow, "s"), off.clone())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        common::anon_put(&app, &bulk_path(ws), bulk).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(common::switched_off(&db, flow).await, vec!["yoff"]);
    assert!(common::audit_mutations(&app).is_empty());
}

#[tokio::test]
async fn switching_is_idempotent() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src("company.a", "idem", &schedule("s")),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    switch(&app, ws, user, flow, "s", true, false).await;
    assert_eq!(common::audit_mutations(&app).len(), 1);
    let e = switch(&app, ws, user, flow, "s", true, false).await;
    assert_eq!(e["runtime_disabled"], json!(true));
    assert_eq!(common::switched_off(&db, flow).await, vec!["s"], "one row");
    assert!(
        common::audit_mutations(&app).is_empty(),
        "off when already off changed nothing: no audit event"
    );
    switch(&app, ws, user, flow, "s", false, false).await;
    assert_eq!(common::audit_mutations(&app).len(), 1);
    let e = switch(&app, ws, user, flow, "s", false, false).await;
    assert_eq!(e["runtime_disabled"], json!(false), "on with no row");
    assert!(
        common::audit_mutations(&app).is_empty(),
        "on when already on changed nothing: no audit event"
    );
    assert!(common::switched_off(&db, flow).await.is_empty());
    assert!(!schedule_copy(&db, flow, "s").await);
}

#[tokio::test]
async fn bulk_switching_is_per_entry_and_bounded() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let many: String = (0..100)
        .map(|i| webhook(&format!("h{i}"), &format!("k{i}")))
        .collect();
    let flow = common::seed_flow(&db, ws, user, &src("company.a", "many", &many), "draft").await;
    let app = flagged_app(&db).await;
    let entries: Vec<Value> = (0..100)
        .map(|i| json!({"flow_id": flow, "trigger_id": format!("h{i}")}))
        .collect();

    // 101: refused before any write.
    let mut over = entries.clone();
    over.push(json!({"flow_id": flow, "trigger_id": "h0"}));
    let (status, body) = answer(
        common::put(
            &app,
            &bulk_path(ws),
            user,
            json!({"triggers": over, "disabled": true}),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "too_many_triggers", "{body}");
    assert_eq!(
        body["error"],
        "101 triggers: a bulk switch takes at most 100; nothing was switched"
    );
    assert!(common::switched_off(&db, flow).await.is_empty());
    assert!(common::audit_mutations(&app).is_empty());

    // Over the body cap: 413.
    let huge = json!({"triggers": [{"flow_id": flow, "trigger_id": "x".repeat(70_000)}], "disabled": true});
    assert_eq!(
        common::put(&app, &bulk_path(ws), user, huge).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    // 100: every one switched, a result each, in order.
    let (status, body) = answer(
        common::put(
            &app,
            &bulk_path(ws),
            user,
            json!({"triggers": entries, "disabled": true}),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 100);
    for (i, r) in results.iter().enumerate() {
        assert_eq!(r["flow_id"], json!(flow), "{r}");
        assert_eq!(r["trigger_id"], json!(format!("h{i}")), "{r}");
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["trigger"]["runtime_disabled"], json!(true), "{r}");
    }
    assert_eq!(common::switched_off(&db, flow).await.len(), 100);
    common::audit_mutations(&app);

    // A mix: one result each, in order; the refusals do not stop the rest.
    let mix = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "mix",
            &format!(
                "{}{}{}",
                schedule("s"),
                leaf("poll"),
                yaml_disabled_schedule("yoff")
            ),
        ),
        "draft",
    )
    .await;
    switch(&app, ws, user, mix, "s", true, false).await;
    common::audit_mutations(&app);
    let (other_ws, other_user) = manager(&db).await;
    let theirs = common::seed_flow(
        &db,
        other_ws,
        other_user,
        &src("company.a", "theirs", &schedule("x")),
        "draft",
    )
    .await;
    common::switch_off(&db, theirs, "x").await;
    let (status, body) = answer(
        common::put(
            &app,
            &bulk_path(ws),
            user,
            json!({
                "triggers": [
                    {"flow_id": theirs, "trigger_id": "x"},
                    {"flow_id": mix, "trigger_id": "poll"},
                    {"flow_id": mix, "trigger_id": "s"},
                    {"flow_id": mix, "trigger_id": "yoff"},
                    {"flow_id": mix, "trigger_id": "nope"},
                ],
                "disabled": false,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Vec<(Value, Value, Value, Value)> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["flow_id"].clone(),
                r["trigger_id"].clone(),
                r["ok"].clone(),
                r["code"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (json!(theirs), json!("x"), json!(false), json!("not_found")),
            (
                json!(mix),
                json!("poll"),
                json!(false),
                json!("trigger_not_supported")
            ),
            (json!(mix), json!("s"), json!(true), json!(null)),
            (
                json!(mix),
                json!("yoff"),
                json!(false),
                json!("trigger_disabled_in_source")
            ),
            (json!(mix), json!("nope"), json!(false), json!("not_found")),
        ]
    );
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[2]["trigger"]["runtime_disabled"], json!(false));
    assert_eq!(results[2]["message"], json!(null));
    assert_eq!(results[1]["trigger"], json!(null));
    assert!(results[1]["message"].is_string());
    assert!(common::switched_off(&db, mix).await.is_empty(), "s is on");
    assert_eq!(
        common::switched_off(&db, theirs).await,
        vec!["x"],
        "another workspace's switch is untouched"
    );
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_trigger:enable".to_string(),
            Some(format!("{mix}/s")),
            json!({"recover_missed": false, "next_evaluation_date": null, "backfill_resume_from": null}),
        )],
        "one event per switched trigger"
    );
}

#[tokio::test]
async fn a_duplicate_id_switches_the_id() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "dup",
            &format!(
                "{}  - id: t\n    type: {SCHEDULE}\n    cron: \"0 * * * *\"\n",
                schedule("t")
            ),
        ),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    let e = switch(&app, ws, user, flow, "t", true, false).await;
    assert_eq!(e["duplicate_id"], json!(true), "{e}");
    assert_eq!(e["runtime_disabled"], json!(true), "{e}");
    assert_eq!(common::switched_off(&db, flow).await, vec!["t"]);
    assert!(schedule_copy(&db, flow, "t").await);
}

/// Enabling a trigger that is already on changes nothing (fix round 1, I1):
/// an overdue schedule that was never switched off is catching up after
/// downtime, or replaying an `ALL` backlog one tick per pass, and a default
/// `recover_missed: false` enable -- a bulk Enable over a mixed selection
/// included -- must not discard that backlog. Nor is an unknown
/// `recoverMissedSchedules` refused there, since no recovery applies (M2).
#[tokio::test]
async fn enabling_an_already_on_schedule_keeps_its_backlog() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "on",
            &format!(
                "{}{}",
                schedule("s"),
                schedule_recovering("odd", Some("SOME"))
            ),
        ),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    common::make_due(&db, flow, "s", 5).await;
    common::make_due(&db, flow, "odd", 5).await;
    let s = schedule_row(&db, flow, "s").await;
    let odd = schedule_row(&db, flow, "odd").await;

    switch(&app, ws, user, flow, "s", false, false).await;
    assert_eq!(
        schedule_row(&db, flow, "s").await,
        s,
        "the backlog stays owed"
    );
    let (status, body) = answer(
        common::put(
            &app,
            &bulk_path(ws),
            user,
            json!({"triggers": [{"flow_id": flow, "trigger_id": "s"}], "disabled": false}),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["ok"], json!(true), "{body}");
    assert_eq!(schedule_row(&db, flow, "s").await, s, "bulk: the same");
    let e = switch(&app, ws, user, flow, "odd", false, true).await;
    assert_eq!(e["runtime_disabled"], json!(false), "{e}");
    assert_eq!(schedule_row(&db, flow, "odd").await, odd);
    assert!(
        common::audit_mutations(&app).is_empty(),
        "nothing changed, nothing audited"
    );
}

/// A skip whose cron has no tick after now goes off the queue with `sync`'s
/// reason (fix round 1, M3). `sync` itself stores such a row off the queue,
/// so the overdue date is staged by SQL, as a row queued before its cron
/// was edited by hand would be.
#[tokio::test]
async fn a_skip_with_no_tick_left_goes_off_the_queue() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src(
            "company.a",
            "never",
            &format!("  - id: s\n    type: {SCHEDULE}\n    cron: \"0 0 30 2 *\"\n"),
        ),
        "draft",
    )
    .await;
    let app = flagged_app(&db).await;
    let (next, _, err) = schedule_row(&db, flow, "s").await;
    assert_eq!(next, None, "sync stores it off the queue");
    let reason = err.expect("with sync's reason");
    sql(
        &db,
        "UPDATE workflow.triggers SET next_evaluation_date = now() - interval '1 minute', \
                last_error = NULL WHERE flow_id = $1",
        flow,
    )
    .await;
    switch(&app, ws, user, flow, "s", true, false).await;
    let e = switch(&app, ws, user, flow, "s", false, false).await;
    let (next, _, err) = schedule_row(&db, flow, "s").await;
    assert_eq!(next, None, "off the queue");
    let err = err.expect("a reason");
    let prefix = "the schedule never fires: `0 0 30 2 *` has no tick after ";
    assert!(
        err.starts_with(prefix) && reason.starts_with(prefix),
        "sync's sentence: {err} / {reason}"
    );
    assert_eq!(e["schedule"]["off_queue"], json!(true), "{e}");
    assert_eq!(e["schedule"]["last_error"], json!(err));
}

/// The body is read after the permission check (the KV bulk delete's
/// pattern), and a refused body answers an `ErrorBody` with its code (fix
/// round 1, M1).
#[tokio::test]
async fn a_bad_body_is_coded_after_the_permission_check() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = manager(&db).await;
    let flow = common::seed_flow(
        &db,
        ws,
        user,
        &src("company.a", "body", &schedule("s")),
        "draft",
    )
    .await;
    let reader = user_with(&db, ws, &["workflow:read"]).await;
    let app = flagged_app(&db).await;
    let bad = json!({"disabled": "yes"});

    for (path, body) in [
        (switch_path(ws, flow, "s"), bad.clone()),
        (bulk_path(ws), json!({"triggers": "s", "disabled": true})),
    ] {
        assert_eq!(
            common::put(&app, &path, reader, body.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{path}: the permission comes first, even with a bad body"
        );
        let (status, got) = answer(common::put(&app, &path, user, body).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {got}");
        assert_eq!(got["code"], json!("invalid_body"), "{got}");
        assert!(
            got["error"]
                .as_str()
                .unwrap()
                .starts_with("the body is not `"),
            "{got}"
        );
        assert_eq!(got.as_object().unwrap().len(), 2, "ErrorBody: {got}");
    }
    let huge = json!({"triggers": [{"flow_id": flow, "trigger_id": "x".repeat(70_000)}], "disabled": true});
    let (status, got) = answer(common::put(&app, &bulk_path(ws), user, huge).await).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(got["code"], json!("body_too_large"), "{got}");
    assert!(common::switched_off(&db, flow).await.is_empty());
}
