//! Z2-3: the `dataapi_alert` dispatch, end to end through `AutomationRunner`.
//!
//! Before this task, `evaluate_and_deliver` recognised only `"report"`
//! explicitly and fell through everything else — including `dataapi_alert` —
//! into the SQL-alert branch, which hit the `NotRunnable("alert is missing
//! its query")` fail-safe at `runner.rs:194-200` (a `dataapi_alert` carries no
//! `database_id`/`sql` by construction; migration 0127's
//! `schedule_dataapi_needs_source` forbids it). These tests prove the FIXED
//! dispatch: a `dataapi_alert` is measured from `dataapi.access_log` via
//! `dataapi_metric::measure`, never touches the SQL engine, and still goes
//! through the SAME `should_fire`/grace/delivery machinery an engine alert
//! does.
//!
//! Structurally a sibling of `tick_end_to_end.rs`'s alert-path fixture, with
//! the SQL-engine pieces (a `query.databases` row, a real `FakeEngine`
//! response) replaced by seeded `dataapi.access_log` rows — a `dataapi_alert`
//! has neither `database_id` nor `sql` to run.

use std::sync::{Arc, Mutex};

use egret_automation::deliver::destination::{Resolver, SsrfPolicy};
use egret_automation::evaluator::{QueryRunner, RunIdentity};
use egret_automation::render::client::{RenderRequest, Renderer};
use egret_automation::runner::AutomationRunner;
use egret_automation::ScheduleRepo;
use egret_core::TabularResult;
use egret_metastore::Db;
use uuid::Uuid;

// ── fakes ────────────────────────────────────────────────────────────────

/// A `dataapi_alert` must never reach the SQL engine — there is no
/// `database_id`/`sql` on the row for it to run. A hard failure here turns
/// "the dispatch fell into the SQL-alert branch by mistake" into a loud test
/// failure instead of a silent pass, and `seen` lets a test assert the engine
/// was never even called.
struct NeverEngine {
    seen: Mutex<Vec<RunIdentity>>,
}

#[async_trait::async_trait]
impl QueryRunner for NeverEngine {
    async fn run(
        &self,
        _db: Uuid,
        _sql: &str,
        identity: &RunIdentity,
        _limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        self.seen.lock().unwrap().push(identity.clone());
        Err("NeverEngine: a dataapi_alert must not run SQL".into())
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

struct NeverRenderer;
#[async_trait::async_trait]
impl Renderer for NeverRenderer {
    async fn render(&self, _req: RenderRequest) -> Result<Vec<u8>, String> {
        Err("NeverRenderer: a dataapi_alert fixture must never call the renderer".into())
    }
}

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

/// Task 4's run-time re-check, without a real CedarPdp or `access_grants`
/// rows: every decision is `allow`, fixed at construction. Good enough for
/// this file's purpose — a single yes/no lets `a_dataapi_alert_whose_owner_
/// lost_endpoint_read_errors_and_sends_nothing` below flip the SAME fixture
/// from `success` to `error` by changing one bool, which is what proves the
/// gate is actually consulted rather than bypassed.
struct FakePdp {
    allow: bool,
}
#[async_trait::async_trait]
impl egret_core::authz::Pdp for FakePdp {
    async fn authorize(
        &self,
        _subject: &egret_core::authz::Subject,
        _action: &str,
        _resource: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: self.allow,
            reasons: vec![],
        })
    }
}

// ── fixture ──────────────────────────────────────────────────────────────

struct Fx {
    runner: AutomationRunner,
    db: Db,
    ws: Uuid,
    schedule: Uuid,
    engine: Arc<NeverEngine>,
}

