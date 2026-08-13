//! The report branch of the tick, end to end (P4 Task 6).
//!
//! Modelled on `tick_end_to_end.rs`: a real database, a fake query engine, and
//! here also a fake `Renderer` — so every assertion is on the RUN LOG and the
//! fake renderer's own record, never on the absence of an error.

use std::sync::{Arc, Mutex};

use egret_automation::deliver::destination::{Resolver, SsrfPolicy};
use egret_automation::evaluator::{QueryRunner, RunIdentity};
use egret_automation::render::client::{RenderRequest, Renderer};
use egret_automation::render::token::RenderTokenRepo;
use egret_automation::runner::AutomationRunner;
use egret_automation::ScheduleRepo;
use egret_bi::{ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SourceSpec};
use egret_core::tabular::ColumnMeta;
use egret_core::TabularResult;
use egret_metastore::Db;
use uuid::Uuid;

// ── fakes ────────────────────────────────────────────────────────────────

/// What the fake renderer decides to do with every request it's handed.
#[derive(Clone)]
enum RenderOutcome {
    Png(Vec<u8>),
    Fail(String),
}

/// Records every request it was asked to render, and answers with a fixed
/// outcome — never a real Chromium process.
struct FakeRenderer {
    outcome: RenderOutcome,
    seen: Mutex<Vec<RenderRequest>>,
}

#[async_trait::async_trait]
impl Renderer for FakeRenderer {
    async fn render(&self, req: RenderRequest) -> Result<Vec<u8>, String> {
        self.seen.lock().unwrap().push(req);
        match &self.outcome {
            RenderOutcome::Png(bytes) => Ok(bytes.clone()),
            RenderOutcome::Fail(reason) => Err(reason.clone()),
        }
    }
}

/// A query engine that returns one fixed row — for the CSV-report fixture,
/// which must never reach `FakeRenderer` at all.
struct FakeEngine;

