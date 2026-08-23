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
use egret_automation::deliver::smtp_config::{SmtpConfigBody, TlsMode};
use egret_automation::evaluator::{QueryRunner, RunIdentity};
use egret_automation::render::client::{RenderRequest, Renderer};
use egret_automation::runner::AutomationRunner;
use egret_automation::ScheduleRepo;
use egret_core::TabularResult;
use egret_metastore::Db;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
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

/// A `Resource`'s decision-relevant fields, comparable and ownable — unlike
/// `egret_core::authz::Resource` itself, which derives neither `PartialEq`
/// nor anything that would let a test assert what it was.
#[derive(Debug, Clone, PartialEq)]
enum SeenResource {
    Workspace(Uuid),
    Object { kind: String, id: Uuid },
    Platform,
}
impl From<&egret_core::authz::Resource> for SeenResource {
    fn from(r: &egret_core::authz::Resource) -> Self {
        match r {
            egret_core::authz::Resource::Workspace(w) => SeenResource::Workspace(w.0),
            egret_core::authz::Resource::Object(o) => SeenResource::Object {
                kind: o.kind.clone(),
                id: o.id,
            },
            egret_core::authz::Resource::Platform => SeenResource::Platform,
        }
    }
}

/// Task 4's run-time re-check, without a real CedarPdp or `access_grants`
/// rows: every decision is `allow`, fixed at construction — but every call is
/// RECORDED (subject, action, resource), so a test can assert not just "was
/// the PDP consulted" but "was it asked the right question". A `FakePdp` that
/// only ever returned a bool (this file's earlier version) said `ok` for a
/// runner that asked about the wrong action, the wrong resource kind, or —
/// the one that matters — the wrong SUBJECT (`s.created_by`, say, instead of
/// the resolved owner, or the tick's own service identity); recording all
/// three and asserting on them in the tests below closes that. A single
/// yes/no answer still lets `a_dataapi_alert_whose_owner_lost_endpoint_read_
/// errors_and_sends_nothing` below flip the SAME fixture from `success` to
/// `error` by changing one bool.
struct FakePdp {
    allow: bool,
    calls: Mutex<Vec<(Uuid, String, SeenResource)>>,
}
#[async_trait::async_trait]
impl egret_core::authz::Pdp for FakePdp {
    async fn authorize(
        &self,
        subject: &egret_core::authz::Subject,
        action: &str,
        resource: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        self.calls.lock().unwrap().push((
            subject.user_id.0,
            action.to_string(),
            SeenResource::from(resource),
        ));
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
    /// The schedule's OWNER — the identity `owner_can_read_dataapi_endpoint`
    /// must ask the PDP about. Exposed so a test can assert `fx.pdp.calls`
    /// named THIS user, not `s.created_by` (left `NULL` by this fixture's own
    /// `INSERT`, so a subject mix-up would show up as `Uuid::nil()`, never as
    /// a coincidental match) and not some other identity entirely.
    owner: Uuid,
    /// The same `Arc` `runner.pdp` holds, kept as the concrete type so a test
    /// can read `.calls` — `AutomationRunner::pdp` is `Arc<dyn Pdp>`, which
    /// erases that.
    pdp: Arc<FakePdp>,
    /// `Some` for `setup_endpoint_scoped` (Task 4 review, Finding 5); `None`
    /// for the plain `setup` above, whose schedule watches the whole
    /// workspace.
    endpoint_id: Option<Uuid>,
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
    let pdp = Arc::new(FakePdp {
        allow: endpoint_read,
        calls: Mutex::new(vec![]),
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
            pdp: pdp.clone(),
            dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
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
        owner: user,
        pdp,
        endpoint_id: None,
    }
}

/// Like [`setup`], but the schedule watches ONE endpoint
/// (`dataapi_endpoint_id`), not the whole workspace — the OBJECT-scoped
/// branch of `owner_can_read_dataapi_endpoint`'s re-check (Task 4 review,
/// Finding 5). Migration 0121's NOT NULL columns (`name`/`path`/`sql`/
/// `database_id`) need a real `query.databases` row, which `setup`'s
/// schedule deliberately has none of (a `dataapi_alert` carries no
/// `database_id`/`sql` itself — `schedule_dataapi_needs_source` — but the
/// ENDPOINT it watches is a normal endpoint row and needs one).
async fn setup_endpoint_scoped(
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
    let database_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints (workspace_id, name, path, sql, database_id, created_by) \
         VALUES ($1,'z2 tick test','/z2-tick-test','SELECT 1',$2,$3) RETURNING id",
    )
    .bind(ws)
    .bind(database_id)
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, validator_type, validator_config, \
            dataapi_metric, dataapi_window_s, grace_period_s, dataapi_endpoint_id) \
         VALUES ($1,'dataapi_alert','dataapi tick test (endpoint-scoped)','* * * * *', \
                 now() - interval '1 minute', 'operator', $2, $3, 3600, $4, $5) RETURNING id",
    )
    .bind(ws)
    .bind(validator_config)
    .bind(metric)
    .bind(grace_period_s)
    .bind(endpoint_id)
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
    let pdp = Arc::new(FakePdp {
        allow: endpoint_read,
        calls: Mutex::new(vec![]),
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
            pdp: pdp.clone(),
            dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
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
        owner: user,
        pdp,
        endpoint_id: Some(endpoint_id),
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

    // The run-time re-check (Task 4) asked the PDP about the right question:
    // the schedule's OWNER (not `s.created_by`, left `NULL` by this fixture,
    // and not some other identity), `data_api_endpoint:read` (not, say,
    // `alert:manage` — the permission `run_now`/`update` check, a plausible
    // copy-paste), against the WORKSPACE resource (this fixture's row has no
    // `dataapi_endpoint_id`, so `None` means every-endpoint-in-the-workspace,
    // never an object resource with a made-up kind).
    let calls = fx.pdp.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "expected exactly one PDP call: {calls:?}");
    assert_eq!(
        calls[0],
        (
            fx.owner,
            "data_api_endpoint:read".to_string(),
            SeenResource::Workspace(fx.ws),
        ),
        "the PDP was asked about the wrong subject, action, or resource"
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

    // Same pinning as the firing test above: the refusal must be an answer
    // to the RIGHT question, not a fixed `allow: false` that would refuse
    // regardless of what (or who) was asked.
    let calls = fx.pdp.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "expected exactly one PDP call: {calls:?}");
    assert_eq!(
        calls[0],
        (
            fx.owner,
            "data_api_endpoint:read".to_string(),
            SeenResource::Workspace(fx.ws),
        ),
        "the PDP was asked about the wrong subject, action, or resource"
    );
}

