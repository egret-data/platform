//! One tick, end to end, against a real database with a fake engine and a fake
//! webhook receiver.
//!
//! The plan's warning about this phase: *a scheduler test that passes because
//! nothing was due*. So every assertion here is on the RUN LOG — what state was
//! written, how many rows — never on the absence of an error.

use std::sync::{Arc, Mutex};

use egret_automation::deliver::destination::{Resolver, SsrfPolicy};
use egret_automation::evaluator::{QueryRunner, RunIdentity};
use egret_automation::render::client::{RenderRequest, Renderer};
use egret_automation::runner::AutomationRunner;
use egret_automation::ScheduleRepo;
use egret_core::tabular::ColumnMeta;
use egret_core::TabularResult;
use egret_metastore::Db;
use uuid::Uuid;

// ── fakes ────────────────────────────────────────────────────────────────

/// An engine that returns one number, and remembers the identity it was given.
struct FakeEngine {
    value: Mutex<serde_json::Value>,
    seen: Mutex<Vec<RunIdentity>>,
    fail: bool,
}

#[async_trait::async_trait]
impl QueryRunner for FakeEngine {
    async fn run(
        &self,
        _db: Uuid,
        _sql: &str,
        identity: &RunIdentity,
        _limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        self.seen.lock().unwrap().push(identity.clone());
        if self.fail {
            return Err("Access Denied: Cannot select from table orders".into());
        }
        let v = self.value.lock().unwrap().clone();
        Ok(TabularResult {
            columns: vec![ColumnMeta {
                name: "v".into(),
                ty: "bigint".into(),
            }],
            rows: vec![vec![v]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }
}

struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

/// This fixture only ever runs the ALERT path (P4's report branch is tested
/// separately in `report_tick.rs`), so the renderer is never actually called
/// — a hard failure here would turn "the report branch ran unexpectedly" into
/// a silent pass instead of a loud one.
struct NeverRenderer;
#[async_trait::async_trait]
impl Renderer for NeverRenderer {
    async fn render(&self, _req: RenderRequest) -> Result<Vec<u8>, String> {
        Err("NeverRenderer: the alert-path fixture must never call the renderer".into())
    }
}

/// Resolves everything to a public address so the SSRF guard admits it; the
/// guard's own behaviour is tested in its module.
struct PublicDns;
#[async_trait::async_trait]
impl Resolver for PublicDns {
    async fn resolve(
        &self,
        _host: &str,
        port: u16,
    ) -> Result<Vec<std::net::SocketAddr>, std::io::Error> {
        Ok(vec![std::net::SocketAddr::new(
            "93.184.216.34".parse().unwrap(),
            port,
        )])
    }
}

// ── fixture ──────────────────────────────────────────────────────────────

struct Fx {
    runner: AutomationRunner,
    db: Db,
    ws: Uuid,
    schedule: Uuid,
    engine: Arc<FakeEngine>,
}

async fn setup(pool: sqlx::PgPool, value: serde_json::Value, fail: bool, webhook: bool) -> Fx {
    let db = Db { pool };
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("o_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // NOT delegated, so the fixture needs no token broker.
    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8080,$2,false) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, database_id, sql, validator_type, \
            grace_period_s) \
         VALUES ($1,'alert','tick test','* * * * *', now() - interval '1 minute', \
                 $2,'SELECT count(*) FROM orders','not_null', 0) RETURNING id",
    )
    .bind(ws)
    .bind(database)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(user)
        .execute(&db.pool)
        .await
        .unwrap();
    if webhook {
        sqlx::query(
            "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
             VALUES ($1,'webhook',$2)",
        )
        .bind(schedule)
        .bind(serde_json::json!({"url": "https://hooks.example.com/x"}))
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let engine = Arc::new(FakeEngine {
        value: Mutex::new(value),
        seen: Mutex::new(vec![]),
        fail,
    });
    let secrets = egret_secrets::SecretStore::new(db.clone(), None);
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    Fx {
        runner: AutomationRunner {
            db: db.clone(),
            schedules: ScheduleRepo::new(
                db.clone(),
                // Crypto-enabled: `SecretStore::new(db, None)` cannot ENCRYPT, so a
                // webhook secret would fail to vault on write.
                egret_secrets::SecretStore::new(
                    db.clone(),
                    Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
                ),
            ),
            databases: egret_query::DatabaseRepo::new(db.clone(), secrets, ws_secrets),
            users: egret_auth::users::UserRepo::new(db.clone()),
            minter: Arc::new(NoMinter),
            query: engine.clone(),
            resolver: Arc::new(PublicDns),
            policy: SsrfPolicy::default(),
            smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
            slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
            charts: egret_bi::ChartRepo::new(db.clone()),
            datasets: egret_bi::DatasetRepo::new(db.clone()),
            render_tokens: egret_automation::render::token::RenderTokenRepo::new(db.clone()),
            renderer: Arc::new(NeverRenderer),
            public_url: "https://egret.internal".into(),
            browser_url: "https://egret.internal".into(),
        },
        db,
        ws,
        schedule,
        engine,
    }
}

async fn run_log(fx: &Fx) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT state, error FROM automation.schedule_runs WHERE schedule_id = $1 \
         ORDER BY started_at",
    )
    .bind(fx.schedule)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap()
}

