//! HTTP tests for the one-shot render-payload endpoint (P4 Task 3).
//!
//! The print page makes exactly ONE authenticated request — no session, no
//! second call — so these assertions are about that envelope: a valid token
//! returns spec + already-executed data together (and the data is the ROWS
//! the engine actually returned, not merely "an array"), the token cannot be
//! replayed, and nothing except the token itself (not a session cookie) can
//! stand in for it.
//!
//! The chart is compiled for real (`egret_bi::compile_chart_sql`, against a
//! real dataset/column row) but EXECUTED against a fake engine injected
//! through `AutomationState.query: Arc<dyn QueryRunner>` — the same seam
//! `AutomationRunner` already gets a `FakeEngine` through in
//! `tick_end_to_end.rs`. That makes the success path deterministic and
//! independent of whether a live query engine is reachable in this
//! environment, and lets these assertions check the actual row content
//! instead of merely "some status in {200, 502, 500}".

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_automation::evaluator::{QueryRunner, RunIdentity};
use egret_automation::render::token::RenderTokenRepo;
use egret_automation::{automation_router, AutomationState, ScheduleRepo};
use egret_bi::{ChartRepo, DashboardRepo, DatasetRepo, NewCard, NewColumn, SourceSpec};
use egret_core::tabular::ColumnMeta;
use egret_core::TabularResult;
use egret_metastore::Db;
use tower::ServiceExt;
use uuid::Uuid;

/// Never actually called: every database in this fixture is non-delegated.
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

/// A query engine that returns one fixed, known table — modeled on
/// `tick_end_to_end.rs`'s `FakeEngine`. What it returns is the exact thing
/// the success-path test checks the response against, so the test proves the
/// endpoint carries the engine's rows through to the HTTP body rather than
/// merely reaching `run_query` without erroring.
struct FakeEngine {
    columns: Vec<&'static str>,
    rows: Vec<Vec<serde_json::Value>>,
}

#[async_trait::async_trait]
impl QueryRunner for FakeEngine {
    async fn run(
        &self,
        _database_id: Uuid,
        _sql: &str,
        _identity: &RunIdentity,
        _limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        Ok(TabularResult {
            columns: self
                .columns
                .iter()
                .map(|c| ColumnMeta {
                    name: (*c).to_string(),
                    ty: "varchar".into(),
                })
                .collect(),
            row_count: self.rows.len() as u64,
            rows: self.rows.clone(),
            bytes_scanned: None,
            truncated: false,
        })
    }
}

fn fake_engine() -> FakeEngine {
    FakeEngine {
        columns: vec!["orderpriority"],
        rows: vec![
            vec![serde_json::json!("1-URGENT")],
            vec![serde_json::json!("2-HIGH")],
        ],
    }
}

struct Fx {
    app: axum::Router,
    render_tokens: RenderTokenRepo,
    dashboard: Uuid,
    owner: Uuid,
    admin_tok: String,
}

impl Fx {
    async fn mint_for_dashboard(&self) -> String {
        self.render_tokens
            .mint(
                "dashboard",
                self.dashboard,
                self.owner,
                Duration::from_secs(60),
            )
            .await
            .expect("mint render token")
    }
}

async fn mk_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn setup(pool: sqlx::PgPool) -> Fx {
    let db = Db { pool };
    let owner = mk_user(&db, "rp_owner").await;
    let slug = format!("rp-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Render Payload Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Never actually dialed — execution goes through `FakeEngine`, not a real
    // connector — but `compile_chart_sql` still needs a real `query.databases`
    // row (engine + dataset source) to compile the chart's spec to SQL text.
    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8090,$2,false) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let secrets = egret_secrets::SecretStore::new(db.clone(), None);
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = egret_query::DatabaseRepo::new(db.clone(), secrets, ws_secrets);

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

    let dashboards = DashboardRepo::new(db.clone());
    let dashboard = dashboards
        .create(ws, "Report dashboard", "", Some(owner))
        .await
        .unwrap();
    dashboards
        .replace_layout(
            dashboard.id,
            &[],
            &[NewCard {
                id: None,
                chart_id: Some(chart.id),
                card_type: "chart".into(),
                grid_col: 0,
                grid_row: 0,
                size_x: 4,
                size_y: 4,
                options: serde_json::json!({}),
                tab_index: None,
            }],
        )
        .await
        .unwrap();

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .unwrap(),
    );
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let admin_tok = sessions
        .create(egret_core::ids::UserId(owner), "test", "127.0.0.1")
        .await
        .unwrap()
        .token;

    let render_tokens = RenderTokenRepo::new(db.clone());
    let st = AutomationState {
        db: db.clone(),
        schedules: ScheduleRepo::new(db.clone()),
        sessions,
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[7u8; 32]).unwrap()),
            ),
        ),
        render_tokens: render_tokens.clone(),
        charts,
        dashboards,
        datasets,
        databases,
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        query: Arc::new(fake_engine()),
    };

    Fx {
        app: automation_router().with_state(st),
        render_tokens,
        dashboard: dashboard.id,
        owner,
        admin_tok,
    }
}

async fn get_payload(fx: &Fx, token: &str) -> (StatusCode, String) {
    let res = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/render/payload")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn get_payload_without_token(fx: &Fx) -> (StatusCode, String) {
    let res = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/render/payload")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn get_payload_with_cookie(fx: &Fx, session_tok: &str) -> (StatusCode, String) {
    let res = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/render/payload")
                .header("Cookie", format!("egret_session={session_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_payload_is_returned_once_for_a_valid_token(pool: sqlx::PgPool) {
    // The print page's only request. It must arrive complete — spec AND data —
    // because the page makes no second call. Unconditional: the engine is
    // faked (see `FakeEngine` above), so there is no environment-dependent
    // branch here.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    let (s, body) = get_payload(&fx, &token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["kind"], "dashboard");
    assert!(
        !v["cards"].as_array().unwrap().is_empty(),
        "cards carry their own data: {body}"
    );
    // The claim under test: the rows are the ENGINE's rows, carried through
    // to the response byte-for-byte — not merely "some array".
    assert_eq!(
        v["cards"][0]["columns"],
        serde_json::json!(["orderpriority"])
    );
    assert_eq!(
        v["cards"][0]["rows"],
        serde_json::json!([["1-URGENT"], ["2-HIGH"]]),
        "{body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_token_is_refused(pool: sqlx::PgPool) {
    // Single use, end to end: the SECOND fetch of the same page fails.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    assert_eq!(get_payload(&fx, &token).await.0, StatusCode::OK);
    assert_eq!(get_payload(&fx, &token).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn no_token_is_refused(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    assert_eq!(
        get_payload_without_token(&fx).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_cookie_cannot_substitute_for_a_token(pool: sqlx::PgPool) {
    // The endpoint must NOT accept an ordinary logged-in user: it exists to be
    // reachable by a browser that has no session, and widening it to sessions
    // would make it a second, unaudited read path for any object.
    let fx = setup(pool).await;
    assert_eq!(
        get_payload_with_cookie(&fx, &fx.admin_tok).await.0,
        StatusCode::UNAUTHORIZED
    );
}