/// The OBJECT-scoped half of the same pinning (Task 4 review, Finding 5):
/// every fixture above sets `dataapi_endpoint_id IS NULL`, so
/// `owner_can_read_dataapi_endpoint`'s `Resource::Object` branch — the one
/// that builds `kind: egret_dataapi_api::OBJECT_KIND` — was never reached by
/// a single tick test. A wrong kind there would ship green exactly the way a
/// wrong kind in the ROUTE's object branch did before Finding 3's positive
/// control. `setup_endpoint_scoped` gives the schedule a real
/// `dataapi_endpoint_id`; this test asserts the PDP was asked about THAT
/// object, with THAT kind — the literal `"data_api_endpoint"`, not the
/// constant the code under test also reads, so a typo in either place shows
/// up as a mismatch here.
#[sqlx::test(migrations = "../../migrations")]
async fn the_endpoint_scoped_re_check_asks_about_the_pinned_object_kind(pool: sqlx::PgPool) {
    let fx = setup_endpoint_scoped(
        pool,
        "call_count",
        serde_json::json!({"op": ">", "threshold": 0}),
        0,
        true,
    )
    .await;

    assert_eq!(fx.runner.tick_once().await, 1, "one schedule was due");

    let calls = fx.pdp.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "expected exactly one PDP call: {calls:?}");
    assert_eq!(
        calls[0],
        (
            fx.owner,
            "data_api_endpoint:read".to_string(),
            SeenResource::Object {
                kind: "data_api_endpoint".to_string(),
                id: fx.endpoint_id.expect("setup_endpoint_scoped sets this"),
            },
        ),
        "the PDP was asked about the wrong subject, action, or resource"
    );
}

