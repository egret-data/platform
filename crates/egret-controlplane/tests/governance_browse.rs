//! `GET /api/control-plane/governance/browse/{catalog_id}` (DG frontend
//! redesign #1, spec §2.3): one level of a catalog for the Governance Data
//! tree. egret's own Iceberg catalogs are read from the embedded warehouse;
//! every other catalog through ONE engine egret provisioned, that is running
//! and attached to it. DB-backed through the per-process test DB; the engine
//! is a fake `CatalogLister` that records every call, and the warehouse a
//! fake `WarehouseBrowser`, so no live engine or lakekeeper is needed.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog_engine::{
    BrowseColumn as WhColumn, BrowseNamespace, BrowseTableDetail, BrowseTabular,
    CatalogEngineError, WarehouseBrowser,
};
use egret_controlplane::{
    control_plane_router, BrowseColumn, BrowseItem, BrowseKind, CatalogLister, CpState,
    ExposureEngine, Listing, ListingEngine, ListingEngineType, MaterializedViewLister, MvRow,
};
use egret_metastore::Db;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

/// Allows exactly the actions it was built with, so a 403 is the permission
/// gate and not an accident of an empty role table.
struct AllowOnly(&'static [&'static str]);

#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowOnly {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: self.0.contains(&a),
            reasons: vec![],
        })
    }
}

/// One recorded `CatalogLister::list` call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    engine: String,
    engine_type: ListingEngineType,
    catalog: String,
    schema: Option<String>,
    table: Option<String>,
}

