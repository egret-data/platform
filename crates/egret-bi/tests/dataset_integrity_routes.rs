//! DB-gated dataset/metric INTEGRITY route tests (B2: KI-098, KI-099, KI-101, KI-102, KI-110,
//! KI-166). Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
//!
//! Through the ROUTER, because each defect was a request that should have been refused and was
//! not: a delete that took the charts with it, a delete of a metric charts use, an unreachable
//! empty column name. The fixture writes the dataset, metric and charts directly so no engine is
//! needed; the paths that DO need one (a metric rename's expression probe) are covered at the
//! repository level here and against Trino in `chart_engine.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dataset_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
    SchemaService,
};
use egret_metastore::Db;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        saved: SavedQueryRepo::new(db.clone()),
        schema: SchemaService::new(),
        databases: egret_query::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        charts: ChartRepo::new(db.clone()),
        datasets: DatasetRepo::new(db.clone()),
        dashboards: DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

struct Fx {
    app: axum::Router,
    slug: String,
    ws: Uuid,
    /// ws_editor: dataset:manage.
    editor: String,
    /// ws_viewer: dataset:read only.
    viewer: String,
    owner: Uuid,
    ds: Uuid,
    metric: Uuid,
    db: Db,
}

async fn user(db: &Db, ws: Uuid, role: &str) -> Uuid {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("b2_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3",
    )
    .bind(ws)
    .bind(id)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("grant")
    .rows_affected();
    assert_eq!(n, 1, "{role} must exist");
    id
}

async fn setup() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let creator: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("b2c_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let slug = format!("b2-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'B2 Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(creator)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let owner = user(&db, ws, "ws_editor").await;
    let viewer_id = user(&db, ws, "ws_viewer").await;

    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, options) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, '{}'::jsonb) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ds: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, description, catalog, schema_name, table_name, created_by) \
         VALUES ($1,$2,'b2-orders','', 'tpch', 'tiny', 'orders', $3) RETURNING id",
    )
    .bind(ws)
    .bind(db_id)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    for (i, (name, ty, tg, temporal)) in [
        ("orderpriority", "varchar(15)", "string", false),
        ("totalprice", "double", "numeric", false),
        ("orderdate", "date", "temporal", true),
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO analytics.dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal) \
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(ds)
        .bind(name)
        .bind(ty)
        .bind(tg)
        .bind(temporal)
        .bind(i as i32)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let metric: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.dataset_metrics \
           (dataset_id, metric_name, expression, data_type, type_generic, verbose_name, description, format, ordinal) \
         VALUES ($1,'revenue','sum(totalprice)','double','numeric','Revenue','','',0) RETURNING id",
    )
    .bind(ds)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let session = |u: Uuid| {
        let st = st.clone();
        async move {
            st.sessions
                .create(egret_core::ids::UserId(u), "test", "127.0.0.1")
                .await
                .expect("session")
                .token
        }
    };
    let editor = session(owner).await;
    let viewer = session(viewer_id).await;
    Some(Fx {
        app: dataset_router().with_state(st),
        slug,
        ws,
        editor,
        viewer,
        owner,
        ds,
        metric,
        db,
    })
}

/// A chart on the fixture dataset, written directly (as `ch_create` stores it).
async fn chart(fx: &Fx, name: &str, viz: &str, params: serde_json::Value) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO analytics.charts (workspace_id, dataset_id, name, description, viz_type, params, created_by) \
         VALUES ($1,$2,$3,'',$4,$5,$6) RETURNING id",
    )
    .bind(fx.ws)
    .bind(fx.ds)
    .bind(name)
    .bind(viz)
    .bind(params)
    .bind(fx.owner)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap()
}

fn bar_on_revenue() -> serde_json::Value {
    json!({
        "version": 2,
        "spec": {"metrics": ["revenue"], "dimensions": ["orderpriority"], "columns": [],
                 "filters": [], "orderby": [{"field": "revenue", "desc": true}],
                 "row_limit": 100, "rollup_groups": []},
        "settings": {"cartesian.xAxis": "orderpriority", "cartesian.yMetrics": ["revenue"]}
    })
}

fn table_of_columns() -> serde_json::Value {
    json!({
        "version": 2,
        "spec": {"metrics": [], "dimensions": [], "columns": ["orderpriority"], "filters": [],
                 "orderby": [], "row_limit": 100, "rollup_groups": []},
        "settings": {}
    })
}