/// Fix-round-3: a `dataapi_alert` whose `dataapi_endpoint_id` names an
/// endpoint in a DIFFERENT workspace than the schedule's own — reachable only
/// by writing the row directly, since BOTH write paths refuse this shape as
/// of fix-round-2 — must tick to an `error` run that sends nothing, never a
/// measurement of the foreign workspace's traffic.
///
/// `endpoint_read: true` in `setup_endpoint_scoped` below is the point: the
/// PDP is set up to ALLOW anything it is asked. If `owner_can_read_dataapi_
/// endpoint`'s new workspace check were missing (or ran after the PDP call
/// rather than before it), this fixture would sail through the PDP and go on
/// to measure `dataapi.access_log` — and since `seed_call` only ever writes
/// into `fx.ws`'s log, never the foreign workspace's, that misconfiguration
/// would surface as a wrongly-fired `noop`/`success`, not as `error`. The
/// `fx.pdp.calls` assertion at the end is the direct proof: the PDP must
/// never be consulted at all once the workspace mismatch is caught.
#[sqlx::test(migrations = "../../migrations")]
async fn a_dataapi_alert_watching_a_foreign_workspace_endpoint_errors_and_sends_nothing(
    pool: sqlx::PgPool,
) {
    let fx = setup_endpoint_scoped(
        pool,
        "call_count",
        serde_json::json!({"op": ">", "threshold": 0}),
        0,
        true, // the PDP would ALLOW -- the workspace check must refuse first
    )
    .await;

    // A real endpoint in a DIFFERENT, unrelated workspace.
    let foreign_owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("fo_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    let foreign_ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(foreign_owner)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let foreign_database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(foreign_ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let foreign_endpoint: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints (workspace_id, name, path, sql, database_id, created_by) \
         VALUES ($1,'foreign','/foreign','SELECT 1',$2,$3) RETURNING id",
    )
    .bind(foreign_ws)
    .bind(foreign_database)
    .bind(foreign_owner)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();

    // Repoint the fixture's (already valid, same-workspace) schedule at the
    // FOREIGN endpoint by writing the row directly — the routes refuse this
    // shape as of fix-round-2, so direct SQL is the only way it can exist.
    sqlx::query("UPDATE automation.schedules SET dataapi_endpoint_id = $1 WHERE id = $2")
        .bind(foreign_endpoint)
        .bind(fx.schedule)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    seed_call(&fx).await;

    assert_eq!(fx.runner.tick_once().await, 1, "one schedule was due");

    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1, "the tick must still write a run row");
    let (state, value, error) = &log[0];
    assert_eq!(
        state, "error",
        "a foreign-workspace endpoint must record error, never noop or success"
    );
    assert_eq!(value, &None, "no value is reported once access is refused");
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("no longer read")),
        "the run log must carry the same reason a revoked owner gets: {error:?}"
    );
    assert!(
        fx.engine.seen.lock().unwrap().is_empty(),
        "must never reach the SQL engine"
    );

    let calls = fx.pdp.calls.lock().unwrap();
    assert!(
        calls.is_empty(),
        "the PDP must never be consulted once the workspace mismatch is caught: {calls:?}"
    );
}

// ── send_email's CSV skip (re-review finding 2) ─────────────────────────────

