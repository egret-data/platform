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
    /// Every statement this engine was asked to run.
    ///
    /// Without it a test can only assert that rows came back, which they do
    /// whether or not the resolved dashboard filters were merged onto the
    /// saved spec — the exact defect this path exists to fix would pass.
    seen_sql: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
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
        self.seen_sql.lock().unwrap().push(_sql.to_string());
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

fn fake_engine(seen_sql: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> FakeEngine {
    FakeEngine {
        columns: vec!["orderpriority"],
        rows: vec![
            vec![serde_json::json!("1-URGENT")],
            vec![serde_json::json!("2-HIGH")],
        ],
        seen_sql,
    }
}

struct Fx {
    app: axum::Router,
    db: Db,
    render_tokens: RenderTokenRepo,
    ws: Uuid,
    dashboard: Uuid,
    dataset: Uuid,
    owner: Uuid,
    admin_tok: String,
    seen_sql: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Fx {
    async fn mint_for_dashboard(&self) -> String {
        self.render_tokens
            .mint(
                "dashboard",
                self.dashboard,
                self.ws,
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
    let seen_sql: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
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
    let smtp = egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(
            db.clone(),
            Some(egret_secrets::Crypto::new(&[7u8; 32]).unwrap()),
        ),
    );
    // This file never exercises "Run now" — it is wired only so
    // `AutomationState` can construct.
    let runner = Arc::new(egret_automation::runner::AutomationRunner {
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
        databases: databases.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        minter: Arc::new(NoMinter),
        query: Arc::new(fake_engine(seen_sql.clone())),
        pdp: pdp.clone(),
        dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        resolver: Arc::new(egret_automation::deliver::destination::SystemResolver),
        policy: egret_automation::deliver::destination::SsrfPolicy::default(),
        smtp: smtp.clone(),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        charts: charts.clone(),
        dashboards: dashboards.clone(),
        datasets: datasets.clone(),
        render_tokens: render_tokens.clone(),
        renderer: Arc::new(egret_automation::render::client::HttpRenderer::new(
            "http://127.0.0.1:1".into(),
        )),
        public_url: "https://egret.test".into(),
        browser_url: "https://egret.test".into(),
    });
    let st = AutomationState {
        db: db.clone(),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        schedules: ScheduleRepo::new(
            db.clone(),
            // Crypto-enabled: `SecretStore::new(db, None)` cannot ENCRYPT, so a
            // webhook secret would fail to vault on write.
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        sessions,
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        smtp,
        render_tokens: render_tokens.clone(),
        charts,
        dashboards,
        datasets,
        folders: egret_bi::folder::FolderRepo::new(db.clone()),
        databases,
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        query: Arc::new(fake_engine(seen_sql.clone())),
        runner,
    };

    Fx {
        // Both routers, as the server mounts them — `render_router` is separate
        // so it can be mounted WITHOUT the CSRF layer, not because it is a
        // different service.
        app: automation_router()
            .merge(egret_automation::render_router())
            .with_state(st),
        db: db.clone(),
        render_tokens,
        ws,
        dashboard: dashboard.id,
        dataset: dataset.id,
        owner,
        admin_tok,
        seen_sql,
    }
}

async fn post_data(fx: &Fx, token: &str, body: serde_json::Value) -> (StatusCode, String) {
    let res = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/render/data")
                .header("Authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
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
async fn the_payload_describes_the_object_and_the_data_call_returns_its_rows(pool: sqlx::PgPool) {
    // The render is two steps now. Step one hands the page the saved specs and
    // the dashboard's parameters so it can resolve them with the SAME code the
    // interactive dashboard uses; step two returns the rows for the filters it
    // resolved. Unconditional: the engine is faked (see `FakeEngine` above),
    // so there is no environment-dependent branch here.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;

    let (s, body) = get_payload(&fx, &token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["kind"], "dashboard");
    let cards = v["cards"].as_array().unwrap();
    assert!(!cards.is_empty(), "no cards to render: {body}");
    // Step one must NOT carry rows — if it did, the parameters could not
    // affect them, which is the whole defect this path fixes.
    assert!(
        cards[0].get("rows").is_none(),
        "step one ran the query before the filters existed: {body}"
    );
    assert!(
        v.get("parameters").is_some(),
        "the page cannot resolve filters it was never given: {body}"
    );
    let chart_id = cards[0]["chart_id"].as_str().unwrap().to_string();

    let (s, body) = post_data(
        &fx,
        &token,
        serde_json::json!([{"chart_id": chart_id, "filters": [], "temporal_units": {}}]),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    // The rows are the ENGINE's rows, carried through byte-for-byte — not
    // merely "some array". Columns carry the engine's name AND type: the print
    // page needs the real type to format a column the way the live chart
    // would, and a names-only list is exactly the drift this asserts against.
    assert_eq!(
        v[0]["columns"],
        serde_json::json!([{"name": "orderpriority", "type": "varchar"}])
    );
    assert_eq!(
        v[0]["rows"],
        serde_json::json!([["1-URGENT"], ["2-HIGH"]]),
        "{body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_payload_carries_each_cards_dataset_display_names_and_formats(pool: sqlx::PgPool) {
    // KI-301: the print page drew a card with the chart's own settings only, so
    // a PNG or PDF report said `order_count` where the dashboard says "Orders".
    // The page has no session to load the dataset with; the payload is the only
    // place it can learn the names from.
    let fx = setup(pool).await;
    let datasets = DatasetRepo::new(fx.db.clone());
    datasets
        .update_column(
            fx.dataset,
            "orderpriority",
            Some("Order priority"),
            None,
            None,
            None,
            None,
            Some("{\"decimals\":0}"),
            Some("category"),
            None,
        )
        .await
        .unwrap();
    datasets
        .add_metric(
            fx.dataset,
            "order_count",
            "count(*)",
            "bigint",
            "numeric",
            "Orders",
            "",
            "",
        )
        .await
        .unwrap();
    let token = fx.mint_for_dashboard().await;

    let (s, body) = get_payload(&fx, &token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["cards"][0]["dataset"],
        serde_json::json!({
            "columns": [{
                "column_name": "orderpriority",
                "verbose_name": "Order priority",
                "format": "{\"decimals\":0}",
                "semantic_type": "category",
            }],
            "metrics": [{"metric_name": "order_count", "verbose_name": "Orders", "format": ""}],
        }),
        "{body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_token_naming_a_dashboard_from_a_different_workspace_is_refused(pool: sqlx::PgPool) {
    // SEC-3, pinning the most security-critical line on this boundary
    // (design §4.1: "every caller passes the SCHEDULE's workspace, never the
    // object's"). `render_image_report`/`render_alert_attachment` always mint
    // with the SCHEDULE's own workspace as `grant.workspace_id` — this
    // reproduces that shape directly, for a dashboard that actually lives in
    // a SEPARATE workspace: exactly the probe's Round-2 Surface 2
    // (`/render/payload` returning workspace 2's title, spec and rows to a
    // workspace-1 token).
    let fx = setup(pool).await;

    let other_owner = mk_user(&fx.db, "rp_other").await;
    let other_ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Other Workspace', $2, 'active') RETURNING id",
    )
    .bind(format!("rp-other-{}", Uuid::new_v4().simple()))
    .bind(other_owner)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let other_dashboards = DashboardRepo::new(fx.db.clone());
    let other_dashboard = other_dashboards
        .create(other_ws, "workspace 2's dashboard", "", Some(other_owner))
        .await
        .unwrap();

    // Minted exactly the way `render_image_report`/`render_alert_attachment`
    // do: `grant.workspace_id` is fx.ws (the SCHEDULE's own workspace, ws1)
    // — never `other_ws`, the object's own. That is what makes this the same
    // shape a foreign row on a pre-existing schedule would produce.
    let token = fx
        .render_tokens
        .mint(
            "dashboard",
            other_dashboard.id,
            fx.ws,
            fx.owner,
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("mint render token");

    let (status, body) = get_payload(&fx, &token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a dashboard from a different workspace must 404, not describe it: {body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_token_survives_the_describe_step_and_dies_on_the_data_step(pool: sqlx::PgPool) {
    // The envelope moved, deliberately: step one is metadata and may be
    // repeated inside the token's ~60s life, step two hands over rows and is
    // the one that burns it. A page that could re-fetch its data would be a
    // data tap; a page that cannot re-read a title would simply be fragile.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;

    assert_eq!(get_payload(&fx, &token).await.0, StatusCode::OK);
    assert_eq!(
        get_payload(&fx, &token).await.0,
        StatusCode::OK,
        "describing the object must not spend the token"
    );

    let (s, body) = get_payload(&fx, &token).await;
    let chart_id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["cards"][0]
        ["chart_id"]
        .as_str()
        .unwrap()
        .to_string();
    let req = serde_json::json!([{"chart_id": chart_id}]);
    assert_eq!(s, StatusCode::OK);
    assert_eq!(post_data(&fx, &token, req.clone()).await.0, StatusCode::OK);
    assert_eq!(
        post_data(&fx, &token, req).await.0,
        StatusCode::NOT_FOUND,
        "the rows were handed out twice"
    );
    assert_eq!(
        get_payload(&fx, &token).await.0,
        StatusCode::NOT_FOUND,
        "the spent token still describes the object"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_resolved_filter_reaches_the_compiled_sql(pool: sqlx::PgPool) {
    // THE defect this two-step render exists to fix. Before it, the payload
    // compiled each card from its saved spec alone, so a dashboard's filters —
    // even its own defaults — never reached the query and the emailed image
    // showed different numbers from the page it claimed to be.
    //
    // Asserted on the SQL the engine was handed, not on the rows: the fake
    // returns the same rows whatever it is asked, exactly as a real engine
    // would return the same rows for two queries that happened to match. Only
    // the statement itself distinguishes "the filter was applied" from "the
    // filter was dropped".
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    let (_, body) = get_payload(&fx, &token).await;
    let chart_id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["cards"][0]
        ["chart_id"]
        .as_str()
        .unwrap()
        .to_string();

    let (s, body) = post_data(
        &fx,
        &token,
        serde_json::json!([{
            "chart_id": chart_id,
            "filters": [{"col": "orderpriority", "op": "eq", "value": "1-URGENT"}],
        }]),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let sql = fx.seen_sql.lock().unwrap().clone();
    let last = sql.last().expect("the engine was asked something");
    assert!(
        last.contains("1-URGENT"),
        "the resolved filter never reached the query: {last}"
    );
    assert!(
        last.contains("orderpriority"),
        "the filter landed on the wrong column: {last}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_filter_on_a_column_the_dataset_does_not_have_is_refused(pool: sqlx::PgPool) {
    // The filters come from a browser, so the column has to be checked against
    // the dataset rather than trusted — `compile_chart_sql` -> `find_column`.
    // Without it, whoever holds a render token could predicate on any column,
    // running as the schedule's owner.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    let (_, body) = get_payload(&fx, &token).await;
    let chart_id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["cards"][0]
        ["chart_id"]
        .as_str()
        .unwrap()
        .to_string();

    let (s, body) = post_data(
        &fx,
        &token,
        serde_json::json!([{
            "chart_id": chart_id,
            "filters": [{"col": "no_such_column", "op": "eq", "value": "x"}],
        }]),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        fx.seen_sql.lock().unwrap().is_empty(),
        "a rejected filter still reached the engine"
    );
}

/// Why the render routes are mounted on a router of their own.
///
/// `POST /render/data` cannot live behind the CSRF layer: double-submit CSRF
/// defends requests the browser authenticates AMBIENTLY, and there is nothing
/// ambient here — the renderer sends a bearer token this code sets, and has no
/// cookie to echo back. Behind the layer the call is refused before it reaches
/// the handler, which is exactly what shipped: every real report failed with
/// the renderer reporting only "the print page reported a failure", because
/// these tests mount the router DIRECTLY and never see the layer.
///
/// The composition itself lives in `egret-server`; this pins the reason, so
/// anyone folding the two routers back together fails here first.
#[sqlx::test(migrations = "../../migrations")]
async fn the_data_call_cannot_live_behind_the_csrf_layer(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;

    let layered = fx
        .app
        .clone()
        .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer));
    let res = layered
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/render/data")
                .header("Authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from("[]"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "the layer let a cookie-less POST through, so this test no longer says anything"
    );

    // The same request, on the router as it is actually mounted.
    assert_eq!(
        post_data(&fx, &token, serde_json::json!([])).await.0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_chart_outside_the_render_is_refused(pool: sqlx::PgPool) {
    // The token authorises ONE object. Without this the body would choose what
    // runs, and a report on a harmless dashboard would read any chart in the
    // workspace as the schedule's owner.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    let (s, body) = post_data(
        &fx,
        &token,
        serde_json::json!([{"chart_id": uuid::Uuid::new_v4()}]),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("not part of this render"), "{body}");
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

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
