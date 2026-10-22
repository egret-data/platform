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
///
/// Also counts every call it receives, in a counter the FIXTURE owns (not the
/// engine) — so a test can assert the engine was never even asked to run,
/// which is the direct proof that a foreign chart's data was never fetched
/// (SEC-3), not merely that the eventual outcome looked like a failure.
struct FakeEngine {
    calls: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl QueryRunner for FakeEngine {
    async fn run(
        &self,
        _db: Uuid,
        _sql: &str,
        _identity: &RunIdentity,
        _limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        *self.calls.lock().unwrap() += 1;
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

/// Task 4's run-time re-check only runs on the `dataapi_alert` path — every
/// schedule in this file is `type = 'report'` — so a `panic!` here turns "the
/// report path started calling the PDP" into a loud test failure instead of a
/// silent one.
struct NeverPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for NeverPdp {
    async fn authorize(
        &self,
        _subject: &egret_core::authz::Subject,
        _action: &str,
        _resource: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        panic!("a report must never consult the PDP");
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
    ws: Uuid,
    schedule: Uuid,
    renderer: Arc<FakeRenderer>,
    engine_calls: Arc<Mutex<usize>>,
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
        charts,
        dashboards: DashboardRepo::new(db.clone()),
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
    let engine_calls = Arc::new(Mutex::new(0));
    let r = runner(
        &db,
        Arc::new(FakeEngine {
            calls: engine_calls.clone(),
        }),
        renderer.clone(),
        ChartRepo::new(db.clone()),
        DatasetRepo::new(db.clone()),
    );

    Fx {
        runner: r,
        db,
        ws,
        schedule,
        renderer,
        engine_calls,
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
    let engine_calls = Arc::new(Mutex::new(0));
    let r = runner(
        &db,
        Arc::new(FakeEngine {
            calls: engine_calls.clone(),
        }),
        renderer.clone(),
        charts,
        datasets,
    );

    Fx {
        runner: r,
        db,
        ws,
        schedule,
        renderer,
        engine_calls,
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

// ── SEC-3 fixtures: a chart/dashboard that live in a SEPARATE workspace ────
//
// Every case below inserts the foreign row directly by SQL rather than
// through the routes — SEC-2 already refuses a foreign target at save, so
// this is deliberately the row that "got in anyway": written before SEC-2
// shipped, or by anything that bypasses the API.

/// A chart in its OWN new workspace — for pointing a report/alert in a
/// DIFFERENT workspace at it.
async fn mk_foreign_chart(db: &Db, tag: &str) -> (Uuid, Uuid) {
    let owner = mk_user(db, tag).await;
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
            "secret",
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
            "metrics": [], "dimensions": [], "columns": ["orderpriority"],
            "filters": [], "orderby": [], "row_limit": 5,
            "rollup_groups": [], "temporal_units": {},
        }
    });
    let chart = charts
        .create(
            ws,
            dataset.id,
            "foreign chart",
            "",
            "table",
            &chart_spec,
            Some(owner),
        )
        .await
        .unwrap();
    (ws, chart.id)
}

/// A dashboard in its OWN new workspace.
async fn mk_foreign_dashboard(db: &Db, tag: &str) -> (Uuid, Uuid) {
    let owner = mk_user(db, tag).await;
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
        .create(ws, "foreign dashboard", "", Some(owner))
        .await
        .unwrap();
    (ws, dashboard.id)
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

// ── SEC-3: a pre-existing foreign target fails closed at run time ──────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_csv_report_with_a_directly_inserted_foreign_chart_id_fails_closed(pool: sqlx::PgPool) {
    // Mirrors the probe's Q1/Q2/Q3: a report in one workspace naming a chart
    // in another. SEC-2 refuses this at save now, so this is the row that
    // "got in anyway" — set directly by SQL, bypassing every route.
    let fx = setup_csv_chart_report(pool).await;
    let (foreign_ws, foreign_chart) = mk_foreign_chart(&fx.db, "csv_foreign").await;
    assert_ne!(
        foreign_ws, fx.ws,
        "the chart must live in a DIFFERENT workspace"
    );
    sqlx::query("UPDATE automation.schedules SET chart_id = $2 WHERE id = $1")
        .bind(fx.schedule)
        .bind(foreign_chart)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    assert_eq!(fx.runner.tick_once().await, 1);
    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "error", "the run must be recorded as an error");
    assert!(
        log[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("no longer exists"),
        "{:?}",
        log[0].1
    );
    // The direct proof that nothing was delivered: the engine that would
    // have run the foreign chart's compiled SQL was never even asked to.
    // Asserting only the run STATE would still pass if `build_chart_report`
    // fetched, compiled and executed the foreign chart and only failed
    // afterwards (e.g. on delivery) — this rules that out.
    assert_eq!(
        *fx.engine_calls.lock().unwrap(),
        0,
        "the foreign chart's SQL must never reach the query engine"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_png_report_with_a_directly_inserted_foreign_schedule_targets_row_fails_closed(
    pool: sqlx::PgPool,
) {
    // Mirrors the probe's Round-2 Surface 1: `render_image_report` reads
    // `targets_for` FIRST and `report_target` (the chart_id/dashboard_id
    // columns) only as a fallback — so this is the entry point a fix scoped
    // to those two columns would miss entirely. The schedule's own
    // `dashboard_id` (set by `setup`) is legitimate and in-workspace; the
    // `schedule_targets` row inserted here is what the runner actually reads.
    let fx = setup(pool, RenderOutcome::Png(vec![0x89, b'P', b'N', b'G'])).await;
    let (foreign_ws, foreign_dashboard) = mk_foreign_dashboard(&fx.db, "png_foreign").await;
    assert_ne!(foreign_ws, fx.ws);
    sqlx::query(
        "INSERT INTO automation.schedule_targets (schedule_id, kind, object_id, ordinal) \
         VALUES ($1, 'dashboard', $2, 0)",
    )
    .bind(fx.schedule)
    .bind(foreign_dashboard)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    assert_eq!(fx.runner.tick_once().await, 1);
    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "error", "the run must be recorded as an error");
    assert!(
        log[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("no longer exists"),
        "{:?}",
        log[0].1
    );
    // The direct proof: no token was minted and the renderer was never asked
    // to fetch the foreign dashboard's print page. A test that only checked
    // the run state would still pass if the renderer HAD been called and
    // merely returned an error screenshot — this rules that out, and it is
    // exactly the gap the probe's Surface 2 exploited (the render/payload
    // boundary 404s, but nothing upstream of it used to notice).
    assert!(
        fx.renderer.seen.lock().unwrap().is_empty(),
        "the renderer must never be asked to fetch a foreign object's print page"
    );
    // Fix round 1's item 2: the comment above claims no token was minted —
    // prove it against the table, not just the (also correct) renderer seam.
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM automation.render_tokens")
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(tokens, 0, "no render token may exist for a foreign object");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_png_report_with_a_same_workspace_schedule_targets_row_still_renders(pool: sqlx::PgPool) {
    // The negative control (design §5): the new per-target validation loop
    // must not break the legitimate multi-target case it sits in front of.
    let fx = setup(pool, RenderOutcome::Png(vec![0x89, b'P', b'N', b'G'])).await;
    let dashboards = DashboardRepo::new(fx.db.clone());
    let own_dashboard = dashboards
        .create(fx.ws, "second in-workspace dashboard", "", None)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO automation.schedule_targets (schedule_id, kind, object_id, ordinal) \
         VALUES ($1, 'dashboard', $2, 0)",
    )
    .bind(fx.schedule)
    .bind(own_dashboard.id)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    assert_eq!(fx.runner.tick_once().await, 1);
    let log = run_log(&fx).await;
    assert_eq!(log[0].0, "success", "{:?}", log[0].1);
    assert_eq!(
        fx.renderer.seen.lock().unwrap().len(),
        1,
        "a same-workspace target still renders"
    );
}

// ── KI-167: the render tokens of a multi-page report outlive its render ──

#[sqlx::test(migrations = "../../migrations")]
async fn a_multi_page_reports_tokens_live_as_long_as_its_render_may(pool: sqlx::PgPool) {
    // The renderer opens the pages one after another; the third page of a
    // long report is fetched well after a fixed 60 s token would have expired.
    let fx = setup(pool, RenderOutcome::Png(b"%PDF".to_vec())).await;
    let dashboards = DashboardRepo::new(fx.db.clone());
    for (i, name) in ["first", "second", "third"].iter().enumerate() {
        let d = dashboards.create(fx.ws, name, "", None).await.unwrap();
        sqlx::query(
            "INSERT INTO automation.schedule_targets (schedule_id, kind, object_id, ordinal) \
             VALUES ($1, 'dashboard', $2, $3)",
        )
        .bind(fx.schedule)
        .bind(d.id)
        .bind(i as i32)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    }

    // The fake renderer never redeems, so the minted tokens stay in the table.
    fx.runner.tick_once().await;
    let lifetimes: Vec<f64> = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - now())::float8 FROM automation.render_tokens",
    )
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(lifetimes.len(), 3, "one token per page");
    let budget = egret_automation::render::client::render_budget(3).as_secs_f64();
    for left in lifetimes {
        assert!(
            left > budget - 10.0,
            "a page token must outlive the whole render ({budget}s), it has {left}s"
        );
    }
}

// ── KI-157 / KI-169: what a csv/text report actually mails ───────────────

/// The engine's answer for the KI-169 rows: a `double` revenue per status.
struct RevenueEngine;

#[async_trait::async_trait]
impl QueryRunner for RevenueEngine {
    async fn run(
        &self,
        _db: Uuid,
        _sql: &str,
        _identity: &RunIdentity,
        _limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        Ok(TabularResult {
            columns: vec![
                ColumnMeta {
                    name: "orderstatus".into(),
                    ty: "varchar".into(),
                },
                ColumnMeta {
                    name: "revenue".into(),
                    ty: "double".into(),
                },
            ],
            rows: vec![
                vec![
                    serde_json::json!("F"),
                    serde_json::from_str("1.0356810234900002e+9").unwrap(),
                ],
                vec![
                    serde_json::json!("P"),
                    serde_json::from_str("6.333947531999999e+7").unwrap(),
                ],
            ],
            row_count: 2,
            bytes_scanned: None,
            truncated: false,
        })
    }
}

/// A plaintext SMTP relay that keeps every message it is handed — enough of
/// RFC 5321 for `lettre` (`TlsMode::None`, no auth). One connection.
async fn capturing_smtp() -> (std::net::SocketAddr, Arc<Mutex<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mail = Arc::new(Mutex::new(String::new()));
    let sink = mail.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (r, mut w) = stream.into_split();
        let mut lines = tokio::io::BufReader::new(r).lines();
        let _ = w.write_all(b"220 fake.local ESMTP\r\n").await;
        let mut in_data = false;
        while let Ok(Some(line)) = lines.next_line().await {
            if in_data {
                if line == "." {
                    in_data = false;
                    let _ = w.write_all(b"250 2.0.0 OK: queued\r\n").await;
                } else {
                    let mut m = sink.lock().unwrap();
                    m.push_str(&line);
                    m.push('\n');
                }
                continue;
            }
            let upper = line.to_ascii_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                b"250-fake.local\r\n250 OK\r\n"
            } else if upper == "DATA" {
                in_data = true;
                b"354 End data with <CR><LF>.<CR><LF>\r\n"
            } else if upper.starts_with("QUIT") {
                let _ = w.write_all(b"221 2.0.0 Bye\r\n").await;
                break;
            } else {
                b"250 OK\r\n"
            };
            let _ = w.write_all(reply).await;
        }
    });
    (addr, mail)
}

/// The csv fixture, renamed and given a mail recipient, a relay that keeps
/// what it receives, and the revenue engine.
async fn mailed_report(pool: sqlx::PgPool, name: &str, format: &str) -> (Fx, Arc<Mutex<String>>) {
    let mut fx = setup_csv_chart_report(pool).await;
    fx.runner.query = Arc::new(RevenueEngine);
    sqlx::query("UPDATE automation.schedules SET name = $2, report_format = $3 WHERE id = $1")
        .bind(fx.schedule)
        .bind(name)
        .bind(format)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO automation.schedule_recipients (schedule_id, kind, config) \
         VALUES ($1,'email',$2)",
    )
    .bind(fx.schedule)
    .bind(serde_json::json!({"addresses": ["reader@example.com"]}))
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let (addr, mail) = capturing_smtp().await;
    sqlx::query(
        "INSERT INTO automation.smtp_config (workspace_id, host, port, from_address, tls_mode) \
         VALUES ($1,$2,$3,'reports@example.com','none')",
    )
    .bind(fx.ws)
    .bind(addr.ip().to_string())
    .bind(addr.port() as i32)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    (fx, mail)
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_csv_report_writes_numbers_as_the_table_shows_them(pool: sqlx::PgPool) {
    let (fx, mail) = mailed_report(pool, "Revenue by status CSV", "csv").await;
    fx.runner.tick_once().await;
    assert_eq!(
        run_log(&fx).await[0].0,
        "success",
        "{:?}",
        run_log(&fx).await
    );

    let mail = mail.lock().unwrap().clone();
    assert!(
        mail.contains("\"F\",\"1035681023.49\""),
        "rounded like the table, not grouped, not float noise:\n{mail}"
    );
    assert!(mail.contains("\"P\",\"63339475.32\""), "{mail}");
    assert!(!mail.contains("e+9"), "no scientific notation:\n{mail}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_text_report_writes_numbers_as_the_table_shows_them(pool: sqlx::PgPool) {
    let (fx, mail) = mailed_report(pool, "Revenue by status (text)", "text").await;
    fx.runner.tick_once().await;
    assert_eq!(
        run_log(&fx).await[0].0,
        "success",
        "{:?}",
        run_log(&fx).await
    );

    let mail = mail.lock().unwrap().clone();
    assert!(mail.contains("F | 1,035,681,023.49"), "{mail}");
    assert!(mail.contains("P | 63,339,475.32"), "{mail}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_report_named_report_is_not_mailed_as_report_report(pool: sqlx::PgPool) {
    let (fx, mail) = mailed_report(pool, "start: first dashboard report", "csv").await;
    fx.runner.tick_once().await;

    let mail = mail.lock().unwrap().clone();
    assert!(
        mail.contains("Subject: [egret] start: first dashboard report\n"),
        "{mail}"
    );
    assert!(!mail.contains("report report"), "{mail}");
}

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
