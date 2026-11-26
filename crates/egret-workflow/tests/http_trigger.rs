//! Package 11e Task 3: the `http.Trigger` poll, `responseCondition`, fire and
//! lifecycle, driven through `scheduler::fire_due` against a local stub HTTP
//! server on 127.0.0.1. Requires DATABASE_URL.
//!
//! **The poll runs OUTSIDE the trigger row lock.** `fire_due` claims the row,
//! advances `next_evaluation_date += interval` and commits, then this task's
//! assertions observe the committed advance before the poll's own effects (the
//! execution, or `last_error`).
//!
//! **The tests in this binary run one at a time** (`SERIAL`) and each starts by
//! taking every trigger an earlier test left behind off the queue -- a pass is
//! global. The binary owns its database (`egret_test_db`), and installs ONE
//! process-wide egress policy with `allow_hosts = ["127.0.0.1"]`, so a stub on
//! any loopback port is reachable while a non-allowlisted host is blocked.
//!
//! "Due" is judged against Postgres's `now()`, so a test makes a trigger due by
//! writing its date into the past (`set_due`), never by moving a Rust clock.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::scheduler::{fire_due, PassReport};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const HTTP_TRIGGER: &str = "io.kestra.plugin.core.http.Trigger";

// ---- the local stub server --------------------------------------------------

struct Server {
    addr: SocketAddr,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.addr.port())
    }
}

