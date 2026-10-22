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

// `trunc_subsecs`: see `run_now_records_its_outcome_without_touching_next_run_at`.
use chrono::SubsecRound as _;

// ── fakes ────────────────────────────────────────────────────────────────

/// An engine that returns one number, and remembers the identity it was given.
struct FakeEngine {
    value: Mutex<serde_json::Value>,
    seen: Mutex<Vec<RunIdentity>>,
    fail: bool,
    /// How long each query takes — a slow engine or render (KI-173).
    delay: std::time::Duration,
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
        tokio::time::sleep(self.delay).await;
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

/// Task 4's run-time re-check only runs on the `dataapi_alert` path — this
/// file's schedules are all `type = 'alert'` — so a `panic!` here turns "the
/// engine-alert path started calling the PDP" into a loud test failure
/// instead of a silent one, the same way `NeverRenderer` above does for the
/// renderer.
struct NeverPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for NeverPdp {
    async fn authorize(
        &self,
        _subject: &egret_core::authz::Subject,
        _action: &str,
        _resource: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        panic!("an engine alert must never consult the PDP");
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

/// Resolves EVERY host to one fixed address — the loopback port a
/// `fake_webhook_receiver` is actually listening on. Used only together with
/// `SsrfPolicy::allow_hosts` naming that same fake host, so this never
/// widens what the guard itself allows; it only lets a test point a
/// plausible-looking public hostname at a local listener.
struct LoopbackDns(std::net::SocketAddr);
#[async_trait::async_trait]
impl Resolver for LoopbackDns {
    async fn resolve(
        &self,
        _host: &str,
        _port: u16,
    ) -> Result<Vec<std::net::SocketAddr>, std::io::Error> {
        Ok(vec![self.0])
    }
}

/// A minimal HTTP/1.1 receiver — accepts one connection, records whatever
/// bytes it read, replies `200`. Enough to prove a webhook POST actually
/// arrived, not a general-purpose server.
async fn fake_webhook_receiver() -> (std::net::SocketAddr, Arc<Mutex<Vec<u8>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let received2 = received.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 65536];
            if let Ok(n) = stream.read(&mut buf).await {
                received2.lock().unwrap().extend_from_slice(&buf[..n]);
            }
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });
    (addr, received)
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
        delay: std::time::Duration::ZERO,
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
            pdp: Arc::new(NeverPdp),
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
            dashboards: egret_bi::DashboardRepo::new(db.clone()),
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

/// Point the fixture's webhook at a loopback address, which the SSRF guard
/// refuses — a delivery failure that needs no network.
fn with_blocked_webhook(fx: &mut Fx) {
    fx.runner.resolver = Arc::new(LoopbackDns("127.0.0.1:9".parse().unwrap()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_firing_whose_delivery_failed_is_an_error_and_starts_no_grace(pool: sqlx::PgPool) {
    // KI-033: the alert fired but nobody was told. That used to be `success`
    // — and start the grace period — with the failure only in the detail.
    let mut fx = setup(pool, serde_json::json!(9), false, true).await;
    with_blocked_webhook(&mut fx);
    sqlx::query("UPDATE automation.schedules SET grace_period_s = 3600 WHERE id = $1")
        .bind(fx.schedule)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    fx.runner.tick_once().await;
    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "error", "an undelivered firing is not a success");
    let detail = log[0].1.as_deref().unwrap_or_default();
    assert!(
        detail.starts_with("webhook hooks.example.com: ") && detail.contains("blocked"),
        "the failure names its destination: {detail}"
    );
    assert_eq!(
        schedule_state(&fx).await.0,
        "error",
        "the list shows it as failing"
    );

    // Nobody was told, so the next tick tries again instead of going quiet.
    sqlx::query(
        "UPDATE automation.schedules SET next_run_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(fx.schedule)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    fx.runner.tick_once().await;
    let states: Vec<String> = run_log(&fx).await.into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        states,
        vec!["error", "error"],
        "no grace after a failed delivery"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_failed_destination_is_recorded_not_only_the_first(pool: sqlx::PgPool) {
    // KI-034: email (no SMTP in this workspace) + webhook (blocked) + Slack
    // (no app): the run log used to show only the webhook's error.
    let mut fx = setup(pool, serde_json::json!(9), false, true).await;
    with_blocked_webhook(&mut fx);
    for (kind, config) in [
        ("email", serde_json::json!({"addresses": ["a@example.com"]})),
        ("slack", serde_json::json!({"channel": "#alerts"})),
    ] {
        sqlx::query(
            "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
             VALUES ($1,$2,$3)",
        )
        .bind(fx.schedule)
        .bind(kind)
        .bind(config)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    }

    fx.runner.tick_once().await;
    let log = run_log(&fx).await;
    assert_eq!(log[0].0, "error");
    let detail = log[0].1.as_deref().unwrap_or_default();
    for want in [
        "webhook hooks.example.com: ",
        "slack #alerts: ",
        "email: smtp is not configured",
    ] {
        assert!(detail.contains(want), "{want:?} missing from {detail:?}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_firing_that_reached_someone_still_starts_the_grace_period(pool: sqlx::PgPool) {
    // The other half of KI-033's rule: the webhook took the message, the
    // email did not. The run is an `error` (something failed) but the grace
    // period starts — re-firing every tick would spam the webhook.
    let (addr, received) = fake_webhook_receiver().await;
    let mut fx = setup(pool, serde_json::json!(9), false, false).await;
    fx.runner.resolver = Arc::new(LoopbackDns(addr));
    fx.runner.policy = SsrfPolicy {
        allow_hosts: vec!["hook.test".into()],
    };
    sqlx::query("UPDATE automation.schedules SET grace_period_s = 3600 WHERE id = $1")
        .bind(fx.schedule)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    for (kind, config) in [
        (
            "webhook",
            serde_json::json!({"url": format!("http://hook.test:{}/x", addr.port())}),
        ),
        ("email", serde_json::json!({"addresses": ["a@example.com"]})),
    ] {
        sqlx::query(
            "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
             VALUES ($1,$2,$3)",
        )
        .bind(fx.schedule)
        .bind(kind)
        .bind(config)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    }

    fx.runner.tick_once().await;
    assert!(!received.lock().unwrap().is_empty(), "the webhook got it");
    sqlx::query(
        "UPDATE automation.schedules SET next_run_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(fx.schedule)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    fx.runner.tick_once().await;

    let log = run_log(&fx).await;
    let states: Vec<&str> = log.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(states, vec!["error", "grace"], "{log:?}");
    assert!(log[0]
        .1
        .as_deref()
        .unwrap_or_default()
        .starts_with("email: "));
}

#[sqlx::test(migrations = "../../migrations")]
async fn schedules_due_in_one_tick_run_side_by_side(pool: sqlx::PgPool) {
    // KI-173: twelve items due at 12:10:00 started between 12:10:18 and
    // 12:11:21 — one slow run delayed every other. Four due schedules, each
    // taking 400 ms, must not take 4 x 400 ms.
    let mut fx = setup(pool, serde_json::json!(0), false, false).await;
    let delay = std::time::Duration::from_millis(400);
    fx.runner.query = Arc::new(FakeEngine {
        value: Mutex::new(serde_json::json!(0)),
        seen: Mutex::new(vec![]),
        fail: false,
        delay,
    });
    for i in 0..3 {
        let copy: Uuid = sqlx::query_scalar(
            "INSERT INTO automation.schedules \
               (workspace_id, type, name, crontab, next_run_at, database_id, sql, \
                validator_type, grace_period_s) \
             SELECT workspace_id, type, name || $2, crontab, next_run_at, database_id, sql, \
                    validator_type, grace_period_s \
               FROM automation.schedules WHERE id = $1 RETURNING id",
        )
        .bind(fx.schedule)
        .bind(format!(" {i}"))
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO automation.schedule_owners (schedule_id, user_id) \
             SELECT $2, user_id FROM automation.schedule_owners WHERE schedule_id = $1",
        )
        .bind(fx.schedule)
        .bind(copy)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    }

    let started = std::time::Instant::now();
    assert_eq!(fx.runner.tick_once().await, 4);
    let took = started.elapsed();

    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM automation.schedule_runs r JOIN automation.schedules s \
           ON s.id = r.schedule_id WHERE s.workspace_id = $1 AND r.state = 'noop'",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(runs, 4, "every due schedule ran");
    assert!(
        took < delay * 3,
        "four 400 ms runs took {took:?}: they ran one after another"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_alert_with_a_foreign_chart_id_still_fires_and_notifies_by_webhook(pool: sqlx::PgPool) {
    // Fix round 1, item 3: `render_alert_attachment`'s whole justification
    // for SKIPPING rather than FAILING on a foreign target is "the alert
    // must not be lost". That refusal is pinned in runner.rs's own unit
    // tests; this pins the SURVIVAL half, end to end, at the tick level — a
    // webhook recipient needs no SMTP, so it proves the alert's core
    // evaluation and delivery are untouched by a foreign chart_id sitting on
    // the row, independent of the attachment mechanism entirely (a webhook
    // payload never carries the picture in the first place).
    let db = Db { pool };
    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("o_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let ws1: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ws2: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // A chart that genuinely exists — but in ws2. ws1's alert points at it.
    let database2: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8090,$2,false) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws2)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let datasets = egret_bi::DatasetRepo::new(db.clone());
    let dataset = datasets
        .create(
            ws2,
            database2,
            "orders",
            "",
            &egret_bi::SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "tiny".into(),
                table: "orders".into(),
            },
            &[egret_bi::NewColumn {
                column_name: "orderpriority".into(),
                data_type: "varchar".into(),
                type_generic: "string".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap();
    let charts = egret_bi::ChartRepo::new(db.clone());
    let foreign_chart = charts
        .create(
            ws2,
            dataset.id,
            "foreign",
            "",
            "table",
            &serde_json::json!({"spec": {}}),
            Some(owner),
        )
        .await
        .unwrap();

    // ws1's own database, for the alert's REAL query — unrelated to ws2.
    let database1: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8080,$2,false) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws1)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, database_id, sql, \
            validator_type, chart_id, report_format, grace_period_s) \
         VALUES ($1,'alert','survives a foreign chart','* * * * *', \
                 now() - interval '1 minute', $2,'SELECT count(*) FROM orders', \
                 'not_null', $3, 'png', 0) RETURNING id",
    )
    .bind(ws1)
    .bind(database1)
    .bind(foreign_chart.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();

    let (webhook_addr, received) = fake_webhook_receiver().await;
    let webhook_host = "survives-foreign-chart.test";
    sqlx::query(
        "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
         VALUES ($1,'webhook',$2)",
    )
    .bind(schedule)
    .bind(serde_json::json!({
        "url": format!("http://{webhook_host}:{}/hook", webhook_addr.port())
    }))
    .execute(&db.pool)
    .await
    .unwrap();

    let engine = Arc::new(FakeEngine {
        value: Mutex::new(serde_json::json!(9)),
        seen: Mutex::new(vec![]),
        fail: false,
        delay: std::time::Duration::ZERO,
    });
    let secrets = egret_secrets::SecretStore::new(db.clone(), None);
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let runner = AutomationRunner {
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
        pdp: Arc::new(NeverPdp),
        dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        resolver: Arc::new(LoopbackDns(webhook_addr)),
        policy: SsrfPolicy {
            allow_hosts: vec![webhook_host.to_string()],
        },
        smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
        charts,
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        datasets,
        render_tokens: egret_automation::render::token::RenderTokenRepo::new(db.clone()),
        // The webhook path never calls the renderer at all (a webhook
        // payload carries rows, never a picture) — NeverRenderer would catch
        // it if that assumption ever broke.
        renderer: Arc::new(NeverRenderer),
        public_url: "https://egret.internal".into(),
        browser_url: "https://egret.internal".into(),
    };

    assert_eq!(runner.tick_once().await, 1);

    let (state, error): (String, Option<String>) =
        sqlx::query_as("SELECT state, error FROM automation.schedule_runs WHERE schedule_id = $1")
            .bind(schedule)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        state, "success",
        "a foreign chart_id must not stop the alert from firing"
    );
    assert_eq!(
        error, None,
        "a webhook recipient never touches the attachment path, so there is \
         nothing here for it to explain"
    );

    // The direct proof of "notifies": the fake receiver actually got a POST,
    // not merely that the run's own bookkeeping looks clean.
    let body = received.lock().unwrap().clone();
    assert!(
        !body.is_empty(),
        "the webhook receiver got nothing — the alert was silently lost"
    );
    let request = String::from_utf8_lossy(&body);
    assert!(request.starts_with("POST "), "{request}");
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
async fn run_now_records_its_outcome_without_touching_next_run_at(pool: sqlx::PgPool) {
    // The whole point of `run_now` NOT calling `release`: a manual run must
    // not move the schedule's own cadence. But its OUTCOME must reach the
    // schedule (KI-032): the list's Last run and the detail badge stayed
    // "Never run" while the run log showed the runs. Pin `next_run_at` and
    // `last_state` to known values first, so both "unchanged" and "changed"
    // are checked against something other than NULL.
    //
    // PINNED AT MICROSECONDS, and that is not cosmetic. `timestamptz` holds
    // microseconds; `Utc::now()` holds nanoseconds. Binding the nanosecond value
    // and then comparing the row against it compares two DIFFERENT precisions of
    // the same instant — `…397028Z` vs `…397028939Z` — so the `assert_eq!` below
    // only passed when the nanosecond remainder happened to be zero, roughly one
    // run in a thousand. Truncating the pin first makes the value one Postgres
    // can store exactly, which keeps the assertion EXACT: the stamp must come
    // back byte-for-byte, not merely close. Weakening it to a tolerance instead
    // would stop it catching the thing it exists for — a `run_now` that called
    // `release` and moved the slot.
    let fx = setup(pool, serde_json::json!(7), false, false).await;
    let pinned_next = (chrono::Utc::now() + chrono::Duration::hours(3)).trunc_subsecs(6);
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
        last_state, "success",
        "run_now must write its outcome onto the schedule, as a tick does"
    );
    assert_eq!(
        next_run_at,
        Some(pinned_next),
        "run_now must not touch next_run_at, or the schedule's own slot would skip or double-fire"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_now_leaves_a_row_the_tick_is_running_alone(pool: sqlx::PgPool) {
    // A tick has claimed the row (`working`). Overwriting that marker with the
    // manual outcome would make the row claimable again mid-run; the tick's own
    // release writes the outcome a moment later.
    let fx = setup(pool, serde_json::json!(7), false, false).await;
    let claimed = egret_automation::scheduler::claim_due(&fx.db.pool, 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);

    fx.runner.run_now(&claimed[0]).await;

    assert_eq!(
        run_log(&fx).await.len(),
        1,
        "the manual run is still logged"
    );
    assert_eq!(schedule_state(&fx).await.0, "working");
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

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
