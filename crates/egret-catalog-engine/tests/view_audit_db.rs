//! View governance B5 (Task 10): the DEFINER audit's reader. It lists the
//! views of egret's catalogs and reads each one's stored `trino.run-as-owner`
//! from the REAL embedded lakekeeper (on its in-memory storage).
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-catalog-engine --test view_audit_db

use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, run_lakekeeper_migrations, CatalogView, EmbeddedViewCatalog, ViewCatalog,
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
    raw: axum::Router,
    prefix: String,
    catalog: String,
    audit: EmbeddedViewCatalog,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    lakekeeper_ready(&dsn).await;
    let (state, _registry) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("vawh"))
        .await
        .expect("lakekeeper warehouse");
    let pool = &db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'VA') RETURNING id")
            .bind(unique("va-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-va-test','AKIATEST') RETURNING id",
    )
    .bind(unique("va-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(unique("va_wh"))
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let catalog = unique("va_cat");
    sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2)",
    )
    .bind(&catalog)
    .bind(warehouse_id)
    .execute(pool)
    .await
    .expect("catalog");
    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let audit = EmbeddedViewCatalog::new(db, state, SYSTEM_USER).only_warehouses(&[warehouse_id]);
    let env = Env {
        raw,
        prefix,
        catalog,
        audit,
    };
    for ns in [json!(["s"]), json!(["s", "deep"])] {
        let (status, body) = env
            .call("POST", "namespaces", Some(json!({"namespace": ns})))
            .await;
        assert_eq!(status, StatusCode::OK, "create namespace: {body}");
    }
    Some(env)
}

impl Env {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = axum::http::Request::builder()
            .method(method)
            .uri(format!("/{}/{path}", self.prefix))
            .header("content-type", "application/json")
            .body(match body {
                Some(b) => Body::from(b.to_string()),
                None => Body::empty(),
            })
            .unwrap();
        let resp = self.raw.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A view in namespace `ns` (URL-encoded levels) with `properties`.
    async fn create(&self, ns: &str, name: &str, properties: Value) {
        let body = json!({
            "name": name,
            "schema": {"type": "struct", "schema-id": 0, "fields": [
                {"id": 1, "name": "id", "type": "string", "required": false}
            ]},
            "view-version": {
                "version-id": 1,
                "schema-id": 0,
                "timestamp-ms": chrono::Utc::now().timestamp_millis(),
                "summary": {"engine-name": "trino", "engine-version": "483"},
                "default-namespace": ["s"],
                "representations": [{"type": "sql", "sql": "SELECT 1 AS id", "dialect": "trino"}]
            },
            "properties": properties
        });
        let (status, body) = self
            .call("POST", &format!("namespaces/{ns}/views"), Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "create {name}: {body}");
    }

    fn view(&self, name: &str) -> CatalogView {
        CatalogView {
            catalog: self.catalog.clone(),
            schema: "s".into(),
            name: name.into(),
            prefix: self.prefix.clone(),
        }
    }
}

/// Live result §1 Q2: a DEFINER view carries `trino.run-as-owner = <owner>`,
/// an INVOKER view does not. The reader returns the owner, or none.
#[tokio::test]
async fn the_stored_run_as_owner_is_the_definer_owner_and_its_absence_is_invoker() {
    let Some(env) = env().await else { return };
    env.create("s", "v_definer", json!({"trino.run-as-owner": "vgadmin"}))
        .await;
    env.create("s", "v_invoker", json!({"comment": "x"})).await;

    assert_eq!(
        env.audit.run_as_owner(&env.view("v_definer")).await,
        Ok(Some("vgadmin".to_string()))
    );
    assert_eq!(
        env.audit.run_as_owner(&env.view("v_invoker")).await,
        Ok(None)
    );
}

/// A view whose metadata cannot be read is an error, never INVOKER: the audit
/// then says "unknown".
#[tokio::test]
async fn a_view_that_cannot_be_loaded_is_an_error_never_invoker() {
    let Some(env) = env().await else { return };

    let got = env.audit.run_as_owner(&env.view("v_gone")).await;

    assert!(got.is_err(), "{got:?}");
}

/// The listing names every view of a single-level namespace under the
/// registry catalog name, and skips tables and multi-level namespaces.
#[tokio::test]
async fn the_listing_names_the_views_of_single_level_namespaces() {
    let Some(env) = env().await else { return };
    env.create("s", "v_one", json!({})).await;
    env.create("s", "v_two", json!({"trino.run-as-owner": "o"}))
        .await;
    env.create("s%1Fdeep", "v_deep", json!({})).await;

    let listed = env.audit.list_views().await.expect("listing");

    assert!(listed.failures.is_empty(), "{:?}", listed.failures);
    let mut names: Vec<_> = listed.views.iter().map(|v| v.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["v_one", "v_two"]);
    assert!(listed
        .views
        .iter()
        .all(|v| v.catalog == env.catalog && v.schema == "s" && v.prefix == env.prefix));
}