async fn serve() -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/ok",
            get(|| async { ([("x-test", "yes")], r#"{"ok":true}"#) }),
        )
        .route(
            "/boom",
            get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { addr }
}

// ---- the fixture ------------------------------------------------------------

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    // A pass is global; start from an empty queue.
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = NULL")
        .execute(&db.pool)
        .await
        .unwrap();
    // The one process-wide egress the scheduler's poll reads. Loopback is
    // refused by the guard unless allow-listed, so 127.0.0.1 is listed; a
    // private address NOT listed (test e) is blocked.
    let cfg = egret_config::WorkflowHttpConfig {
        allow_hosts: vec!["127.0.0.1".into()],
        ..Default::default()
    };
    let egress =
        egret_workflow::http::HttpEgress::from_config(&cfg, Arc::new(egret_egress::SystemResolver));
    let _ = egret_workflow::http::install_egress(egress);
    let (ws, user) = common::seed_workspace(&db).await;
    Some(Fx {
        db,
        ws,
        user,
        _serial: serial,
    })
}

impl Fx {
    /// A flow with one `http.Trigger` `s`. Parsed directly: publish still
    /// refuses the type until the package's reconcile task.
    async fn flow(&self, id: &str, uri: &str, extra: &str) -> Uuid {
        let src = format!(
            "id: {id}\nnamespace: http11e\ntriggers:\n  - id: s\n    type: {HTTP_TRIGGER}\n    uri: {uri}\n{extra}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
        );
        egret_workflow::spec::parse(&src)
            .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"));
        common::seed_flow(&self.db, self.ws, self.user, &src, "draft").await
    }

    async fn pass(&self) -> PassReport {
        fire_due(&self.db.pool).await.expect("a pass")
    }
}

/// Make the trigger due now (its date into the past).
async fn set_due(db: &Db, flow: Uuid) {
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = now() - interval '1 minute' \
         WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn next_date(db: &Db, flow: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar(
        "SELECT next_evaluation_date FROM workflow.triggers \
         WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn last_error(db: &Db, flow: Uuid) -> Option<String> {
    sqlx::query_scalar(
        "SELECT last_error FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// The one execution's stored `labels` array (`[{key,value}]`), or `None`.
async fn labels_of(db: &Db, flow: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT labels FROM workflow.executions WHERE flow_id = $1")
        .bind(flow)
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

/// The one execution's `(execution_trigger, scheduled_date, state)`. `state` is
/// the denormalised last wire state ("RUNNING", "FAILED", ...), a plain word.
async fn only_execution(db: &Db, flow: Uuid) -> (serde_json::Value, Option<DateTime<Utc>>, String) {
    sqlx::query_as(
        "SELECT execution_trigger, scheduled_date, state \
         FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// How many task runs the one execution has (a born-failed one has none).
async fn task_runs_of(db: &Db, flow: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs r \
           JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE e.flow_id = $1",
    )
    .bind(flow)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

// ---- (a) truthy responseCondition fires with trigger.* bound ----------------

#[tokio::test]
async fn an_ok_endpoint_fires_one_execution_with_the_response_bound_under_trigger() {
    let Some(fx) = fx().await else { return };
    let server = serve().await;
    let uri = server.url("/ok");
    let flow = fx
        .flow("ok-fires", &uri, "    allowConcurrent: true\n")
        .await;
    set_due(&fx.db, flow).await;

    let r = fx.pass().await;
    assert_eq!((r.fired, r.errored, r.held), (1, 0, 0), "{r:?}");
    assert_eq!(executions_of(&fx.db, flow).await, 1);

    let (trigger, scheduled, _state) = only_execution(&fx.db, flow).await;
    assert_eq!(scheduled, None, "a poll is not a date tick");
    assert_eq!(trigger["id"], "s");
    assert_eq!(trigger["type"], HTTP_TRIGGER);
    // 11a's `{{ trigger.* }}` binding reads `variables.*`.
    assert_eq!(trigger["variables"]["uri"], uri);
    assert_eq!(trigger["variables"]["code"], 200);
    assert_eq!(trigger["variables"]["body"], r#"{"ok":true}"#);
    assert_eq!(trigger["variables"]["headers"]["x-test"][0], "yes");

    // The interval advanced (the tick is consumed); the next pass is a no-op.
    assert!(next_date(&fx.db, flow).await.unwrap() > Utc::now());
}

// ---- (b) falsy responseCondition does not fire, interval advanced -----------

#[tokio::test]
async fn a_500_is_falsy_under_the_default_condition_and_does_not_fire() {
    let Some(fx) = fx().await else { return };
    let server = serve().await;
    let flow = fx
        .flow(
            "boom-skips",
            &server.url("/boom"),
            "    allowConcurrent: true\n",
        )
        .await;
    set_due(&fx.db, flow).await;

    let r = fx.pass().await;
    assert_eq!((r.fired, r.errored), (0, 0), "{r:?}");
    assert_eq!(r.skipped, 1, "a silent no-fire: {r:?}");
    assert_eq!(executions_of(&fx.db, flow).await, 0);
    // Advanced anyway, no error.
    assert!(next_date(&fx.db, flow).await.unwrap() > Utc::now());
    assert_eq!(last_error(&fx.db, flow).await, None);
}

// ---- (c) failOnTriggerError + transport error -> born-failed execution ------

#[tokio::test]
async fn a_transport_error_with_fail_on_trigger_error_starts_a_born_failed_execution() {
    let Some(fx) = fx().await else { return };
    // Nothing listens on 127.0.0.1:1 -> connection refused (a transport error,
    // not a bad status). 127.0.0.1 is allow-listed, so the guard lets it try.
    let flow = fx
        .flow(
            "refused-fails",
            "http://127.0.0.1:1/",
            "    failOnTriggerError: true\n    allowConcurrent: true\n",
        )
        .await;
    set_due(&fx.db, flow).await;

    let r = fx.pass().await;
    assert_eq!(r.errored, 1, "{r:?}");
    assert_eq!(
        executions_of(&fx.db, flow).await,
        1,
        "a born-failed execution"
    );
    let (trigger, scheduled, state) = only_execution(&fx.db, flow).await;
    assert_eq!(scheduled, None);
    assert_eq!(trigger["id"], "s");
    assert_eq!(state, "FAILED");
    assert_eq!(task_runs_of(&fx.db, flow).await, 0, "no task run");
}

// ---- (d) allowConcurrent:false + a live execution -> the poll is held -------

#[tokio::test]
async fn a_live_execution_holds_the_poll_when_concurrency_is_off() {
    let Some(fx) = fx().await else { return };
    let server = serve().await;
    // Default allowConcurrent is false (Kestra's default); omit it.
    let flow = fx.flow("held", &server.url("/ok"), "").await;

    // First poll fires one execution, which no worker drives, so it stays live.
    set_due(&fx.db, flow).await;
    let r = fx.pass().await;
    assert_eq!(r.fired, 1, "{r:?}");
    assert_eq!(executions_of(&fx.db, flow).await, 1);

    // Second poll: the prior execution is still live, so the tick is HELD and
    // no second execution is created.
    set_due(&fx.db, flow).await;
    let r = fx.pass().await;
    assert_eq!((r.fired, r.held), (0, 1), "{r:?}");
    assert_eq!(
        executions_of(&fx.db, flow).await,
        1,
        "no second fire while live"
    );
}

// ---- (f) the trigger's base `when` gates the fire and sees the response -----

#[tokio::test]
async fn the_base_when_gates_the_fire_and_reads_the_response() {
    let Some(fx) = fx().await else { return };
    let server = serve().await;

    // A falsy `when` against a 200: `responseCondition` is truthy (200 < 400),
    // but the base `when` -- evaluated with `response` bound -- is not, so
    // NOTHING fires. A silent no-fire: the interval advanced, no error.
    let gated = fx
        .flow(
            "when-gated",
            &server.url("/ok"),
            "    allowConcurrent: true\n    when: \"{{ response.statusCode == 500 }}\"\n",
        )
        .await;
    set_due(&fx.db, gated).await;
    let r = fx.pass().await;
    assert_eq!((r.fired, r.errored), (0, 0), "{r:?}");
    assert_eq!(r.skipped, 1, "when gated the fire out: {r:?}");
    assert_eq!(executions_of(&fx.db, gated).await, 0);
    assert!(next_date(&fx.db, gated).await.unwrap() > Utc::now());
    assert_eq!(
        last_error(&fx.db, gated).await,
        None,
        "no error, just no fire"
    );

    // A truthy `when` reading the same response: fires one execution.
    let fires = fx
        .flow(
            "when-fires",
            &server.url("/ok"),
            "    allowConcurrent: true\n    when: \"{{ response.statusCode == 200 }}\"\n",
        )
        .await;
    set_due(&fx.db, fires).await;
    let r = fx.pass().await;
    assert_eq!(r.fired, 1, "a truthy when fires: {r:?}");
    assert_eq!(executions_of(&fx.db, fires).await, 1);
}

// ---- (g) a templated label is rendered against the fire context -------------

#[tokio::test]
async fn a_templated_label_is_rendered_against_the_fire_context() {
    let Some(fx) = fx().await else { return };
    let server = serve().await;
    // `{{ trigger.code }}` is the polled status (11a's binding); the label value
    // must be the RENDERED "200", not the literal template.
    let flow = fx
        .flow(
            "labelled",
            &server.url("/ok"),
            "    allowConcurrent: true\n    labels:\n      status: \"{{ trigger.code }}\"\n",
        )
        .await;
    set_due(&fx.db, flow).await;

    let r = fx.pass().await;
    assert_eq!(r.fired, 1, "{r:?}");
    assert_eq!(
        labels_of(&fx.db, flow).await,
        Some(serde_json::json!([{"key": "status", "value": "200"}])),
        "the templated label is rendered"
    );
}

// ---- (e) a non-allowlisted host is blocked; failOnTriggerError governs ------

#[tokio::test]
async fn a_blocked_host_does_not_fire_and_is_recorded_without_fail_on_trigger_error() {
    let Some(fx) = fx().await else { return };
    // A private address not on the allowlist: the egress guard blocks it before
    // any connection. failOnTriggerError is false (default), so no execution.
    let flow = fx
        .flow(
            "blocked",
            "http://10.255.255.1/health",
            "    allowConcurrent: true\n",
        )
        .await;
    set_due(&fx.db, flow).await;

    let r = fx.pass().await;
    assert_eq!(r.errored, 1, "{r:?}");
    assert_eq!(executions_of(&fx.db, flow).await, 0, "no fire");
    // The interval still advanced, and the reason is kept.
    assert!(next_date(&fx.db, flow).await.unwrap() > Utc::now());
    assert!(
        last_error(&fx.db, flow).await.is_some(),
        "the block is recorded"
    );
}