/// One `dataapi_alert` schedule, due now, plus its owner. `metric` and
/// `window_s` name what it measures; `validator_config` is the `operator`
/// condition — migration 0127's `schedule_dataapi_needs_source` pins
/// `validator_type = 'operator'` for every `dataapi_alert` row, so the
/// fixture never needs to vary it.
async fn setup(
    pool: sqlx::PgPool,
    metric: &str,
    validator_config: serde_json::Value,
    grace_period_s: i32,
    endpoint_read: bool,
) -> Fx {
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

    // No `query.databases` row, no `database_id`/`sql` — a dataapi_alert has
    // neither by construction (schedule_dataapi_needs_source).
    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, validator_type, validator_config, \
            dataapi_metric, dataapi_window_s, grace_period_s) \
         VALUES ($1,'dataapi_alert','dataapi tick test','* * * * *', \
                 now() - interval '1 minute', 'operator', $2, $3, 3600, $4) RETURNING id",
    )
    .bind(ws)
    .bind(validator_config)
    .bind(metric)
    .bind(grace_period_s)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(user)
        .execute(&db.pool)
        .await
        .unwrap();

    let engine = Arc::new(NeverEngine {
        seen: Mutex::new(vec![]),
    });
    let secrets = egret_secrets::SecretStore::new(db.clone(), None);
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    Fx {
        runner: AutomationRunner {
            db: db.clone(),
            schedules: ScheduleRepo::new(
                db.clone(),
                egret_secrets::SecretStore::new(
                    db.clone(),
                    Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
                ),
            ),
            databases: egret_query::DatabaseRepo::new(db.clone(), secrets, ws_secrets),
            users: egret_auth::users::UserRepo::new(db.clone()),
            minter: Arc::new(NoMinter),
            query: engine.clone(),
            pdp: Arc::new(FakePdp {
                allow: endpoint_read,
            }),
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

/// One `dataapi.access_log` row for `fx.ws`, otherwise a healthy, non-trial
/// 200 — mirrors `dataapi_metric.rs`'s own `Row`/`seed`, trimmed to what these
/// tests vary (nothing; every test here needs only "one call happened").
async fn seed_call(fx: &Fx) {
    sqlx::query(
        "INSERT INTO dataapi.access_log \
         (ts, workspace_id, endpoint_id, status_code, duration_ms, actor_user_id, \
          error_code, request_id) \
         VALUES (now(), $1, NULL, 200, 5, NULL, NULL, $2)",
    )
    .bind(fx.ws)
    .bind(Uuid::new_v4().to_string())
    .execute(&fx.db.pool)
    .await
    .unwrap();
}

async fn run_log(fx: &Fx) -> Vec<(String, Option<f64>, Option<String>)> {
    sqlx::query_as(
        "SELECT state, value, error FROM automation.schedule_runs WHERE schedule_id = $1 \
         ORDER BY started_at",
    )
    .bind(fx.schedule)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap()
}

// ── tests ────────────────────────────────────────────────────────────────

/// The dispatch itself (Step 1/2/3). Also the test that dies if the dispatch
/// regresses to the fall-through: `NeverEngine.seen` staying empty is what
/// proves this ran through `dataapi_metric::measure`, not `evaluate`/the SQL
/// path — the fall-through would instead hit `NotRunnable` before ever
/// calling the engine, so this alone would not distinguish the two failure
/// modes; the run-log assertions below do that.
#[sqlx::test(migrations = "../../migrations")]
async fn a_firing_dataapi_alert_dispatches_to_the_metric_evaluator(pool: sqlx::PgPool) {
    let fx = setup(
        pool,
        "call_count",
        serde_json::json!({"op": ">", "threshold": 0}),
        0,
        true,
    )
    .await;
    seed_call(&fx).await;

    assert_eq!(fx.runner.tick_once().await, 1, "one schedule was due");

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1, "the tick must write exactly one run row");
    assert_eq!(
        log[0],
        ("success".to_string(), Some(1.0), None),
        "one call, threshold 0: call_count > 0 must fire"
    );
    assert!(
        fx.engine.seen.lock().unwrap().is_empty(),
        "a dataapi_alert must never reach the SQL engine"
    );
}

/// Step 3's first bullet: `should_fire` already treats a missing measurement
/// as "did not fire" for every `Operator` case (see
/// `validator.rs::a_missing_value_never_fires_whatever_the_operator`) — a
/// `None` measurement must land as `noop`, never as a fired/`success` run.
/// `error_rate` (unlike `call_count`) is `None` over an empty window
/// (`dataapi_metric.rs`'s documented empty-window asymmetry), so this test
/// seeds nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_dataapi_alert_with_no_measurement_is_noop_not_fired(pool: sqlx::PgPool) {
    let fx = setup(
        pool,
        "error_rate",
        serde_json::json!({"op": ">", "threshold": 0}),
        0,
        true,
    )
    .await;
    // Nothing seeded: the window is empty, so error_rate measures None.

    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(
        log[0],
        ("noop".to_string(), None, None),
        "a missing measurement must not fire, and must be recorded as noop, not error"
    );
}

/// Step 4: `in_grace` is called inside the alert path, at `runner.rs:233`. A
/// dispatch that routed a `dataapi_alert` around that call (a separate
/// delivery function that forgot the check, say) would send twice here
/// instead of once.
#[sqlx::test(migrations = "../../migrations")]
async fn a_dataapi_alert_that_fires_twice_inside_grace_delivers_once(pool: sqlx::PgPool) {
    let fx = setup(
        pool,
        "call_count",
        serde_json::json!({"op": ">", "threshold": 0}),
        3600,
        true,
    )
    .await;
    seed_call(&fx).await;

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
    let states: Vec<&str> = log.iter().map(|(s, _, _)| s.as_str()).collect();
    assert_eq!(
        states,
        vec!["success", "grace"],
        "the second firing inside the grace window must be suppressed and SAID SO"
    );
}

/// Task 4's run-time re-check. The owner had `data_api_endpoint:read` when
/// the schedule was saved and lost it afterward (a workspace admin revoking
/// the grant, say) — nothing about the SCHEDULE changed, only the PDP's
/// answer, which is exactly what flipping `endpoint_read` to `false` and
/// nothing else in this fixture simulates. This is the case the brief calls
/// out by name: an engine `alert` gets this re-check for free because the
/// CONNECTOR enforces the owner's access on every run; a `dataapi_alert` runs
/// no query at all, so without this check `alert:manage` would go on serving
/// the endpoint's traffic to an owner who can no longer see the endpoint
/// itself.
#[sqlx::test(migrations = "../../migrations")]
async fn a_dataapi_alert_whose_owner_lost_endpoint_read_errors_and_sends_nothing(
    pool: sqlx::PgPool,
) {
    let fx = setup(
        pool,
        "call_count",
        serde_json::json!({"op": ">", "threshold": 0}),
        0,
        false, // the PDP now refuses `data_api_endpoint:read` for the owner
    )
    .await;
    seed_call(&fx).await;

    assert_eq!(fx.runner.tick_once().await, 1, "one schedule was due");

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1, "the tick must still write a run row");
    let (state, value, error) = &log[0];
    assert_eq!(
        state, "error",
        "a revoked owner must record error, never noop or success"
    );
    assert_eq!(value, &None, "no value is reported once access is refused");
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("no longer read")),
        "the run log must carry the reason, not just \"error\": {error:?}"
    );
    assert!(
        fx.engine.seen.lock().unwrap().is_empty(),
        "still must never reach the SQL engine"
    );
}