/// What the fake engine answers.
#[derive(Clone)]
enum Answer {
    /// Schemas at the catalog level, tables at the schema level, columns at
    /// the table level.
    Levels,
    Fail(&'static str),
    /// Never answers, like an engine that accepts the connection and goes
    /// silent.
    Hang,
}

struct FakeLister {
    answer: Answer,
    calls: Mutex<Vec<Call>>,
}

impl FakeLister {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(FakeLister {
            answer,
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl CatalogLister for FakeLister {
    async fn list(
        &self,
        engine: &ListingEngine,
        catalog: &str,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<Listing, String> {
        self.calls.lock().unwrap().push(Call {
            engine: engine.name.clone(),
            engine_type: engine.engine_type,
            catalog: catalog.to_string(),
            schema: schema.map(String::from),
            table: table.map(String::from),
        });
        match &self.answer {
            Answer::Fail(e) => Err(e.to_string()),
            Answer::Hang => {
                std::future::pending::<()>().await;
                unreachable!()
            }
            Answer::Levels => Ok(match (schema, table) {
                (None, _) => Listing::Items(vec![BrowseItem {
                    name: "sales".into(),
                    kind: BrowseKind::Schema,
                }]),
                (Some(_), None) => Listing::Items(vec![
                    BrowseItem {
                        name: "orders".into(),
                        kind: BrowseKind::Table,
                    },
                    BrowseItem {
                        name: "v_orders".into(),
                        kind: BrowseKind::View,
                    },
                ]),
                (Some(_), Some(_)) => Listing::Columns(vec![BrowseColumn {
                    name: "id".into(),
                    r#type: "integer".into(),
                }]),
            }),
        }
    }
}

/// The MV lister is not this route's; it must never be asked.
struct NoMvLister;

#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("the browse route never lists materialized views")
    }
}

/// A warehouse with `sales` (table `orders`, view `v_orders`) and the nested
/// namespace `a.b`. Records every lakekeeper warehouse id it is asked about.
#[derive(Default)]
struct FakeWarehouse {
    asked: Mutex<Vec<String>>,
}

fn table_detail(columns: &[(&str, &str)]) -> BrowseTableDetail {
    BrowseTableDetail {
        location: "s3://b/t".into(),
        format_version: 2,
        last_updated_ms: 0,
        current_snapshot_id: None,
        snapshot_count: 0,
        columns: columns
            .iter()
            .map(|(n, t)| WhColumn {
                name: n.to_string(),
                type_: t.to_string(),
                required: false,
            })
            .collect(),
        partition_fields: vec![],
        sort_fields: vec![],
        properties: Default::default(),
        snapshots: vec![],
    }
}

#[async_trait::async_trait]
impl WarehouseBrowser for FakeWarehouse {
    async fn namespaces(&self, wh: &str) -> Result<Vec<BrowseNamespace>, CatalogEngineError> {
        self.asked.lock().unwrap().push(wh.to_string());
        Ok(vec![
            BrowseNamespace {
                parts: vec!["sales".into()],
                id: None,
                protected: false,
            },
            BrowseNamespace {
                parts: vec!["a".into(), "b".into()],
                id: None,
                protected: false,
            },
        ])
    }
    async fn tabulars(
        &self,
        wh: &str,
        ns: &[String],
    ) -> Result<Vec<BrowseTabular>, CatalogEngineError> {
        self.asked.lock().unwrap().push(wh.to_string());
        if ns != ["sales".to_string()] {
            return Err(CatalogEngineError::NotFound(format!("namespace {ns:?}")));
        }
        Ok(vec![
            BrowseTabular {
                name: "orders".into(),
                kind: "table".into(),
                id: None,
                protected: false,
            },
            BrowseTabular {
                name: "v_orders".into(),
                kind: "view".into(),
                id: None,
                protected: false,
            },
        ])
    }
    async fn schema(
        &self,
        _wh: &str,
        _ns: &[String],
        table: &str,
    ) -> Result<BrowseTableDetail, CatalogEngineError> {
        assert_eq!(table, "orders", "only a table's schema is loaded");
        Ok(table_detail(&[("id", "long"), ("amount", "decimal(10,2)")]))
    }
    async fn view_columns(
        &self,
        _wh: &str,
        _ns: &[String],
        view: &str,
    ) -> Result<Vec<WhColumn>, CatalogEngineError> {
        assert_eq!(view, "v_orders", "only a view's columns are loaded");
        Ok(vec![WhColumn {
            name: "total".into(),
            type_: "double".into(),
            required: false,
        }])
    }
    async fn deleted_tabulars(
        &self,
        _wh: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseDeletedTabular>, CatalogEngineError> {
        unimplemented!()
    }
    async fn undrop(&self, _wh: &str, _k: &str, _id: Uuid) -> Result<(), CatalogEngineError> {
        unimplemented!()
    }
    async fn set_protection(
        &self,
        _wh: &str,
        _t: &str,
        _id: Uuid,
        _p: bool,
    ) -> Result<bool, CatalogEngineError> {
        unimplemented!()
    }
    async fn config(
        &self,
        _wh: &str,
    ) -> Result<egret_catalog_engine::BrowseWarehouseConfig, CatalogEngineError> {
        unimplemented!()
    }
    async fn set_active(&self, _wh: &str, _a: bool) -> Result<(), CatalogEngineError> {
        unimplemented!()
    }
    async fn set_delete_profile(
        &self,
        _wh: &str,
        _s: Option<i64>,
    ) -> Result<(), CatalogEngineError> {
        unimplemented!()
    }
    async fn set_format_versions(&self, _wh: &str, _v: Vec<u8>) -> Result<(), CatalogEngineError> {
        unimplemented!()
    }
    async fn rotate_credential(
        &self,
        _wh: &str,
        _a: &str,
        _s: &str,
    ) -> Result<(), CatalogEngineError> {
        unimplemented!()
    }
    async fn search(
        &self,
        _wh: &str,
        _q: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseSearchResult>, CatalogEngineError> {
        unimplemented!()
    }
    async fn tasks(
        &self,
        _wh: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseTask>, CatalogEngineError> {
        unimplemented!()
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    sessions: SessionService,
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    Some(Fixture {
        db,
        users,
        sessions,
    })
}

fn app_with(
    f: &Fixture,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    lister: Arc<dyn CatalogLister>,
    browser: Option<Arc<dyn WarehouseBrowser>>,
    browse_timeout: std::time::Duration,
) -> axum::Router {
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto)),
    );
    control_plane_router().with_state(CpState {
        storage: egret_catalog_engine::CatalogStore::new(f.db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(f.db.clone()),
        users: f.users.clone(),
        groups: egret_auth::groups::GroupRepo::new(f.db.clone()),
        roles: egret_auth::roles::RoleRepo::new(f.db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(f.db.clone()),
        secrets,
        sessions: f.sessions.clone(),
        browser,
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(f.db.clone()),
        tags: egret_bi::TagRepo::new(f.db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            f.db.clone(),
            egret_events::EventBus::new(),
        ),
        db: f.db.clone(),
        mv_lister: Arc::new(NoMvLister),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: lister,
        browse_timeout,
    })
}

fn app(f: &Fixture, lister: Arc<FakeLister>) -> axum::Router {
    app_with(
        f,
        Arc::new(AllowOnly(&["policy:read"])),
        lister,
        None,
        std::time::Duration::from_secs(15),
    )
}

async fn session(f: &Fixture) -> String {
    let email = format!("gb_{}@egret.local", Uuid::new_v4().simple());
    let uid = f.users.insert_local(&email, "T", "x").await.unwrap();
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .unwrap()
        .token
}

async fn get(app: axum::Router, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri(uri)
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// `GET .../governance/browse/{catalog}` with `query` appended; asserts 200.
async fn browse(app: axum::Router, f: &Fixture, catalog: Uuid, query: &str) -> serde_json::Value {
    let token = session(f).await;
    let (status, j) = get(app, &format!("/governance/browse/{catalog}{query}"), &token).await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    j
}

fn suffix() -> String {
    Uuid::new_v4().simple().to_string()
}

/// How an engine is stored.
#[derive(Clone, Copy)]
enum Eng {
    /// A provisioned Trino in JWT delegation mode, in `status`.
    TrinoJwt(&'static str),
    /// A provisioned Trino whose spec names `impersonate`.
    TrinoImpersonate(&'static str),
    /// A provisioned Trino with no `delegationMode`: impersonate by default
    /// (`EngineRow::delegation_mode`).
    TrinoNoMode(&'static str),
    StarRocks(&'static str),
    RisingWave(&'static str),
    /// An external Trino, stored as SP4d-4 registers one: `status = 'running'`
    /// is a placeholder (migration 0184).
    ExternalTrino,
}

/// An engine named `<prefix>-<random>`; returns its name.
async fn engine(f: &Fixture, prefix: &str, e: Eng) -> (Uuid, String) {
    let name = format!("{prefix}-{}", suffix());
    let (ty, spec, status) = match e {
        Eng::TrinoJwt(s) => ("trino", r#"{"trinoConfig":{"delegationMode":"jwt"}}"#, s),
        Eng::TrinoImpersonate(s) => (
            "trino",
            r#"{"trinoConfig":{"delegationMode":"impersonate"}}"#,
            s,
        ),
        Eng::TrinoNoMode(s) => ("trino", "{}", s),
        Eng::StarRocks(s) => ("starrocks", "{}", s),
        Eng::RisingWave(s) => ("risingwave", "{}", s),
        Eng::ExternalTrino => {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, origin, \
                 external_host, external_port, ranger_plugin_version) \
                 VALUES ($1,'trino','','{}'::jsonb,'running','external','trino.example.com',443, \
                         '2.5.0') RETURNING id",
            )
            .bind(&name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
            return (id, name);
        }
    };
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status) \
         VALUES ($1,$2,'egret',$3::jsonb,$4) RETURNING id",
    )
    .bind(&name)
    .bind(ty)
    .bind(spec)
    .bind(status)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    (id, name)
}

/// An external (non-egret) catalog; returns `(id, name)`.
async fn external_catalog(f: &Fixture) -> (Uuid, String) {
    let name = format!("gb_ext_{}", suffix());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1,'tpch','{}'::jsonb) \
         RETURNING id",
    )
    .bind(&name)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    (id, name)
}

/// An egret Iceberg catalog over a warehouse whose lakekeeper id is
/// `lk-<random>`; returns `(catalog id, lakekeeper warehouse id)`.
async fn egret_catalog(f: &Fixture) -> (Uuid, String) {
    let pool = &f.db.pool;
    let s = suffix();
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'W') RETURNING id")
            .bind(format!("gb-ws-{s}"))
            .fetch_one(pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(format!("gb-sc-{s}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let lk = format!("lk-{s}");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(format!("gb_wh_{s}"))
    .bind(ws)
    .bind(sc)
    .bind(&lk)
    .fetch_one(pool)
    .await
    .unwrap();
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1,'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type','egret','warehouse_id',$2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(format!("gb_cat_{s}"))
    .bind(wh)
    .fetch_one(pool)
    .await
    .unwrap();
    (cat, lk)
}

async fn attach(f: &Fixture, engine: Uuid, catalog: Uuid) {
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog)
    .execute(&f.db.pool)
    .await
    .unwrap();
}

/// `(name, kind)` of every item.
fn items(j: &serde_json::Value) -> Vec<(String, String)> {
    j["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items: {j}"))
        .iter()
        .map(|i| {
            (
                i["name"].as_str().unwrap().to_string(),
                i["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// `(name, type)` of every column.
fn columns(j: &serde_json::Value) -> Vec<(String, String)> {
    j["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("columns: {j}"))
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_string(),
                c["type"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn s(v: &str) -> String {
    v.to_string()
}

/// Asserts the answer is `notListed` with `reason`, and nothing listed.
fn assert_not_listed(j: &serde_json::Value, reason: &str) -> String {
    assert_eq!(j["notListed"]["reason"], reason, "body={j}");
    assert!(j["items"].is_null(), "a failed listing is not empty: {j}");
    assert!(j["columns"].is_null(), "a failed listing is not empty: {j}");
    j["notListed"]["detail"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn an_egret_catalog_is_read_from_the_warehouse_and_no_engine_is_asked() {
    let Some(f) = setup().await else { return };
    let (cat, lk) = egret_catalog(&f).await;
    // Attached to a running JWT Trino that COULD list it: still not asked.
    let (trino, _) = engine(&f, "gb-t", Eng::TrinoJwt("running")).await;
    attach(&f, trino, cat).await;
    let lister = FakeLister::new(Answer::Levels);
    let wh = Arc::new(FakeWarehouse::default());
    let router = || {
        app_with(
            &f,
            Arc::new(AllowOnly(&["tag:read"])),
            lister.clone(),
            Some(wh.clone()),
            std::time::Duration::from_secs(15),
        )
    };

    let j = browse(router(), &f, cat, "").await;
    assert_eq!(j["source"], serde_json::json!({"kind": "egret"}));
    assert!(j["notListed"].is_null(), "body={j}");
    assert_eq!(
        items(&j),
        vec![(s("a.b"), s("schema")), (s("sales"), s("schema"))],
        "every namespace, dotted, sorted"
    );

    let j = browse(router(), &f, cat, "?schema=sales").await;
    assert_eq!(
        items(&j),
        vec![(s("orders"), s("table")), (s("v_orders"), s("view"))]
    );

    let j = browse(router(), &f, cat, "?schema=sales&table=orders").await;
    assert!(j["items"].is_null(), "body={j}");
    assert_eq!(
        columns(&j),
        vec![(s("id"), s("long")), (s("amount"), s("decimal(10,2)"))]
    );
    let j = browse(router(), &f, cat, "?schema=sales&table=v_orders").await;
    assert_eq!(columns(&j), vec![(s("total"), s("double"))]);

    assert!(
        lister.calls().is_empty(),
        "no engine is asked for an egret catalog"
    );
    assert!(
        wh.asked.lock().unwrap().iter().all(|w| *w == lk),
        "the catalog's own warehouse is read"
    );
}

#[tokio::test]
async fn a_missing_egret_table_is_a_404_not_an_empty_listing() {
    let Some(f) = setup().await else { return };
    let (cat, _) = egret_catalog(&f).await;
    let router = app_with(
        &f,
        Arc::new(AllowOnly(&["policy:read"])),
        FakeLister::new(Answer::Levels),
        Some(Arc::new(FakeWarehouse::default())),
        std::time::Duration::from_secs(15),
    );
    let token = session(&f).await;
    let (status, j) = get(
        router,
        &format!("/governance/browse/{cat}?schema=sales&table=nosuch"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={j}");
}

#[tokio::test]
async fn an_external_catalog_is_listed_through_its_running_trino_by_its_engine_name() {
    let Some(f) = setup().await else { return };
    let (cat, cat_name) = external_catalog(&f).await;
    let (trino, trino_name) = engine(&f, "gb-t", Eng::TrinoJwt("running")).await;
    // A running StarRocks, named to sort FIRST: Trino is still preferred.
    let (sr, _) = engine(&f, "gb-a-sr", Eng::StarRocks("running")).await;
    attach(&f, trino, cat).await;
    attach(&f, sr, cat).await;
    let lister = FakeLister::new(Answer::Levels);

    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    assert_eq!(
        j["source"],
        serde_json::json!({"kind": "engine", "name": trino_name})
    );
    assert_eq!(items(&j), vec![(s("sales"), s("schema"))]);
    assert!(j["notListed"].is_null(), "body={j}");

    let j = browse(app(&f, lister.clone()), &f, cat, "?schema=sales").await;
    assert_eq!(
        items(&j),
        vec![(s("orders"), s("table")), (s("v_orders"), s("view"))]
    );
    let j = browse(
        app(&f, lister.clone()),
        &f,
        cat,
        "?schema=sales&table=orders",
    )
    .await;
    assert!(j["items"].is_null(), "body={j}");
    assert_eq!(columns(&j), vec![(s("id"), s("integer"))]);

    let call = |schema: Option<&str>, table: Option<&str>| Call {
        engine: trino_name.clone(),
        engine_type: ListingEngineType::Trino,
        catalog: cat_name.clone(),
        schema: schema.map(String::from),
        table: table.map(String::from),
    };
    assert_eq!(
        lister.calls(),
        vec![
            call(None, None),
            call(Some("sales"), None),
            call(Some("sales"), Some("orders")),
        ],
        "the registry name is the catalog's name on every engine (D5)"
    );
}

#[tokio::test]
async fn starrocks_is_used_when_no_trino_runs_and_ties_go_by_name() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (zz, _) = engine(&f, "gb-zz", Eng::StarRocks("running")).await;
    let (aa, aa_name) = engine(&f, "gb-aa", Eng::StarRocks("running")).await;
    // A Trino that is not running is not a candidate.
    let (t, _) = engine(&f, "gb-t", Eng::TrinoJwt("failed")).await;
    for e in [zz, aa, t] {
        attach(&f, e, cat).await;
    }
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    assert_eq!(
        j["source"],
        serde_json::json!({"kind": "engine", "name": aa_name})
    );
    let calls = lister.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].engine, aa_name);
    assert_eq!(calls[0].engine_type, ListingEngineType::StarRocks);
}

#[tokio::test]
async fn an_impersonate_mode_trino_is_never_asked() {
    let Some(f) = setup().await else { return };

    // Only impersonate-mode Trinos (explicit, and by default): engine_error,
    // and nothing is dialled.
    let (cat, _) = external_catalog(&f).await;
    let (imp, imp_name) = engine(&f, "gb-imp", Eng::TrinoImpersonate("running")).await;
    let (dflt, _) = engine(&f, "gb-dflt", Eng::TrinoNoMode("running")).await;
    attach(&f, imp, cat).await;
    attach(&f, dflt, cat).await;
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    let detail = assert_not_listed(&j, "engine_error");
    assert!(
        detail.contains(&imp_name) && detail.contains("impersonate mode"),
        "detail names the engine and why: {detail}"
    );
    assert!(j["source"].is_null(), "nothing was asked: {j}");
    assert!(
        lister.calls().is_empty(),
        "an impersonate Trino is never dialled"
    );

    // With a running StarRocks beside it, the StarRocks lists.
    let (cat2, _) = external_catalog(&f).await;
    let (sr, sr_name) = engine(&f, "gb-zz-sr", Eng::StarRocks("running")).await;
    attach(&f, imp, cat2).await;
    attach(&f, sr, cat2).await;
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat2, "").await;
    assert_eq!(
        j["source"],
        serde_json::json!({"kind": "engine", "name": sr_name})
    );
    assert_eq!(lister.calls().len(), 1);
    assert_eq!(lister.calls()[0].engine, sr_name);
}

#[tokio::test]
async fn an_engine_egret_cannot_list_through_is_never_asked() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (rw, rw_name) = engine(&f, "gb-rw", Eng::RisingWave("running")).await;
    attach(&f, rw, cat).await;
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    let detail = assert_not_listed(&j, "engine_error");
    assert!(detail.contains(&rw_name), "detail: {detail}");
    assert!(lister.calls().is_empty());
}

#[tokio::test]
async fn a_catalog_attached_only_to_external_engines_is_never_dialled() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (ext, ext_name) = engine(&f, "gb-ext", Eng::ExternalTrino).await;
    attach(&f, ext, cat).await;
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    let detail = assert_not_listed(&j, "only_external_engines");
    assert!(detail.contains(&ext_name), "detail: {detail}");
    assert!(
        lister.calls().is_empty(),
        "an external engine is never dialled"
    );
}

#[tokio::test]
async fn a_catalog_attached_to_nothing_says_so() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    assert_not_listed(&j, "no_engine_attached");
    assert_eq!(j["engines"], serde_json::json!([]));
    assert!(lister.calls().is_empty());
}

#[tokio::test]
async fn a_provisioned_engine_that_is_not_running_is_not_asked() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (t, t_name) = engine(&f, "gb-t", Eng::TrinoJwt("provisioning")).await;
    let (ext, _) = engine(&f, "gb-ext", Eng::ExternalTrino).await;
    attach(&f, t, cat).await;
    attach(&f, ext, cat).await;
    let lister = FakeLister::new(Answer::Levels);
    let j = browse(app(&f, lister.clone()), &f, cat, "").await;
    let detail = assert_not_listed(&j, "engine_not_running");
    assert!(
        detail.contains(&t_name) && detail.contains("provisioning"),
        "detail: {detail}"
    );
    assert!(lister.calls().is_empty());
}

#[tokio::test]
async fn an_engine_error_is_reported_without_the_engines_text() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (t, t_name) = engine(&f, "gb-t", Eng::TrinoJwt("running")).await;
    attach(&f, t, cat).await;
    let lister = FakeLister::new(Answer::Fail(
        "Access Denied: Cannot access catalog SECRET_RAW_ENGINE_TEXT",
    ));
    let j = browse(app(&f, lister.clone()), &f, cat, "?schema=sales").await;
    let detail = assert_not_listed(&j, "engine_error");
    assert!(
        !j.to_string().contains("SECRET_RAW_ENGINE_TEXT"),
        "the engine's own text stays in the log: {j}"
    );
    assert!(
        detail.contains(&t_name),
        "detail names the engine: {detail}"
    );
    assert_eq!(
        j["source"],
        serde_json::json!({"kind": "engine", "name": t_name}),
        "the engine that was asked"
    );
    assert_eq!(lister.calls().len(), 1);
}

#[tokio::test]
async fn an_engine_that_does_not_answer_in_time_is_a_timeout() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (t, _) = engine(&f, "gb-t", Eng::TrinoJwt("running")).await;
    attach(&f, t, cat).await;
    let lister = FakeLister::new(Answer::Hang);
    let router = app_with(
        &f,
        Arc::new(AllowOnly(&["policy:read"])),
        lister.clone(),
        None,
        std::time::Duration::from_millis(50),
    );
    let started = std::time::Instant::now();
    let j = browse(router, &f, cat, "").await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the deadline bounds the request"
    );
    assert_not_listed(&j, "timeout");
    assert_eq!(lister.calls().len(), 1);
}

#[tokio::test]
async fn engines_lists_every_attached_engine_with_origin_and_status() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let (t, t_name) = engine(&f, "gb-1t", Eng::TrinoJwt("running")).await;
    let (sr, sr_name) = engine(&f, "gb-2sr", Eng::StarRocks("failed")).await;
    let (ext, ext_name) = engine(&f, "gb-3ext", Eng::ExternalTrino).await;
    for e in [t, sr, ext] {
        attach(&f, e, cat).await;
    }
    let j = browse(app(&f, FakeLister::new(Answer::Levels)), &f, cat, "").await;
    assert_eq!(
        j["engines"],
        serde_json::json!([
            {"name": t_name, "type": "trino", "origin": "provisioned", "status": "running"},
            {"name": sr_name, "type": "starrocks", "origin": "provisioned", "status": "failed"},
            {"name": ext_name, "type": "trino", "origin": "external", "status": "running"},
        ])
    );
}

#[tokio::test]
async fn browse_needs_policy_read_or_tag_read() {
    let Some(f) = setup().await else { return };
    let (cat, _) = external_catalog(&f).await;
    let uri = format!("/governance/browse/{cat}");
    for (allowed, want) in [
        (&[][..], StatusCode::FORBIDDEN),
        (&["catalog_registry:read"][..], StatusCode::FORBIDDEN),
        (&["policy:read"][..], StatusCode::OK),
        (&["tag:read"][..], StatusCode::OK),
    ] {
        let lister = FakeLister::new(Answer::Levels);
        let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(AllowOnly(allowed));
        let router = app_with(
            &f,
            pdp,
            lister.clone(),
            None,
            std::time::Duration::from_secs(15),
        );
        let (status, j) = get(router, &uri, &session(&f).await).await;
        assert_eq!(status, want, "allowed={allowed:?} body={j}");
    }
}

#[tokio::test]
async fn an_unknown_catalog_is_404_and_a_table_needs_its_schema() {
    let Some(f) = setup().await else { return };
    let token = session(&f).await;
    let (status, _) = get(
        app(&f, FakeLister::new(Answer::Levels)),
        &format!("/governance/browse/{}", Uuid::new_v4()),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (cat, _) = external_catalog(&f).await;
    let lister = FakeLister::new(Answer::Levels);
    let (status, j) = get(
        app(&f, lister.clone()),
        &format!("/governance/browse/{cat}?table=orders"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(lister.calls().is_empty());
}