#[async_trait::async_trait]
impl QueryRunner for FakeEngine {
    async fn run(
        &self,
        _db: Uuid,
        _sql: &str,
        _identity: &RunIdentity,
        _limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        Ok(TabularResult {
            columns: vec![ColumnMeta {
                name: "orderpriority".into(),
                ty: "varchar".into(),
            }],
            rows: vec![vec![serde_json::json!("1-URGENT")]],
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
    schedule: Uuid,
    renderer: Arc<FakeRenderer>,
}

async fn mk_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
        .bind(format!("{tag}_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

fn runner(
    db: &Db,
    query: Arc<dyn QueryRunner>,
    renderer: Arc<dyn Renderer>,
    charts: ChartRepo,
    datasets: DatasetRepo,
) -> AutomationRunner {
    let secrets = egret_secrets::SecretStore::new(db.clone(), None);
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    AutomationRunner {
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
        query,
        resolver: Arc::new(PublicDns),
        policy: SsrfPolicy::default(),
        smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
        charts,
        datasets,
        render_tokens: RenderTokenRepo::new(db.clone()),
        renderer,
        public_url: "https://egret.internal".into(),
        browser_url: "https://egret.internal".into(),
    }
}

/// A `png` report against a dashboard target. `schedules.dashboard_id` carries
/// a real FK to `analytics.dashboards`, so — unlike a render token, which only
/// needs a `kind` + `object_id` — the schedule row needs an actual dashboard
/// to point at, even though the renderer itself never touches the database.
async fn setup(pool: sqlx::PgPool, outcome: RenderOutcome) -> Fx {
    let db = Db { pool };
    let owner = mk_user(&db, "o").await;
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let dashboards = DashboardRepo::new(db.clone());
    let dashboard = dashboards
        .create(ws, "Report dashboard", "", Some(owner))
        .await
        .unwrap();
    let dashboard_id = dashboard.id;

    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, dashboard_id, report_format, \
            grace_period_s) \
         VALUES ($1,'report','nightly report','* * * * *', now() - interval '1 minute', \
                 $2,'png', 0) RETURNING id",
    )
    .bind(ws)
    .bind(dashboard_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();

    let renderer = Arc::new(FakeRenderer {
        outcome,
        seen: Mutex::new(vec![]),
    });
    let r = runner(
        &db,
        Arc::new(FakeEngine),
        renderer.clone(),
        ChartRepo::new(db.clone()),
        DatasetRepo::new(db.clone()),
    );

    Fx {
        runner: r,
        db,
        schedule,
        renderer,
    }
}

/// A `csv` report against a REAL chart+dataset+database — `build_chart_report`
/// actually compiles and runs the chart's saved spec, so this fixture needs
/// the same minimal bi rows `render_payload_routes.rs` sets up.
async fn setup_csv_chart_report(pool: sqlx::PgPool) -> Fx {
    let db = Db { pool };
    let owner = mk_user(&db, "c").await;
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8090,$2,false) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let datasets = DatasetRepo::new(db.clone());
    let dataset = datasets
        .create(
            ws,
            database,
            "orders",
            "",
            &SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "tiny".into(),
                table: "orders".into(),
            },
            &[NewColumn {
                column_name: "orderpriority".into(),
                data_type: "varchar".into(),
                type_generic: "string".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap();

    let charts = ChartRepo::new(db.clone());
    let chart_spec = serde_json::json!({
        "spec": {
            "metrics": [],
            "dimensions": [],
            "columns": ["orderpriority"],
            "filters": [],
            "orderby": [],
            "row_limit": 5,
            "rollup_groups": [],
            "temporal_units": {},
        }
    });
    let chart = charts
        .create(
            ws,
            dataset.id,
            "Order priorities",
            "",
            "table",
            &chart_spec,
            Some(owner),
        )
        .await
        .unwrap();

    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, chart_id, report_format, \
            grace_period_s) \
         VALUES ($1,'report','orders csv','* * * * *', now() - interval '1 minute', \
                 $2,'csv', 0) RETURNING id",
    )
    .bind(ws)
    .bind(chart.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();

    let renderer = Arc::new(FakeRenderer {
        // Never reached — a hard failure here would flip a "the renderer was
        // never called" bug into a false pass.
        outcome: RenderOutcome::Fail("csv reports must never call the renderer".into()),
        seen: Mutex::new(vec![]),
    });
    let r = runner(
        &db,
        Arc::new(FakeEngine),
        renderer.clone(),
        charts,
        datasets,
    );

    Fx {
        runner: r,
        db,
        schedule,
        renderer,
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

/// Make the schedule due again, as its crontab would on the next minute.
async fn make_due_again(fx: &Fx) {
    sqlx::query(
        "UPDATE automation.schedules SET next_run_at = now() - interval '1 minute' \
                 WHERE id = $1",
    )
    .bind(fx.schedule)
    .execute(&fx.db.pool)
    .await
    .unwrap();
}

/// Pull the `t=` value out of a print-page URL.
fn token_of(url: &str) -> String {
    url.split("t=").nth(1).unwrap_or_default().to_string()
}

// ── tests ────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_png_report_renders_and_emails_the_image(pool: sqlx::PgPool) {
    // The whole point of P4, asserted on the RUN LOG and the fake renderer's
    // record — never on the absence of an error.
    let fx = setup(pool, RenderOutcome::Png(vec![0x89, b'P', b'N', b'G'])).await;
    assert_eq!(fx.runner.tick_once().await, 1);
    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "success");
    let seen = fx.renderer.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the renderer was asked exactly once");
    assert!(
        seen[0].url.contains("/embed/print/dashboard/"),
        "{}",
        seen[0].url
    );
    assert!(seen[0].url.contains("t="), "the URL carries a render token");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_renderer_failure_is_an_error_run_that_stays_scheduled(pool: sqlx::PgPool) {
    let fx = setup(pool, RenderOutcome::Fail("timeout".into())).await;
    fx.runner.tick_once().await;
    let log = run_log(&fx).await;
    assert_eq!(log[0].0, "error");
    assert!(log[0].1.as_deref().unwrap_or_default().contains("timeout"));
    let (_, next) = schedule_state(&fx).await;
    assert!(
        next.unwrap() > chrono::Utc::now(),
        "a failing report keeps trying"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn each_render_gets_a_fresh_token(pool: sqlx::PgPool) {
    // Single-use means a second run cannot reuse the first run's token.
    let fx = setup(pool, RenderOutcome::Png(vec![0x89, b'P', b'N', b'G'])).await;
    fx.runner.tick_once().await;
    make_due_again(&fx).await;
    fx.runner.tick_once().await;
    let seen = fx.renderer.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_ne!(token_of(&seen[0].url), token_of(&seen[1].url));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_csv_report_never_calls_the_renderer(pool: sqlx::PgPool) {
    // csv and text are data the backend already has.
    let fx = setup_csv_chart_report(pool).await;
    fx.runner.tick_once().await;
    assert_eq!(run_log(&fx).await[0].0, "success");
    assert!(fx.renderer.seen.lock().unwrap().is_empty());
}
