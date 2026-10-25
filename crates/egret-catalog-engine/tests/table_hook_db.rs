//! D2 (StarRocks whole-table classification, spec §5.2 and §5.5): the column
//! source read against the REAL embedded Lakekeeper (the drift guard C3), and
//! the table-commit hook.
//!
//! Each test drives Lakekeeper (on its in-memory storage) through
//! [`front_catalog_router`].
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-catalog-engine --test table_hook_db

use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, load_table_detail, run_lakekeeper_migrations, set_delete_profile,
    ApiContext, EngineState, LAKEKEEPER_SCHEMA,
};
use egret_metastore::Db;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// lakekeeper's schema and default project, once per test process.
async fn lakekeeper_ready(dsn: &str) {
    LAKEKEEPER
        .get_or_init(|| async {
            run_lakekeeper_migrations(dsn)
                .await
                .expect("lakekeeper migrations");
            let (state, _registry) = build_state(dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
}

struct Env {
    db: Db,
    state: ApiContext<EngineState>,
    /// Lakekeeper with no hook in front.
    raw: axum::Router,
    prefix: String,
    warehouse_id: Uuid,
}

/// A lakekeeper warehouse with namespace `sales`, and the egret warehouse over
/// it.
async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    lakekeeper_ready(&dsn).await;
    let (state, _registry) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("thwh"))
        .await
        .expect("lakekeeper warehouse");
    let pool = &db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'TH') RETURNING id")
            .bind(unique("th-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-th-test','AKIATEST') RETURNING id",
    )
    .bind(unique("th-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(unique("th_wh"))
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let env = Env {
        db,
        state,
        raw,
        prefix,
        warehouse_id,
    };
    let (status, body) = env
        .call_raw("POST", "namespaces", Some(json!({"namespace": ["sales"]})))
        .await;
    assert_eq!(status, StatusCode::OK, "create namespace: {body}");
    Some(env)
}

impl Env {
    async fn call_raw(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        send(&self.raw, &self.prefix, method, path, body).await
    }

    /// What the column source answers for `sales.{table}`.
    async fn source(&self, table: &str) -> Option<Vec<String>> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm_compile::table_columns::current_columns_via(
            &mut conn,
            self.warehouse_id,
            "sales",
            table,
        )
        .await
        .expect("column source")
        .map(|c| c.columns)
    }

    /// The top-level column names Lakekeeper itself loads for `sales.{table}`.
    async fn loaded(&self, table: &str) -> Vec<String> {
        load_table_detail(
            self.state.clone(),
            SYSTEM_USER,
            &self.prefix,
            &["sales".to_string()],
            table,
        )
        .await
        .expect("load table")
        .columns
        .into_iter()
        .map(|c| c.name)
        .collect()
    }
}

async fn send(
    app: &axum::Router,
    prefix: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method(method)
        .uri(format!("/{prefix}/{path}"))
        .header("content-type", "application/json")
        .body(match body {
            Some(b) => Body::from(b.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// An Iceberg schema whose fields are `(id, name)`.
fn schema(id: i64, fields: &[(i64, &str)]) -> Value {
    let fields: Vec<Value> = fields
        .iter()
        .map(|(i, c)| json!({"id": i, "name": c, "type": "string", "required": false}))
        .collect();
    json!({"type": "struct", "schema-id": id, "fields": fields})
}

/// The commit that adds `schema` and makes it current.
fn schema_commit(schema: Value) -> Value {
    json!({
        "requirements": [],
        "updates": [
            {"action": "add-schema", "schema": schema},
            {"action": "set-current-schema", "schema-id": -1}
        ]
    })
}

/// C3, the drift guard: after every kind of schema change Lakekeeper accepts,
/// the column source answers exactly the top-level names Lakekeeper itself
/// loads (order included); a renamed or dropped table answers `None` under its
/// old name, and a view or a staged create is no table.
#[tokio::test]
async fn the_column_source_reads_what_lakekeeper_loads() {
    let Some(env) = env().await else { return };
    assert_eq!(
        egret_perm_compile::table_columns::LAKEKEEPER_SCHEMA,
        LAKEKEEPER_SCHEMA,
        "the column source's SQL names Lakekeeper's schema"
    );

    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({
                "name": "orders",
                "schema": schema(0, &[(1, "id"), (2, "email"), (3, "region")]),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create: {body}");
    assert_eq!(env.source("orders").await, Some(env.loaded("orders").await));
    assert_eq!(
        env.source("orders").await,
        Some(vec!["id".into(), "email".into(), "region".into()])
    );

    let steps: [(&str, Vec<(i64, &str)>); 3] = [
        (
            "add phone",
            vec![(1, "id"), (2, "email"), (3, "region"), (4, "phone")],
        ),
        (
            "rename region to area",
            vec![(1, "id"), (2, "email"), (3, "area"), (4, "phone")],
        ),
        ("drop phone", vec![(1, "id"), (2, "email"), (3, "area")]),
    ];
    for (i, (what, fields)) in steps.iter().enumerate() {
        let (status, body) = env
            .call_raw(
                "POST",
                "namespaces/sales/tables/orders",
                Some(schema_commit(schema(i as i64 + 1, fields))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{what}: {body}");
        let want: Vec<String> = fields.iter().map(|(_, n)| n.to_string()).collect();
        assert_eq!(env.loaded("orders").await, want, "{what}: lakekeeper");
        assert_eq!(
            env.source("orders").await,
            Some(want),
            "{what}: column source"
        );
    }
    // The lookup folds case; the answer is the catalog's spelling.
    assert_eq!(env.source("ORDERS").await, env.source("orders").await);

    let (status, body) = env
        .call_raw(
            "POST",
            "tables/rename",
            Some(json!({
                "source": {"namespace": ["sales"], "name": "orders"},
                "destination": {"namespace": ["sales"], "name": "orders_v2"},
            })),
        )
        .await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert_eq!(
        env.source("orders").await,
        None,
        "the old name after a rename"
    );
    assert_eq!(
        env.source("orders_v2").await,
        Some(env.loaded("orders_v2").await)
    );

    // A SOFT drop keeps the row with `deleted_at` set: the predicate that
    // matters is exercised only then.
    set_delete_profile(env.state.clone(), SYSTEM_USER, &env.prefix, Some(3600))
        .await
        .expect("soft delete profile");
    let (status, body) = env
        .call_raw("DELETE", "namespaces/sales/tables/orders_v2", None)
        .await;
    assert!(status.is_success(), "drop: {status} {body}");
    let soft: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM lakekeeper.tabular \
          WHERE warehouse_id::text = $1 AND name = 'orders_v2' AND deleted_at IS NOT NULL",
    )
    .bind(&env.prefix)
    .fetch_one(&env.db.pool)
    .await
    .expect("soft-deleted row");
    assert_eq!(
        soft, 1,
        "the drop must be soft for this step to test anything"
    );
    assert_eq!(env.source("orders_v2").await, None, "a dropped table");

    // A view of a table's name is no table.
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/views",
            Some(json!({
                "name": "v_orders",
                "schema": schema(0, &[(1, "id")]),
                "view-version": {
                    "version-id": 1, "schema-id": 0,
                    "timestamp-ms": 1_700_000_000_000_i64,
                    "summary": {"engine-name": "trino"},
                    "default-namespace": ["sales"],
                    "representations": [{"type": "sql", "sql": "SELECT 1 AS id", "dialect": "trino"}]
                },
                "properties": {}
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "create view: {body}");
    assert_eq!(env.source("v_orders").await, None, "a view");

    // A staged create has no metadata location until its committing update.
    let (status, body) = env
        .call_raw(
            "POST",
            "namespaces/sales/tables",
            Some(json!({
                "name": "staged",
                "schema": schema(0, &[(1, "id")]),
                "stage-create": true,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "stage create: {body}");
    assert_eq!(env.source("staged").await, None, "a staged create");
}