/// A minimal, single-connection SMTP server double — just enough of RFC 5321
/// (EHLO/MAIL FROM/RCPT TO/DATA/QUIT) for `lettre`'s plaintext transport
/// (`TlsMode::None`, the same "trusted relay or mail catcher" branch
/// `email.rs`'s own tests build) to complete a real send against it. The
/// captured `DATA` payload — the raw MIME the mail actually carries — is
/// returned over the oneshot channel.
///
/// Built for this test specifically: `send_email` (runner.rs) is a private
/// method with no injectable transport, so pinning what it attaches needs a
/// real peer on the other end of `email::send`'s SMTP connection, not a mock
/// of a trait. No fixture in this file offered that before — every other
/// tick test here leaves SMTP unconfigured, so `deliver`'s `email_to.is_empty()`
/// guard short-circuits before `send_email` is ever called (see runner.rs's
/// `a_dataapi_alert_body_says_nothing_about_attached_rows`, which says this
/// explicitly).
async fn fake_smtp() -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        if writer.write_all(b"220 mock.local ESMTP\r\n").await.is_err() {
            return;
        }
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let upper = line.trim_end().to_ascii_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") {
                b"250 mock.local\r\n"
            } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                b"250 OK\r\n"
            } else if upper.starts_with("QUIT") {
                let _ = writer.write_all(b"221 Bye\r\n").await;
                return;
            } else if upper.starts_with("DATA") {
                if writer
                    .write_all(b"354 Send message, end with <CRLF>.<CRLF>\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                let mut data = Vec::new();
                let mut dline = String::new();
                loop {
                    dline.clear();
                    match reader.read_line(&mut dline).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    if dline == ".\r\n" {
                        break;
                    }
                    data.extend_from_slice(dline.as_bytes());
                }
                if let Some(tx) = tx.take() {
                    let _ = tx.send(data);
                }
                b"250 OK: queued\r\n"
            } else {
                b"500 unrecognized\r\n"
            };
            if writer.write_all(reply).await.is_err() {
                return;
            }
        }
    });
    (port, rx)
}

/// Pins the fix wave's finding 1 mechanism (the `dataapi_alert` CSV skip in
/// `send_email`) by driving it end to end: a real `tick_once()`, through
/// `deliver`, into `send_email`, out over a real (fake) SMTP connection.
///
/// The re-reviewer's mutation — `if s.r#type != "dataapi_alert"` flipped to
/// `if true`, so a CSV is always built — left all 238 existing tests green,
/// because none of them let `send_email` run this far. This test's fixture
/// configures a real (loopback) SMTP relay and an email recipient
/// specifically so that mutation now has something to fail: with it applied,
/// the mail would carry a fabricated one-column `column_1.csv` attachment
/// (`send_email`'s positional-header fallback, built from `dataapi_alert`'s
/// always-empty `Evaluation.columns`) even though a `dataapi_alert` has no
/// rows at all.
#[sqlx::test(migrations = "../../migrations")]
async fn a_dataapi_alert_email_carries_no_csv_attachment(pool: sqlx::PgPool) {
    let fx = setup(
        pool,
        "call_count",
        serde_json::json!({"op": ">", "threshold": 0}),
        0,
        true,
    )
    .await;
    seed_call(&fx).await;

    sqlx::query(
        "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
         VALUES ($1, 'email', $2)",
    )
    .bind(fx.schedule)
    .bind(serde_json::json!({"addresses": ["oncall@example.com"]}))
    .execute(&fx.db.pool)
    .await
    .unwrap();

    let (port, rx) = fake_smtp().await;
    fx.runner
        .smtp
        .upsert(
            fx.ws,
            &SmtpConfigBody {
                host: "127.0.0.1".into(),
                port: port as i32,
                username: None,
                from_address: "alerts@egret.local".into(),
                from_name: None,
                tls_mode: TlsMode::None,
                password: None,
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(fx.runner.tick_once().await, 1, "one schedule was due");

    // A delivery failure would land as an "error" run rather than "success" —
    // asserting this first is what makes the CSV assertion below trustworthy:
    // it proves send_email ran to completion against the fake relay, rather
    // than failing before ever reaching the attachment-building code.
    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1, "the tick must write exactly one run row");
    assert_eq!(
        log[0].0, "success",
        "delivery through the fake relay must succeed, or the assertions below prove nothing: {:?}",
        log[0]
    );

    let raw = rx
        .await
        .expect("the fake SMTP server never received a DATA payload");
    let raw = String::from_utf8_lossy(&raw);
    assert!(
        !raw.to_lowercase().contains("text/csv"),
        "a dataapi_alert has no rows to attach; the mail must carry no CSV part:\n{raw}"
    );
    assert!(
        !raw.contains(".csv"),
        "a dataapi_alert's mail must not name a csv attachment:\n{raw}"
    );
}