async fn req(
    fx: &Fx,
    tok: &str,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Cookie", format!("egret_session={tok}"));
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    let res = fx
        .app
        .clone()
        .oneshot(
            b.body(
                body.map(|v| Body::from(v.to_string()))
                    .unwrap_or(Body::empty()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn ds_path(fx: &Fx) -> String {
    format!("/ws/{}/datasets/{}", fx.slug, fx.ds)
}

async fn chart_exists(fx: &Fx, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM analytics.charts WHERE id = $1)")
        .bind(id)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap()
}

fn chart_ids(body: &serde_json::Value) -> Vec<String> {
    body["charts"]
        .as_array()
        .expect("charts listed")
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

// ── KI-098 ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deleting_a_dataset_that_charts_use_is_refused_with_the_charts() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let live = chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    let trashed = chart(&fx, "b2-table", "table", table_of_columns()).await;
    sqlx::query("UPDATE analytics.charts SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    let (st, body) = req(&fx, &fx.editor, "DELETE", &ds_path(&fx), None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "in_use");
    // The trashed chart is listed too: the cascade would delete it as well.
    assert_eq!(chart_ids(&body), [live.to_string(), trashed.to_string()]);
    assert_eq!(body["charts"][0]["viz_type"], "bar");
    assert_eq!(body["charts"][0]["trashed"], false);
    assert_eq!(body["charts"][1]["trashed"], true);
    assert!(body["error"].as_str().unwrap().starts_with("2 charts"));
    assert!(chart_exists(&fx, live).await, "nothing was deleted");

    // A wrong name is no confirmation.
    let (st, _) = req(
        &fx,
        &fx.editor,
        "DELETE",
        &format!("{}?confirm=b2-ORDERS", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // The typed name is: dataset and charts go.
    let (st, body) = req(
        &fx,
        &fx.editor,
        "DELETE",
        &format!("{}?confirm=b2-orders", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!chart_exists(&fx, live).await);
    assert!(!chart_exists(&fx, trashed).await);
}

#[tokio::test]
async fn deleting_a_dataset_no_chart_uses_needs_no_confirmation() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (st, body) = req(&fx, &fx.editor, "DELETE", &ds_path(&fx), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_viewer_cannot_delete_even_with_the_name() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (st, _) = req(
        &fx,
        &fx.viewer,
        "DELETE",
        &format!("{}?confirm=b2-orders", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

// ── KI-099 ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deleting_a_metric_that_charts_use_is_refused_unless_confirmed() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let bar = chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    let _other = chart(&fx, "b2-table", "table", table_of_columns()).await;
    let path = format!("{}/metrics/{}", ds_path(&fx), fx.metric);

    let (st, body) = req(&fx, &fx.editor, "DELETE", &path, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "in_use");
    // Only the chart that names the metric, not every chart on the dataset.
    assert_eq!(chart_ids(&body), [bar.to_string()]);

    let (st, body) = req(
        &fx,
        &fx.editor,
        "DELETE",
        &format!("{path}?confirm=revenue"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["metrics"], json!([]));
}

#[tokio::test]
async fn a_metric_only_sorted_by_still_counts_as_used() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let mut p = table_of_columns();
    p["spec"]["orderby"] = json!([{"field": "revenue", "desc": true}]);
    let c = chart(&fx, "b2-sorted", "table", p).await;
    let (st, body) = req(
        &fx,
        &fx.editor,
        "DELETE",
        &format!("{}/metrics/{}", ds_path(&fx), fx.metric),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(chart_ids(&body), [c.to_string()]);
}

#[tokio::test]
async fn renaming_a_used_metric_onto_a_column_name_is_refused_before_the_engine() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let bar = chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    // `totalprice` is a column: `"totalprice"` in the settings would then mean either.
    let (st, body) = req(
        &fx,
        &fx.editor,
        "PUT",
        &format!("{}/metrics/{}", ds_path(&fx), fx.metric),
        Some(json!({"metric_name": "totalprice", "expression": "sum(totalprice)"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "in_use");
    assert_eq!(chart_ids(&body), [bar.to_string()]);
}

#[tokio::test]
async fn rename_metric_rewrites_the_charts_in_the_same_transaction() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let bar = chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    let other = chart(&fx, "b2-table", "table", table_of_columns()).await;
    let before_other: serde_json::Value =
        sqlx::query_scalar("SELECT params FROM analytics.charts WHERE id = $1")
            .bind(other)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    // A dashboard card of the bar chart with a per-card override keyed by the metric.
    let dash: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.dashboards (workspace_id, name, created_by) VALUES ($1,'b2-dash',$2) RETURNING id",
    )
    .bind(fx.ws)
    .bind(fx.owner)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let card: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.dashboard_cards (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal) \
         VALUES ($1,$2,'chart',0,0,4,4,$3,0) RETURNING id",
    )
    .bind(dash)
    .bind(bar)
    .bind(json!({"vizOverride": {"cartesian.yMetrics": ["revenue"]}, "title": "revenue"}))
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();

    let repo = DatasetRepo::new(fx.db.clone());
    let used = repo.charts_using_metric(fx.ds, "revenue").await.unwrap();
    assert_eq!(used.iter().map(|c| c.id).collect::<Vec<_>>(), [bar]);
    repo.rename_metric(
        fx.ds,
        fx.metric,
        "revenue",
        "sales",
        "sum(totalprice)",
        "double",
        "numeric",
        "Revenue",
        "",
        "",
    )
    .await
    .unwrap();

    let params: serde_json::Value =
        sqlx::query_scalar("SELECT params FROM analytics.charts WHERE id = $1")
            .bind(bar)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(params["spec"]["metrics"], json!(["sales"]));
    assert_eq!(params["spec"]["orderby"][0]["field"], "sales");
    assert_eq!(params["settings"]["cartesian.yMetrics"], json!(["sales"]));
    assert_eq!(params["settings"]["cartesian.xAxis"], "orderpriority");
    let after_other: serde_json::Value =
        sqlx::query_scalar("SELECT params FROM analytics.charts WHERE id = $1")
            .bind(other)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        after_other, before_other,
        "a chart without the metric is untouched"
    );
    let options: serde_json::Value =
        sqlx::query_scalar("SELECT options FROM analytics.dashboard_cards WHERE id = $1")
            .bind(card)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        options["vizOverride"]["cartesian.yMetrics"],
        json!(["sales"])
    );
    // Outside `vizOverride` a card's options are not settings and are left alone.
    assert_eq!(options["title"], "revenue");
    assert!(repo
        .charts_using_metric(fx.ds, "revenue")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn rename_metric_to_a_taken_name_changes_no_chart() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let bar = chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    sqlx::query(
        "INSERT INTO analytics.dataset_metrics (dataset_id, metric_name, expression, data_type, type_generic, ordinal) \
         VALUES ($1,'orders','count(*)','bigint','numeric',1)",
    )
    .bind(fx.ds)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let repo = DatasetRepo::new(fx.db.clone());
    let err = repo
        .rename_metric(
            fx.ds,
            fx.metric,
            "revenue",
            "orders",
            "sum(totalprice)",
            "double",
            "numeric",
            "",
            "",
            "",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, egret_bi::BiError::Conflict(_)), "{err:?}");
    let params: serde_json::Value =
        sqlx::query_scalar("SELECT params FROM analytics.charts WHERE id = $1")
            .bind(bar)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(params["spec"]["metrics"], json!(["revenue"]));
}

// ── KI-101 ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_empty_or_blank_column_name_is_a_400() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    for name in ["", "   "] {
        let (st, body) = req(
            &fx,
            &fx.editor,
            "POST",
            &format!("{}/columns", ds_path(&fx)),
            Some(json!({"column_name": name, "expression": "1"})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{name:?}: {body}");
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("column name is required"));
    }
}

// ── KI-102 ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn refresh_keeps_the_time_switch_and_names_what_changed() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    // The user turned Time ON for a varchar column and OFF for the date column.
    sqlx::query(
        "UPDATE analytics.dataset_columns SET is_temporal = (column_name = 'orderpriority') \
         WHERE dataset_id = $1 AND column_name IN ('orderpriority','orderdate')",
    )
    .bind(fx.ds)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let col = |n: &str, ty: &str, tg: &str, t: bool| NewColumn {
        column_name: n.into(),
        data_type: ty.into(),
        type_generic: tg.into(),
        is_temporal: t,
    };
    // The source now: `totalprice` changed type, `orderdate` unchanged, `clerk` new, nothing
    // else (no `comment`, and `orderpriority` kept its type).
    let fresh = [
        col("orderpriority", "varchar(15)", "string", false),
        col("totalprice", "timestamp(3)", "temporal", true),
        col("orderdate", "date", "temporal", true),
        col("clerk", "varchar(15)", "string", false),
    ];
    let repo = DatasetRepo::new(fx.db.clone());
    let changes = repo
        .reconcile_physical_columns(fx.ds, &fresh)
        .await
        .unwrap();
    assert_eq!(changes.added, ["clerk"]);
    assert!(changes.removed.is_empty());

    let temporal: Vec<(String, bool)> = sqlx::query_as(
        "SELECT column_name, is_temporal FROM analytics.dataset_columns WHERE dataset_id = $1 ORDER BY ordinal",
    )
    .bind(fx.ds)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        temporal,
        [
            ("orderpriority".to_string(), true), // the user's ON survives
            ("totalprice".to_string(), true),    // type changed: the engine's answer
            ("orderdate".to_string(), false),    // the user's OFF survives
            ("clerk".to_string(), false),
        ]
    );

    let changes = repo
        .reconcile_physical_columns(fx.ds, &fresh[..2])
        .await
        .unwrap();
    assert!(changes.added.is_empty());
    assert_eq!(changes.removed, ["orderdate", "clerk"]);
}

// ── KI-110 ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn duplicate_copies_a_physical_dataset_with_its_settings_and_metrics() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    sqlx::query(
        "UPDATE analytics.dataset_columns SET verbose_name='Priority', groupby=false, filterable=false, \
         format='0.00', semantic_type='category' WHERE dataset_id=$1 AND column_name='orderpriority'",
    )
    .bind(fx.ds)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO analytics.dataset_columns (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, expression) \
         VALUES ($1,'net','double','numeric',false,3,'totalprice * 0.9')",
    )
    .bind(fx.ds)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE analytics.dataset_metrics SET certified=true, certified_by='x@y' WHERE id=$1",
    )
    .bind(fx.metric)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    let (st, body) = req(
        &fx,
        &fx.editor,
        "POST",
        &format!("{}/duplicate", ds_path(&fx)),
        Some(json!({"name": "b2-orders copy"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "b2-orders copy");
    assert_eq!(body["kind"], "physical");
    assert_eq!(body["table"], "orders");
    assert_ne!(body["id"], fx.ds.to_string());
    let cols = body["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 4);
    let p = &cols[0];
    assert_eq!(p["column_name"], "orderpriority");
    assert_eq!(p["verbose_name"], "Priority");
    assert_eq!(p["groupby"], false);
    assert_eq!(p["filterable"], false);
    assert_eq!(p["format"], "0.00");
    assert_eq!(p["semantic_type"], "category");
    assert_eq!(cols[3]["expression"], "totalprice * 0.9");
    assert_eq!(body["metrics"][0]["metric_name"], "revenue");
    assert_eq!(body["metrics"][0]["verbose_name"], "Revenue");
    assert_eq!(
        body["metrics"][0]["certified"], false,
        "a copy has not been vouched for"
    );

    // The name is unique per workspace.
    let (st, _) = req(
        &fx,
        &fx.editor,
        "POST",
        &format!("{}/duplicate", ds_path(&fx)),
        Some(json!({"name": "b2-orders copy"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_viewer_cannot_duplicate() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let (st, _) = req(
        &fx,
        &fx.viewer,
        "POST",
        &format!("{}/duplicate", ds_path(&fx)),
        Some(json!({"name": "b2-viewer copy"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.datasets WHERE workspace_id = $1 AND name = 'b2-viewer copy'",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}

// ── KI-166 / KI-111 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn usage_names_each_charts_type_and_detail_says_when_trashed() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    chart(&fx, "b2-table", "table", table_of_columns()).await;
    let (st, body) = req(
        &fx,
        &fx.viewer,
        "GET",
        &format!("{}/usage", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let mut types: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["viz_type"].as_str().unwrap())
        .collect();
    types.sort();
    assert_eq!(types, ["bar", "table"]);

    let (_, d) = req(&fx, &fx.viewer, "GET", &ds_path(&fx), None).await;
    assert_eq!(d["deleted_at"], serde_json::Value::Null);
    let (st, _) = req(
        &fx,
        &fx.editor,
        "POST",
        &format!("{}/trash", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, d) = req(&fx, &fx.viewer, "GET", &ds_path(&fx), None).await;
    assert!(d["deleted_at"].is_string(), "{d}");
}

#[tokio::test]
async fn usage_with_trashed_lists_every_chart_a_delete_would_take() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    let live = chart(&fx, "b2-bar", "bar", bar_on_revenue()).await;
    let gone = chart(&fx, "b2-table", "table", table_of_columns()).await;
    sqlx::query("UPDATE analytics.charts SET deleted_at = now() WHERE id = $1")
        .bind(gone)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let (_, body) = req(
        &fx,
        &fx.viewer,
        "GET",
        &format!("{}/usage", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(chart_ids(&json!({"charts": body})), [live.to_string()]);
    let (_, body) = req(
        &fx,
        &fx.viewer,
        "GET",
        &format!("{}/usage?include_trashed=true", ds_path(&fx)),
        None,
    )
    .await;
    assert_eq!(
        chart_ids(&json!({"charts": body})),
        [live.to_string(), gone.to_string()]
    );
    assert_eq!(body[1]["trashed"], true);
}