async fn schedule_state(fx: &Fx) -> (String, Option<chrono::DateTime<chrono::Utc>>) {
    sqlx::query_as("SELECT last_state, next_run_at FROM automation.schedules WHERE id = $1")
        .bind(fx.schedule)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap()
}

// ── tests ────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_firing_alert_is_recorded_as_success_and_rescheduled(pool: sqlx::PgPool) {
    let fx = setup(pool, serde_json::json!(7), false, false).await;

    assert_eq!(fx.runner.tick_once().await, 1, "one schedule was due");

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1, "the tick must write exactly one run row");
    assert_eq!(log[0].0, "success");
    assert_eq!(log[0].1, None, "no error");

    let (state, next) = schedule_state(&fx).await;
    assert_eq!(state, "success", "the row is released, not left `working`");
    assert!(
        next.expect("a next run must be set") > chrono::Utc::now(),
        "a released schedule must be scheduled forward, or it re-runs every tick"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_query_runs_under_the_owners_identity(pool: sqlx::PgPool) {
    // The end-to-end half of §5.2: not just that `owner_identity` builds the
    // right thing, but that the tick actually hands it to the engine.
    let fx = setup(pool, serde_json::json!(1), false, false).await;
    fx.runner.tick_once().await;

    let seen = fx.engine.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the engine was called once");
    assert!(
        seen[0].session_user.is_some(),
        "the engine must be told who this runs as, never an anonymous connection"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_condition_that_is_false_is_recorded_as_noop_not_success(pool: sqlx::PgPool) {
    // `noop` is one of the two answers to "why didn't I get an email?" —
    // collapsing it into success would lose it.
    let fx = setup(pool, serde_json::json!(0), false, false).await;
    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "noop", "zero does not fire a not_null alert");
    assert_eq!(schedule_state(&fx).await.0, "noop");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_engine_refusal_is_recorded_as_error_with_its_message(pool: sqlx::PgPool) {
    // The owner lost access. The run must fail visibly and keep its schedule,
    // rather than silently delivering or silently stopping.
    let fx = setup(pool, serde_json::json!(1), true, false).await;
    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "error");
    assert!(
        log[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("Access Denied"),
        "the engine's own words must reach the run log: {:?}",
        log[0].1
    );

    let (state, next) = schedule_state(&fx).await;
    assert_eq!(state, "error");
    assert!(
        next.expect("still scheduled") > chrono::Utc::now(),
        "a failing alert must keep trying on its schedule, not stop dead"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_tick_inside_the_grace_period_is_suppressed(pool: sqlx::PgPool) {
    // The anti-spam rule. Without it a 1-minute alert on a persistent condition
    // sends 60 mails an hour.
    let fx = setup(pool, serde_json::json!(5), false, false).await;
    sqlx::query("UPDATE automation.schedules SET grace_period_s = 3600 WHERE id = $1")
        .bind(fx.schedule)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    fx.runner.tick_once().await;
    // Make it due again immediately, as a 1-minute crontab would.
    sqlx::query(
        "UPDATE automation.schedules SET next_run_at = now() - interval '1 minute' \
                 WHERE id = $1",
    )
    .bind(fx.schedule)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    let states: Vec<&str> = log.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        states,
        vec!["success", "grace"],
        "the second firing must be suppressed and SAID SO, not silently skipped"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_schedule_with_no_owner_errors_rather_than_running_as_nobody(pool: sqlx::PgPool) {
    // A schedule with no owner has no identity to run as. The dangerous
    // implementation runs it anyway.
    let fx = setup(pool, serde_json::json!(1), false, false).await;
    sqlx::query("DELETE FROM automation.schedule_owners WHERE schedule_id = $1")
        .bind(fx.schedule)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "error");
    assert!(
        log[0].1.as_deref().unwrap_or_default().contains("owner"),
        "the reason must name the missing owner: {:?}",
        log[0].1
    );
    assert!(
        fx.engine.seen.lock().unwrap().is_empty(),
        "the query must never have been sent"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_webhook_that_cannot_be_reached_still_records_the_firing(pool: sqlx::PgPool) {
    // The alert DID fire; only delivery failed. Recording it as `error` would
    // lose the fact that the condition was met, and starting no grace period
    // would re-fire it every minute.
    let fx = setup(pool, serde_json::json!(9), false, true).await;
    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(
        log[0].0, "success",
        "firing is recorded even if delivery is not"
    );
    assert!(
        log[0].1.is_some(),
        "...and the delivery failure is visible rather than swallowed"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn nothing_due_writes_nothing(pool: sqlx::PgPool) {
    // The control for every test above: if the tick wrote rows regardless of
    // what was due, all of them would pass for the wrong reason.
    let fx = setup(pool, serde_json::json!(1), false, false).await;
    sqlx::query(
        "UPDATE automation.schedules SET next_run_at = now() + interval '1 hour' \
                 WHERE id = $1",
    )
    .bind(fx.schedule)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    assert_eq!(fx.runner.tick_once().await, 0);
    assert!(run_log(&fx).await.is_empty());
    assert!(
        fx.engine.seen.lock().unwrap().is_empty(),
        "no query may be sent for a schedule that is not due"
    );
}

// ── run_now (P5 "Run now") ──────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn run_now_writes_a_run_log_row_without_touching_next_run_at_or_last_state(
    pool: sqlx::PgPool,
) {
    // The whole point of `run_now` NOT calling `release`: a manual run must
    // not move the schedule's own cadence. Pin `next_run_at` and `last_state`
    // to known values first, so "unchanged" is checked against something
    // other than NULL.
    let fx = setup(pool, serde_json::json!(7), false, false).await;
    let pinned_next = chrono::Utc::now() + chrono::Duration::hours(3);
    sqlx::query(
        "UPDATE automation.schedules SET next_run_at = $2, last_state = 'noop' WHERE id = $1",
    )
    .bind(fx.schedule)
    .bind(pinned_next)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    let schedule = fx
        .runner
        .schedules
        .get(fx.ws, fx.schedule)
        .await
        .unwrap()
        .expect("schedule exists");
    fx.runner.run_now(&schedule).await;

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1, "a manual run must write a run-log row");
    assert_eq!(log[0].0, "success");

    let (last_state, next_run_at) = schedule_state(&fx).await;
    assert_eq!(
        last_state, "noop",
        "run_now must not touch last_state — that would race a concurrent tick's claim"
    );
    assert_eq!(
        next_run_at,
        Some(pinned_next),
        "run_now must not touch next_run_at, or the schedule's own slot would skip or double-fire"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_now_still_honours_the_grace_period(pool: sqlx::PgPool) {
    // The documented, deliberate choice: a manual run does not bypass
    // anti-spam. If the tick already fired and notified inside the grace
    // window, "Run now" must record `grace`, not resend.
    let fx = setup(pool, serde_json::json!(5), false, false).await;
    sqlx::query("UPDATE automation.schedules SET grace_period_s = 3600 WHERE id = $1")
        .bind(fx.schedule)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    // First firing, via the normal tick.
    fx.runner.tick_once().await;

    // Now press "Run now" while still inside the grace window.
    let schedule = fx
        .runner
        .schedules
        .get(fx.ws, fx.schedule)
        .await
        .unwrap()
        .expect("schedule exists");
    fx.runner.run_now(&schedule).await;

    let log = run_log(&fx).await;
    let states: Vec<&str> = log.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        states,
        vec!["success", "grace"],
        "a manual run inside the grace window must be suppressed and SAID SO, not resent"
    );
}
