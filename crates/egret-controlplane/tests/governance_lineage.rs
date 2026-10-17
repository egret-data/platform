//! `GET /api/control-plane/governance/lineage/{catalog_id}?schema=&table=` (DG
//! frontend redesign #1, spec §2.3 "Lineage"): for a table, the views whose
//! recorded lineage reads it and the StarRocks materialized views that read
//! it; for a view, its recorded base tables, security mode, owner, whether the
//! owner is cleared, and the lineage state. DB-backed through the per-process
//! test DB; the StarRocks FE is a fake `MaterializedViewLister`, egret's
//! warehouse a fake `WarehouseBrowser` and its views' metadata a fake
//! `ViewCatalog`, so no live engine or lakekeeper is needed.

use std::collections::{BTreeMap, BTreeSet};
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
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow,
    UnwiredCatalogLister,
};
use egret_metastore::Db;
use egret_perm_compile::lineage::ViewRef;
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

/// What the fake StarRocks FE answers.
#[derive(Clone)]
enum MvAnswer {
    Rows(Vec<MvRow>),
    Fail(&'static str),
    /// Accepts the connection and goes silent.
    Hang,
}

/// A StarRocks FE per engine name; records every engine it is asked about.
#[derive(Default)]
struct FakeMvs {
    answers: BTreeMap<String, MvAnswer>,
    asked: Mutex<Vec<String>>,
}

impl FakeMvs {
    fn answer(mut self, engine: &str, a: MvAnswer) -> Self {
        self.answers.insert(engine.to_string(), a);
        self
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl MaterializedViewLister for FakeMvs {
    async fn list(&self, engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        self.asked.lock().unwrap().push(engine.name.clone());
        match self.answers.get(&engine.name) {
            Some(MvAnswer::Rows(rows)) => Ok(rows.clone()),
            Some(MvAnswer::Fail(e)) => Err(e.to_string()),
            Some(MvAnswer::Hang) => {
                std::future::pending::<()>().await;
                unreachable!()
            }
            None => panic!("engine {} was not expected to be asked", engine.name),
        }
    }
}

/// The views' stored `trino.run-as-owner`, by view name.
#[derive(Default)]
struct FakeViews {
    owners: BTreeMap<String, Result<Option<String>, String>>,
    asked: Mutex<Vec<egret_catalog_engine::CatalogView>>,
}

impl FakeViews {
    fn view(mut self, name: &str, owner: Result<Option<&str>, &str>) -> Self {
        self.owners.insert(
            name.to_string(),
            owner.map(|o| o.map(String::from)).map_err(String::from),
        );
        self
    }
}

#[async_trait::async_trait]
impl egret_catalog_engine::ViewCatalog for FakeViews {
    async fn list_views(&self) -> Result<egret_catalog_engine::CatalogViews, String> {
        panic!("the lineage route never lists every view")
    }
    async fn run_as_owner(
        &self,
        view: &egret_catalog_engine::CatalogView,
    ) -> Result<Option<String>, String> {
        self.asked.lock().unwrap().push(view.clone());
        self.owners
            .get(&view.name)
            .cloned()
            .unwrap_or_else(|| Err("no such view".into()))
    }
}

/// A warehouse whose `sales` namespace holds the table `orders` and the
/// views `v_orders`, `v_failed` and `v_new`.
struct FakeWarehouse;

#[async_trait::async_trait]
impl WarehouseBrowser for FakeWarehouse {
    async fn namespaces(&self, _wh: &str) -> Result<Vec<BrowseNamespace>, CatalogEngineError> {
        unimplemented!()
    }
    async fn tabulars(
        &self,
        _wh: &str,
        ns: &[String],
    ) -> Result<Vec<BrowseTabular>, CatalogEngineError> {
        if ns != ["sales".to_string()] {
            return Err(CatalogEngineError::NotFound(format!("namespace {ns:?}")));
        }
        let t = |name: &str, kind: &str| BrowseTabular {
            name: name.into(),
            kind: kind.into(),
            id: None,
            protected: false,
        };
        Ok(vec![
            t("orders", "table"),
            t("v_orders", "view"),
            t("v_failed", "view"),
            t("v_new", "view"),
        ])
    }
    async fn schema(
        &self,
        _wh: &str,
        _ns: &[String],
        _t: &str,
    ) -> Result<BrowseTableDetail, CatalogEngineError> {
        unimplemented!()
    }
    async fn view_columns(
        &self,
        _wh: &str,
        _ns: &[String],
        _v: &str,
    ) -> Result<Vec<WhColumn>, CatalogEngineError> {
        unimplemented!()
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

async fn setup() -> Fixture {
    let dsn = egret_test_db::url()
        .await
        .expect("no test database: set DATABASE_URL to the egret_test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    Fixture {
        db,
        users,
        sessions,
    }
}

struct Wiring {
    pdp: Arc<dyn egret_core::authz::Pdp>,
    mvs: Arc<FakeMvs>,
    views: Arc<FakeViews>,
    timeout: std::time::Duration,
}

impl Wiring {
    fn new() -> Self {
        Wiring {
            pdp: Arc::new(AllowOnly(&["policy:read"])),
            mvs: Arc::new(FakeMvs::default()),
            views: Arc::new(FakeViews::default()),
            timeout: std::time::Duration::from_secs(15),
        }
    }
}

fn app(f: &Fixture, w: &Wiring) -> axum::Router {
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
        browser: Some(Arc::new(FakeWarehouse)),
        pdp: w.pdp.clone(),
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(f.db.clone()),
        tags: egret_bi::TagRepo::new(f.db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            f.db.clone(),
            egret_events::EventBus::new(),
        ),
        db: f.db.clone(),
        mv_lister: w.mvs.clone(),
        // Not this route's deadline: a long one, so a test that hangs the
        // lister proves the route uses `browse_timeout`.
        mv_list_timeout: std::time::Duration::from_secs(600),
        lineage_sweeper: Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: w.views.clone(),
        view_audit_timeout: std::time::Duration::from_secs(600),
        catalog_lister: Arc::new(UnwiredCatalogLister),
        browse_timeout: w.timeout,
    })
}

async fn session(f: &Fixture) -> String {
    let email = format!("gl_{}@egret.local", Uuid::new_v4().simple());
    let uid = f.users.insert_local(&email, "T", "x").await.unwrap();
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .unwrap()
        .token
}

async fn get(f: &Fixture, w: &Wiring, uri: &str) -> (StatusCode, serde_json::Value) {
    let token = session(f).await;
    let resp = app(f, w)
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

/// The lineage of `schema.table` in `catalog`; asserts 200.
async fn lineage(
    f: &Fixture,
    w: &Wiring,
    catalog: Uuid,
    schema: &str,
    table: &str,
) -> serde_json::Value {
    let (status, j) = get(
        f,
        w,
        &format!("/governance/lineage/{catalog}?schema={schema}&table={table}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    j
}

fn suffix() -> String {
    Uuid::new_v4().simple().to_string()
}

/// An external (non-egret) catalog; returns `(id, name)`.
async fn external_catalog(f: &Fixture) -> (Uuid, String) {
    let name = format!("gl_ext_{}", suffix());
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
/// `lk-<random>`; returns `(catalog id, catalog name, lakekeeper id)`.
async fn egret_catalog(f: &Fixture) -> (Uuid, String, String) {
    let pool = &f.db.pool;
    let s = suffix();
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'W') RETURNING id")
            .bind(format!("gl-ws-{s}"))
            .fetch_one(pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(format!("gl-sc-{s}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let lk = format!("lk-{s}");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(format!("gl_wh_{s}"))
    .bind(ws)
    .bind(sc)
    .bind(&lk)
    .fetch_one(pool)
    .await
    .unwrap();
    let name = format!("gl_cat_{s}");
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1,'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type','egret','warehouse_id',$2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&name)
    .bind(wh)
    .fetch_one(pool)
    .await
    .unwrap();
    (cat, name, lk)
}

/// A provisioned StarRocks engine in `status`, or an external one; returns
/// `(id, name)`.
async fn starrocks(f: &Fixture, status: &str, external: bool) -> (Uuid, String) {
    let name = format!("gl-sr-{}", suffix());
    let id: Uuid = if external {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, origin, \
             external_host, external_port, ranger_plugin_version) \
             VALUES ($1,'starrocks','','{}'::jsonb,'running','external','sr.example.com',9030, \
                     '2.8.0') RETURNING id",
        )
        .bind(&name)
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
    } else {
        sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status) \
             VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
        )
        .bind(&name)
        .bind(status)
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
    };
    (id, name)
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

fn view_ref(catalog: &str, schema: &str, view: &str) -> ViewRef {
    ViewRef {
        catalog: catalog.into(),
        schema: schema.into(),
        name: view.into(),
    }
}

/// Record `tables` as `catalog.sales.view`'s computed lineage.
async fn computed(f: &Fixture, catalog: &str, view: &str, tables: &[(&str, &str, &str)]) {
    let mut conn = f.db.pool.acquire().await.unwrap();
    let set: BTreeSet<_> = tables
        .iter()
        .map(|(c, s, t)| (c.to_string(), s.to_string(), t.to_string()))
        .collect();
    egret_perm_compile::lineage::write_lineage(&mut conn, &view_ref(catalog, "sales", view), &set)
        .await
        .unwrap();
}

/// `(catalog, schema, name, kind, engine)` of every reader.
fn readers(j: &serde_json::Value) -> Vec<(String, String, String, String, Option<String>)> {
    j["readers"]
        .as_array()
        .unwrap_or_else(|| panic!("readers: {j}"))
        .iter()
        .map(|r| {
            (
                r["catalog"].as_str().unwrap().to_string(),
                r["schema"].as_str().unwrap().to_string(),
                r["name"].as_str().unwrap().to_string(),
                r["kind"].as_str().unwrap().to_string(),
                r["engine"].as_str().map(String::from),
            )
        })
        .collect()
}

fn s(v: &str) -> String {
    v.to_string()
}

fn mv(database: &str, name: &str, bases: Option<&[&str]>, definition: &str) -> MvRow {
    MvRow {
        database: database.into(),
        name: name.into(),
        base_tables: bases.map(|b| b.iter().map(|x| x.to_string()).collect()),
        definition: definition.into(),
    }
}

#[tokio::test]
async fn a_table_read_by_two_views_lists_both_and_no_other() {
    let f = setup().await;
    let (cat, name) = external_catalog(&f).await;
    computed(&f, &name, "v_a", &[(&name, "sales", "orders")]).await;
    computed(
        &f,
        &name,
        "v_b",
        &[(&name, "sales", "orders"), (&name, "sales", "customers")],
    )
    .await;
    // Reads another table of the same schema: not a reader.
    computed(&f, &name, "v_other", &[(&name, "sales", "customers")]).await;
    let w = Wiring::new();

    let j = lineage(&f, &w, cat, "sales", "orders").await;

    assert_eq!(
        readers(&j),
        vec![
            (s(&name), s("sales"), s("v_a"), s("view"), None),
            (s(&name), s("sales"), s("v_b"), s("view"), None),
        ],
        "body={j}"
    );
    assert!(j["readersIncomplete"].is_null(), "body={j}");
    assert!(j["view"].is_null(), "a table has no view half: {j}");
    assert!(w.mvs.asked().is_empty(), "no StarRocks is attached");
}

#[tokio::test]
async fn an_egret_table_lists_its_view_readers_and_the_starrocks_mvs_that_read_it() {
    let f = setup().await;
    let (cat, name, _) = egret_catalog(&f).await;
    computed(&f, &name, "v_orders", &[(&name, "sales", "orders")]).await;
    let (sr, sr_name) = starrocks(&f, "running", false).await;
    attach(&f, sr, cat).await;
    let base = format!("{name}.sales.orders");
    let mut w = Wiring::new();
    w.mvs = Arc::new(FakeMvs::default().answer(
        &sr_name,
        MvAnswer::Rows(vec![
            mv("mart", "mv_orders", Some(&[base.as_str()]), ""),
            // Over the first materialized view: it holds the same values.
            mv(
                "mart",
                "mv_rollup",
                Some(&["default_catalog.mart.mv_orders"]),
                "",
            ),
            mv("mart", "mv_other", Some(&["x.y.z"]), ""),
        ]),
    ));

    let j = lineage(&f, &w, cat, "sales", "orders").await;

    assert_eq!(
        readers(&j),
        vec![
            (s(&name), s("sales"), s("v_orders"), s("view"), None),
            (
                s("default_catalog"),
                s("mart"),
                s("mv_orders"),
                s("materialized_view"),
                Some(sr_name.clone())
            ),
            (
                s("default_catalog"),
                s("mart"),
                s("mv_rollup"),
                s("materialized_view"),
                Some(sr_name.clone())
            ),
        ],
        "body={j}"
    );
    assert!(j["readersIncomplete"].is_null(), "body={j}");
    assert!(j["view"].is_null(), "body={j}");
    assert_eq!(w.mvs.asked(), vec![sr_name]);
}

#[tokio::test]
async fn a_failed_mv_listing_marks_the_readers_incomplete_without_its_text() {
    let f = setup().await;
    let (cat, name) = external_catalog(&f).await;
    computed(&f, &name, "v_a", &[(&name, "sales", "orders")]).await;
    let (sr, sr_name) = starrocks(&f, "running", false).await;
    attach(&f, sr, cat).await;
    let mut w = Wiring::new();
    w.mvs = Arc::new(FakeMvs::default().answer(
        &sr_name,
        MvAnswer::Fail("Access denied for user 'root'@'10.1.2.3'"),
    ));

    let j = lineage(&f, &w, cat, "sales", "orders").await;

    let detail = j["readersIncomplete"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("readersIncomplete is set: {j}"));
    assert!(detail.contains(&sr_name), "names the engine: {detail}");
    assert!(
        !detail.contains("Access denied") && !detail.contains("10.1.2.3"),
        "the engine's own text stays in the log: {detail}"
    );
    assert_eq!(
        readers(&j),
        vec![(s(&name), s("sales"), s("v_a"), s("view"), None)],
        "what was read is still shown, flagged incomplete"
    );
}

#[tokio::test]
async fn an_mv_listing_past_the_browse_deadline_marks_the_readers_incomplete() {
    let f = setup().await;
    let (cat, _) = external_catalog(&f).await;
    let (sr, sr_name) = starrocks(&f, "running", false).await;
    attach(&f, sr, cat).await;
    let mut w = Wiring::new();
    w.mvs = Arc::new(FakeMvs::default().answer(&sr_name, MvAnswer::Hang));
    w.timeout = std::time::Duration::from_millis(300);

    let started = std::time::Instant::now();
    let j = lineage(&f, &w, cat, "sales", "orders").await;

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the browse deadline, not the 600 s MV deadline, bounds the request"
    );
    let detail = j["readersIncomplete"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("{j}"));
    assert!(detail.contains(&sr_name), "{detail}");
    assert_eq!(readers(&j), vec![], "body={j}");
}

#[tokio::test]
async fn an_external_or_stopped_starrocks_is_never_dialled_and_its_mvs_are_unknown() {
    let f = setup().await;
    let (cat, _) = external_catalog(&f).await;
    let (ext, ext_name) = starrocks(&f, "running", true).await;
    let (stopped, stopped_name) = starrocks(&f, "failed", false).await;
    attach(&f, ext, cat).await;
    attach(&f, stopped, cat).await;
    let w = Wiring::new();

    let j = lineage(&f, &w, cat, "sales", "orders").await;

    assert!(w.mvs.asked().is_empty(), "neither engine is dialled");
    let detail = j["readersIncomplete"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("{j}"));
    assert!(
        detail.contains(&ext_name) && detail.contains(&stopped_name),
        "{detail}"
    );
}

#[tokio::test]
async fn a_view_with_computed_lineage_shows_its_bases_mode_owner_and_state() {
    let f = setup().await;
    let (cat, name, lk) = egret_catalog(&f).await;
    computed(
        &f,
        &name,
        "v_orders",
        &[(&name, "sales", "orders"), (&name, "sales", "customers")],
    )
    .await;
    let mut w = Wiring::new();
    w.views = Arc::new(FakeViews::default().view("v_orders", Ok(Some("alice"))));

    let j = lineage(&f, &w, cat, "sales", "v_orders").await;

    let v = &j["view"];
    assert_eq!(v["securityMode"], "definer", "body={j}");
    assert_eq!(v["owner"], "alice");
    // No base carries a classification, so nothing gates the owner.
    assert_eq!(v["ownerCleared"], true);
    assert_eq!(v["lineageState"], "computed");
    assert_eq!(
        v["bases"],
        serde_json::json!([
            {"catalog": name, "schema": "sales", "table": "customers"},
            {"catalog": name, "schema": "sales", "table": "orders"},
        ])
    );
    let asked = w.views.asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        vec![egret_catalog_engine::CatalogView {
            catalog: name.clone(),
            schema: "sales".into(),
            name: "v_orders".into(),
            prefix: lk,
        }],
        "the view's metadata is read from its own warehouse"
    );
}

#[tokio::test]
async fn a_view_whose_lineage_failed_says_so_and_why() {
    let f = setup().await;
    let (cat, name, _) = egret_catalog(&f).await;
    let mut conn = f.db.pool.acquire().await.unwrap();
    egret_perm_compile::lineage::record_lineage_failure(
        &mut conn,
        &view_ref(&name, "sales", "v_failed"),
        "EXPLAIN failed: table not found",
    )
    .await
    .unwrap();
    drop(conn);
    let mut w = Wiring::new();
    w.views = Arc::new(FakeViews::default().view("v_failed", Ok(Some("alice"))));

    let j = lineage(&f, &w, cat, "sales", "v_failed").await;

    let v = &j["view"];
    assert_eq!(v["lineageState"], "failed", "body={j}");
    assert_eq!(v["lineageDetail"], "EXPLAIN failed: table not found");
    assert_eq!(v["bases"], serde_json::json!([]));
    assert_eq!(v["securityMode"], "definer");
    assert!(
        v["ownerCleared"].is_null(),
        "with no lineage, clearance cannot be said: {j}"
    );
}

#[tokio::test]
async fn a_view_with_no_recorded_lineage_is_missing_not_a_table() {
    let f = setup().await;
    let (cat, _, _) = egret_catalog(&f).await;
    let mut w = Wiring::new();
    w.views = Arc::new(FakeViews::default().view("v_new", Ok(None)));

    let j = lineage(&f, &w, cat, "sales", "v_new").await;

    let v = &j["view"];
    assert_eq!(v["lineageState"], "missing", "body={j}");
    assert_eq!(v["securityMode"], "invoker");
    assert!(v["owner"].is_null());
    assert!(v["ownerCleared"].is_null());
    assert_eq!(v["bases"], serde_json::json!([]));
}

#[tokio::test]
async fn a_view_whose_metadata_cannot_be_read_has_an_unknown_mode() {
    let f = setup().await;
    let (cat, name, _) = egret_catalog(&f).await;
    computed(&f, &name, "v_orders", &[(&name, "sales", "orders")]).await;
    let mut w = Wiring::new();
    w.views =
        Arc::new(FakeViews::default().view("v_orders", Err("lakekeeper said 500 at 10.0.0.9")));

    let j = lineage(&f, &w, cat, "sales", "v_orders").await;

    let v = &j["view"];
    assert_eq!(v["securityMode"], "unknown", "body={j}");
    let err = v["securityModeError"]
        .as_str()
        .unwrap_or_else(|| panic!("{j}"));
    assert!(
        !err.contains("10.0.0.9"),
        "the raw error stays in the log: {err}"
    );
    assert!(v["owner"].is_null());
    assert_eq!(v["lineageState"], "computed");
}

#[tokio::test]
async fn a_view_never_reports_its_readers_as_complete() {
    // Recorded lineage resolves nested views to their base tables, so no view
    // is ever recorded as READING a view: an empty list would be a lie.
    let f = setup().await;
    let (cat, name, _) = egret_catalog(&f).await;
    computed(&f, &name, "v_orders", &[(&name, "sales", "orders")]).await;
    let mut w = Wiring::new();
    w.views = Arc::new(FakeViews::default().view("v_orders", Ok(None)));

    let j = lineage(&f, &w, cat, "sales", "v_orders").await;

    assert!(j["readersIncomplete"]["detail"].is_string(), "body={j}");
    assert!(w.mvs.asked().is_empty());
}

#[tokio::test]
async fn an_egret_object_that_does_not_exist_is_a_404() {
    let f = setup().await;
    let (cat, _, _) = egret_catalog(&f).await;
    let w = Wiring::new();
    let (status, j) = get(
        &f,
        &w,
        &format!("/governance/lineage/{cat}?schema=sales&table=nope"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={j}");
    let (status, _) = get(
        &f,
        &w,
        &format!(
            "/governance/lineage/{}?schema=sales&table=orders",
            Uuid::new_v4()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an unknown catalog");
}

#[tokio::test]
async fn schema_and_table_are_required_and_not_empty() {
    let f = setup().await;
    let (cat, _) = external_catalog(&f).await;
    let w = Wiring::new();
    for q in [
        "?schema=sales",
        "?table=orders",
        "?schema=&table=orders",
        "?schema=sales&table=",
    ] {
        let (status, j) = get(&f, &w, &format!("/governance/lineage/{cat}{q}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{q}: body={j}");
    }
}

#[tokio::test]
async fn it_needs_policy_read_or_tag_read() {
    let f = setup().await;
    let (cat, _) = external_catalog(&f).await;
    let uri = format!("/governance/lineage/{cat}?schema=sales&table=orders");
    let mut w = Wiring::new();

    w.pdp = Arc::new(AllowOnly(&["catalog:read"]));
    let (status, j) = get(&f, &w, &uri).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body={j}");

    w.pdp = Arc::new(AllowOnly(&["tag:read"]));
    let (status, j) = get(&f, &w, &uri).await;
    assert_eq!(status, StatusCode::OK, "tag:read alone reads lineage: {j}");

    w.pdp = Arc::new(AllowOnly(&["policy:read"]));
    let (status, j) = get(&f, &w, &uri).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "policy:read alone reads lineage: {j}"
    );
}
