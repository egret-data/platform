//! `GET /api/control-plane/classifications/{id}/view-exposure` (view governance
//! A4, revised 2026-10-14): the materialized views that already exist and read a
//! table of the classification. StarRocks keeps every materialized view in
//! `default_catalog`, so the lister answers once per engine with every view and
//! its base tables, and the route matches them. DB-backed through the
//! per-process test DB; the StarRocks FE is a fake `MaterializedViewLister`, so
//! no live engine is needed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow,
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

type Listing = Result<Vec<MvRow>, String>;

/// Engine name -> the rows of its `information_schema.materialized_views`, or an
/// error message.
#[derive(Default)]
struct FakeLister {
    answers: Mutex<BTreeMap<String, Listing>>,
    asked: Mutex<Vec<String>>,
    /// Engines that never answer, like an FE that accepts the connection and
    /// goes silent.
    hangs: Mutex<Vec<String>>,
}

impl FakeLister {
    fn answer(&self, engine: &str, r: Result<Vec<MvRow>, &str>) {
        self.answers
            .lock()
            .unwrap()
            .insert(engine.to_string(), r.map_err(String::from));
    }
    fn asked(&self) -> Vec<String> {
        let mut a = self.asked.lock().unwrap().clone();
        a.sort();
        a
    }
}

#[async_trait::async_trait]
impl MaterializedViewLister for FakeLister {
    async fn list(&self, engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        self.asked.lock().unwrap().push(engine.name.clone());
        if self.hangs.lock().unwrap().contains(&engine.name) {
            std::future::pending::<()>().await;
        }
        self.answers
            .lock()
            .unwrap()
            .get(&engine.name)
            .cloned()
            .unwrap_or_else(|| Ok(vec![]))
    }
}

/// A view whose `BASE_TABLE_REFRESH_VERSION_TIMES` names `base`.
fn mv(database: &str, name: &str, base: &[&str]) -> MvRow {
    MvRow {
        database: database.into(),
        name: name.into(),
        base_tables: Some(base.iter().map(|b| b.to_string()).collect()),
        definition: format!("CREATE MATERIALIZED VIEW {name} AS SELECT 1"),
    }
}

/// A view with no base-table map (never refreshed): only its text can match.
fn mv_text(database: &str, name: &str, definition: &str) -> MvRow {
    MvRow {
        database: database.into(),
        name: name.into(),
        base_tables: None,
        definition: definition.into(),
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

fn app(
    f: &Fixture,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    lister: Arc<dyn MaterializedViewLister>,
) -> axum::Router {
    app_with_timeout(f, pdp, lister, std::time::Duration::from_secs(15))
}

fn app_with_timeout(
    f: &Fixture,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    lister: Arc<dyn MaterializedViewLister>,
    mv_list_timeout: std::time::Duration,
) -> axum::Router {
    app_full(
        f,
        pdp,
        lister,
        mv_list_timeout,
        Arc::new(egret_catalog_engine::NoCatalogSweeper),
    )
}

fn app_full(
    f: &Fixture,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    lister: Arc<dyn MaterializedViewLister>,
    mv_list_timeout: std::time::Duration,
    lineage_sweeper: Arc<dyn egret_catalog_engine::LineageSweeper>,
) -> axum::Router {
    app_views(
        f,
        pdp,
        lister,
        mv_list_timeout,
        lineage_sweeper,
        Arc::new(egret_catalog_engine::NoViewCatalog),
        std::time::Duration::from_secs(15),
    )
}

#[allow(clippy::too_many_arguments)]
fn app_views(
    f: &Fixture,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    lister: Arc<dyn MaterializedViewLister>,
    mv_list_timeout: std::time::Duration,
    lineage_sweeper: Arc<dyn egret_catalog_engine::LineageSweeper>,
    view_catalog: Arc<dyn egret_catalog_engine::ViewCatalog>,
    view_audit_timeout: std::time::Duration,
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
        browser: None,
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(f.db.clone()),
        tags: egret_bi::TagRepo::new(f.db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            f.db.clone(),
            egret_events::EventBus::new(),
        ),
        db: f.db.clone(),
        mv_lister: lister,
        mv_list_timeout,
        lineage_sweeper,
        view_catalog,
        view_audit_timeout,
        catalog_lister: std::sync::Arc::new(egret_controlplane::UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
        audit_retention_days: Some(90),
    })
}

async fn session(f: &Fixture) -> String {
    let email = format!("ve_{}@egret.local", Uuid::new_v4().simple());
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

fn suffix() -> String {
    Uuid::new_v4().simple().to_string()
}

/// A StarRocks policy profile.
async fn profile(f: &Fixture) -> i64 {
    let pool = &f.db.pool;
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) \
         VALUES (910000001, 'starrocks', '{}'::jsonb) ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'starrocks', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("ve-svc-{}", suffix()))
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A StarRocks engine bound to `service`, in `status`; returns `(id, name)`.
async fn engine(f: &Fixture, service: i64, status: &str) -> (Uuid, String) {
    let name = format!("ve-eng-{}", suffix());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id, status) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2,$3) RETURNING id",
    )
    .bind(&name)
    .bind(service)
    .bind(status)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    (id, name)
}

/// An EXTERNAL StarRocks engine bound to `service`, stored as SP4d-4 registers
/// one: `status = 'running'` (a placeholder; migration 0184 did not widen it)
/// and `origin = 'external'`. egret holds no credential for it.
async fn external_engine(f: &Fixture, service: i64) -> (Uuid, String) {
    let name = format!("ve-ext-{}", suffix());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, origin, \
         external_host, external_port, ranger_plugin_version, authz_service_id) \
         VALUES ($1,'starrocks','','{}'::jsonb,'running','external','sr.example.com',9030, \
                 '2.8.0',$2) RETURNING id",
    )
    .bind(&name)
    .bind(service)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    (id, name)
}

async fn new_tag(f: &Fixture) -> Uuid {
    let s = suffix();
    sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1,$2) RETURNING id")
        .bind(format!("ve-tag-{s}"))
        .bind(format!("VE {s}"))
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
}

/// A warehouse and its registry catalog attached to `engine_id`; returns
/// `(warehouse id, catalog name)`.
async fn attached_catalog(f: &Fixture, engine_id: Uuid) -> (Uuid, String) {
    let pool = &f.db.pool;
    let s = suffix();
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'W') RETURNING id")
            .bind(format!("ve-ws-{s}"))
            .fetch_one(pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(format!("ve-sc-{s}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("ve_wh_{s}"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .unwrap();
    let catalog = format!("ve_cat_{s}");
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1,'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type','egret','warehouse_id',$2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(wh)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(cat)
    .execute(pool)
    .await
    .unwrap();
    (wh, catalog)
}

async fn tag_column(f: &Fixture, tag: Uuid, wh: Uuid, db: &str, table: &str, col: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, \
         table_name, column_name) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(tag)
    .bind(wh)
    .bind(db)
    .bind(table)
    .bind(col)
    .execute(&f.db.pool)
    .await
    .unwrap();
}

struct Seeded {
    tag_id: Uuid,
    engine_name: String,
    catalog: String,
}

/// A tag with two classified columns of `sales.orders`, reachable from one
/// running StarRocks engine (engine -> profile, catalog attached, warehouse
/// tagged).
async fn seed(f: &Fixture) -> Seeded {
    let service = profile(f).await;
    let (engine_id, engine_name) = engine(f, service, "running").await;
    let (wh, catalog) = attached_catalog(f, engine_id).await;
    let tag_id = new_tag(f).await;
    for col in ["ssn", "email"] {
        tag_column(f, tag_id, wh, "sales", "orders", col).await;
    }
    Seeded {
        tag_id,
        engine_name,
        catalog,
    }
}

async fn exposure(f: &Fixture, lister: Arc<FakeLister>, tag_id: Uuid) -> serde_json::Value {
    let token = session(f).await;
    let router = app(f, Arc::new(AllowOnly(&["tag:read"])), lister);
    let (status, j) = get(
        router,
        &format!("/classifications/{tag_id}/view-exposure"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    j
}

/// `(engine, database, name, matchedBy)` of every listed view, sorted.
fn listed(j: &serde_json::Value) -> Vec<(String, String, String, String)> {
    let mut out: Vec<_> = j["starrocksMaterializedViews"]
        .as_array()
        .unwrap_or_else(|| panic!("a list: {j}"))
        .iter()
        .map(|m| {
            assert_eq!(m["catalog"], "default_catalog", "{m}");
            (
                m["engine"].as_str().unwrap().to_string(),
                m["database"].as_str().unwrap().to_string(),
                m["name"].as_str().unwrap().to_string(),
                m["matchedBy"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn row(engine: &str, db: &str, name: &str, by: &str) -> (String, String, String, String) {
    (engine.into(), db.into(), name.into(), by.into())
}

#[tokio::test]
async fn a_view_whose_base_tables_name_a_classified_table_matches_by_base_table() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    let classified = format!("{}.sales.orders", s.catalog);
    lister.answer(
        &s.engine_name,
        Ok(vec![
            mv("db1", "mv_copy_of_ssn", &["ice.other.t", &classified]),
            // Same catalog and database, an unclassified table.
            mv(
                "db1",
                "mv_customers",
                &[&format!("{}.sales.customers", s.catalog)],
            ),
            // A base-table map that names nothing classified does not fall back
            // to the text, even when the text mentions the catalog.
            MvRow {
                definition: format!("SELECT * FROM {}.sales.customers", s.catalog),
                ..mv("db2", "mv_other", &["ice.db1.t"])
            },
        ]),
    );

    let j = exposure(&f, lister.clone(), s.tag_id).await;

    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(j["trinoViews"], serde_json::json!([]), "body={j}");
    assert_eq!(j["trinoListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(j["enginesNotChecked"], serde_json::json!([]), "body={j}");
    assert_eq!(
        listed(&j),
        [row(&s.engine_name, "db1", "mv_copy_of_ssn", "baseTable")]
    );
    assert_eq!(
        lister.asked(),
        std::slice::from_ref(&s.engine_name),
        "one listing per engine, however many classified columns"
    );
}

#[tokio::test]
async fn a_view_with_no_base_table_map_matches_by_text_on_the_catalog_and_database() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(
        &s.engine_name,
        Ok(vec![
            mv_text(
                "db1",
                "mv_never_refreshed",
                &format!(
                    "CREATE MATERIALIZED VIEW mv_never_refreshed AS SELECT ssn FROM {}.sales.orders",
                    s.catalog
                ),
            ),
            // The catalog alone is not enough: the database must be named too.
            mv_text(
                "db1",
                "mv_other_db",
                &format!("SELECT * FROM {}.hr.staff", s.catalog),
            ),
        ]),
    );

    let j = exposure(&f, lister, s.tag_id).await;

    assert_eq!(
        listed(&j),
        [row(&s.engine_name, "db1", "mv_never_refreshed", "text")]
    );
}

/// Task 4 review I-3: two engines on ONE profile, each with its own attached
/// catalog. Each engine is matched against the classified tables of ITS OWN
/// attachments, and an engine that reaches no classified table is not asked.
#[tokio::test]
async fn each_engine_matches_only_the_catalogs_attached_to_it() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let service = profile(&f).await;
    let (a_id, a) = engine(&f, service, "running").await;
    let (b_id, b) = engine(&f, service, "running").await;
    let (_c_id, c) = engine(&f, service, "running").await;
    let (wh_a, cat_a) = attached_catalog(&f, a_id).await;
    let (wh_b, cat_b) = attached_catalog(&f, b_id).await;
    let tag = new_tag(&f).await;
    tag_column(&f, tag, wh_a, "sales", "orders", "ssn").await;
    tag_column(&f, tag, wh_b, "hr", "staff", "salary").await;
    let on_a = format!("{cat_a}.sales.orders");
    let on_b = format!("{cat_b}.hr.staff");
    let lister = Arc::new(FakeLister::default());
    lister.answer(
        &a,
        Ok(vec![
            mv("x", "mv_a_own", &[&on_a]),
            mv("x", "mv_a_names_b", &[&on_b]),
        ]),
    );
    lister.answer(
        &b,
        Ok(vec![
            mv("y", "mv_b_own", &[&on_b]),
            mv("y", "mv_b_names_a", &[&on_a]),
        ]),
    );

    let j = exposure(&f, lister.clone(), tag).await;

    // `listed` sorts by engine name, which is random: sort the expectation too.
    let mut expected = vec![
        row(&a, "x", "mv_a_own", "baseTable"),
        row(&b, "y", "mv_b_own", "baseTable"),
    ];
    expected.sort();
    assert_eq!(listed(&j), expected);
    let mut want = vec![a.clone(), b.clone()];
    want.sort();
    assert_eq!(lister.asked(), want, "{c} reaches no classified table");
}

#[tokio::test]
async fn an_engine_that_is_not_running_is_not_dialled_and_is_named_as_not_checked() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let service: i64 =
        sqlx::query_scalar("SELECT authz_service_id FROM engine_mgmt.engines WHERE name = $1")
            .bind(&s.engine_name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (down_id, down) = engine(&f, service, "provisioning").await;
    let (wh, cat) = attached_catalog(&f, down_id).await;
    tag_column(&f, s.tag_id, wh, "sales", "orders", "ssn").await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(
        &s.engine_name,
        Ok(vec![mv(
            "db1",
            "mv_up",
            &[&format!("{}.sales.orders", s.catalog)],
        )]),
    );
    lister.answer(
        &down,
        Ok(vec![mv(
            "db1",
            "mv_down",
            &[&format!("{cat}.sales.orders")],
        )]),
    );

    let j = exposure(&f, lister.clone(), s.tag_id).await;

    assert_eq!(
        lister.asked(),
        std::slice::from_ref(&s.engine_name),
        "body={j}"
    );
    assert_eq!(
        j["enginesNotChecked"],
        serde_json::json!([{"engine": down, "reason": "not running: provisioning"}]),
        "body={j}"
    );
    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(
        listed(&j),
        [row(&s.engine_name, "db1", "mv_up", "baseTable")]
    );
}

/// Task 4b review C-1: an external engine is stored `running`, but egret never
/// connects to one (SP4d-4) and holds no credential for it. It is not dialled
/// and is named as not checked, with the reason.
#[tokio::test]
async fn an_external_engine_is_never_dialled_and_is_named_as_not_checked() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let service: i64 =
        sqlx::query_scalar("SELECT authz_service_id FROM engine_mgmt.engines WHERE name = $1")
            .bind(&s.engine_name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (ext_id, ext) = external_engine(&f, service).await;
    let (wh, cat) = attached_catalog(&f, ext_id).await;
    tag_column(&f, s.tag_id, wh, "sales", "orders", "ssn").await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(
        &ext,
        Ok(vec![mv("db1", "mv_ext", &[&format!("{cat}.sales.orders")])]),
    );

    let j = exposure(&f, lister.clone(), s.tag_id).await;

    assert_eq!(
        lister.asked(),
        std::slice::from_ref(&s.engine_name),
        "the external engine must never be dialled; body={j}"
    );
    assert_eq!(
        j["enginesNotChecked"],
        serde_json::json!([{"engine": ext, "reason": "external"}]),
        "body={j}"
    );
    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(listed(&j), [], "body={j}");
}

#[tokio::test]
async fn an_unreachable_engine_is_an_error_and_never_an_empty_list() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(&s.engine_name, Err("connect timed out"));

    let j = exposure(&f, lister, s.tag_id).await;

    assert_eq!(
        j["starrocksMaterializedViews"],
        serde_json::Value::Null,
        "a failed listing must be null, never []; body={j}"
    );
    let err = j["starrocksListError"].as_str().expect("an error line");
    assert!(
        err.contains(&s.engine_name) && err.contains("connect timed out"),
        "the error names the engine; got {err}"
    );
}

#[tokio::test]
async fn a_classification_with_no_starrocks_column_is_empty_without_an_error() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let tag_id = new_tag(&f).await;
    let lister = Arc::new(FakeLister::default());

    let j = exposure(&f, lister.clone(), tag_id).await;

    assert_eq!(
        j["starrocksMaterializedViews"],
        serde_json::json!([]),
        "body={j}"
    );
    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(j["enginesNotChecked"], serde_json::json!([]), "body={j}");
    assert!(lister.asked().is_empty());
}

#[tokio::test]
async fn a_caller_without_tag_read_is_refused_before_anything_is_listed() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    let token = session(&f).await;
    let router = app(&f, Arc::new(AllowOnly(&["policy:read"])), lister.clone());

    let (status, _) = get(
        router,
        &format!("/classifications/{}/view-exposure", s.tag_id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(lister.asked().is_empty());
}

#[tokio::test]
async fn an_unknown_classification_is_a_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let token = session(&f).await;
    let router = app(
        &f,
        Arc::new(AllowOnly(&["tag:read"])),
        Arc::new(FakeLister::default()),
    );
    let (status, _) = get(
        router,
        &format!("/classifications/{}/view-exposure", Uuid::new_v4()),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A second running engine on the seeded profile, reaching `hr.staff` through
/// its own catalog; returns `(name, a view over that classified table)`. The
/// view MATCHES, so the engine's own list is non-empty and a partial list could
/// be returned if the null-on-any-failure rule were lost (Task 4b review I-1).
async fn second_engine(f: &Fixture, s: &Seeded) -> (String, MvRow) {
    let service: i64 =
        sqlx::query_scalar("SELECT authz_service_id FROM engine_mgmt.engines WHERE name = $1")
            .bind(&s.engine_name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (id, name) = engine(f, service, "running").await;
    let (wh, catalog) = attached_catalog(f, id).await;
    tag_column(f, s.tag_id, wh, "hr", "staff", "salary").await;
    (name, mv("db1", "mv_hr", &[&format!("{catalog}.hr.staff")]))
}

#[tokio::test]
async fn one_listing_ok_and_one_failed_is_null_and_names_only_the_failure() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let (other, matching) = second_engine(&f, &s).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(&other, Ok(vec![matching]));
    lister.answer(&s.engine_name, Err("connect refused"));

    let j = exposure(&f, lister.clone(), s.tag_id).await;

    assert_eq!(
        j["starrocksMaterializedViews"],
        serde_json::Value::Null,
        "one listing succeeded, but a partial list must never be returned; body={j}"
    );
    let err = j["starrocksListError"].as_str().expect("an error line");
    assert!(
        err.contains(&s.engine_name) && err.contains("connect refused"),
        "names the failed listing; got {err}"
    );
    assert!(
        !err.contains(&other),
        "must not name the listing that worked; got {err}"
    );
    assert_eq!(lister.asked().len(), 2, "both listings were attempted");
}

#[tokio::test]
async fn a_listing_that_never_answers_ends_at_the_deadline_as_a_named_failure() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let (other, matching) = second_engine(&f, &s).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(&other, Ok(vec![matching]));
    lister.hangs.lock().unwrap().push(s.engine_name.clone());
    let token = session(&f).await;
    let router = app_with_timeout(
        &f,
        Arc::new(AllowOnly(&["tag:read"])),
        lister.clone(),
        std::time::Duration::from_millis(100),
    );

    let started = std::time::Instant::now();
    let (status, j) = get(
        router,
        &format!("/classifications/{}/view-exposure", s.tag_id),
        &token,
    )
    .await;

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the route must answer at its deadline, not wait for the hung listing"
    );
    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(
        j["starrocksMaterializedViews"],
        serde_json::Value::Null,
        "body={j}"
    );
    let err = j["starrocksListError"].as_str().expect("an error line");
    assert!(
        err.contains(&s.engine_name) && err.contains("no answer"),
        "names what did not answer; got {err}"
    );
    assert!(
        !err.contains(&other),
        "the listing that answered is not named; got {err}"
    );
}

// ── View governance B3: POST /classifications/lineage-sweep ──────────────────

/// Answers each sweep with `answer`, counting the calls.
struct FakeSweeper {
    answer: Result<egret_catalog_engine::SweepReport, egret_catalog_engine::SweepError>,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl egret_catalog_engine::LineageSweeper for FakeSweeper {
    async fn sweep(
        &self,
    ) -> Result<egret_catalog_engine::SweepReport, egret_catalog_engine::SweepError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.answer.clone()
    }
}

async fn post_sweep(app: axum::Router, token: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/classifications/lineage-sweep")
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

fn sweeper(
    answer: Result<egret_catalog_engine::SweepReport, egret_catalog_engine::SweepError>,
) -> Arc<FakeSweeper> {
    Arc::new(FakeSweeper {
        answer,
        calls: std::sync::atomic::AtomicUsize::new(0),
    })
}

/// The sweep runs on demand for `tag:manage`, the classification write
/// permission, and its report names each view it could not judge.
#[tokio::test]
async fn the_lineage_sweep_runs_for_tag_manage_and_reports_failures() {
    let Some(f) = setup().await else { return };
    let token = session(&f).await;
    let report = egret_catalog_engine::SweepReport {
        views: 2,
        computed: 1,
        retagged: 1,
        failures: vec![egret_catalog_engine::SweepFailure {
            catalog: "ice".into(),
            schema: Some("s".into()),
            view: Some("v_bad".into()),
            reason: "engine trino-1: boom".into(),
        }],
    };
    let fake = sweeper(Ok(report));

    // `tag:read` reads a classification; it does not run the sweep.
    let app = app_full(
        &f,
        Arc::new(AllowOnly(&["tag:read"])),
        Arc::new(FakeLister::default()),
        std::time::Duration::from_secs(15),
        fake.clone(),
    );
    let (status, _) = post_sweep(app, &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(fake.calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let app = app_full(
        &f,
        Arc::new(AllowOnly(&["tag:manage"])),
        Arc::new(FakeLister::default()),
        std::time::Duration::from_secs(15),
        fake.clone(),
    );
    let (status, body) = post_sweep(app, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fake.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        body,
        serde_json::json!({
            "views": 2, "computed": 1, "retagged": 1,
            "failures": [{"catalog": "ice", "schema": "s", "view": "v_bad",
                          "reason": "engine trino-1: boom"}]
        })
    );
}

/// A sweep already running, or no embedded catalog, is a 409 that says so.
#[tokio::test]
async fn the_lineage_sweep_says_when_it_cannot_run() {
    let Some(f) = setup().await else { return };
    let token = session(&f).await;
    for (answer, says) in [
        (egret_catalog_engine::SweepError::Busy, "already running"),
        (
            egret_catalog_engine::SweepError::Unavailable(
                "the embedded catalog is not enabled".into(),
            ),
            "not enabled",
        ),
    ] {
        let app = app_full(
            &f,
            Arc::new(AllowOnly(&["tag:manage"])),
            Arc::new(FakeLister::default()),
            std::time::Duration::from_secs(15),
            sweeper(Err(answer)),
        );
        let (status, body) = post_sweep(app, &token).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body.to_string().contains(says), "{body}");
    }
}

// ---------------------------------------------------------------------------
// View governance B5 (Task 10): the Trino half, the DEFINER audit.
// ---------------------------------------------------------------------------

/// The views of egret's catalogs, and each one's stored `trino.run-as-owner`.
#[derive(Default)]
struct FakeViews {
    views: Vec<egret_catalog_engine::CatalogView>,
    failures: Vec<String>,
    unlistable: Option<String>,
    /// The listing never answers, like a lakekeeper whose metadata reads hang.
    stall: bool,
    /// View name -> its stored owner (`None` = INVOKER), or a read error.
    owners: BTreeMap<String, Result<Option<String>, String>>,
}

impl FakeViews {
    fn view(mut self, catalog: &str, name: &str, owner: Result<Option<&str>, &str>) -> Self {
        self.views.push(egret_catalog_engine::CatalogView {
            catalog: catalog.into(),
            schema: "sales".into(),
            name: name.into(),
            prefix: "lk-prefix".into(),
        });
        self.owners.insert(
            name.into(),
            owner.map(|o| o.map(String::from)).map_err(String::from),
        );
        self
    }
}

#[async_trait::async_trait]
impl egret_catalog_engine::ViewCatalog for FakeViews {
    async fn list_views(&self) -> Result<egret_catalog_engine::CatalogViews, String> {
        if self.stall {
            std::future::pending::<()>().await;
        }
        if let Some(e) = &self.unlistable {
            return Err(e.clone());
        }
        Ok(egret_catalog_engine::CatalogViews {
            views: self.views.clone(),
            failures: self.failures.clone(),
        })
    }

    async fn run_as_owner(
        &self,
        view: &egret_catalog_engine::CatalogView,
    ) -> Result<Option<String>, String> {
        self.owners
            .get(&view.name)
            .cloned()
            .unwrap_or_else(|| Err("no such view".into()))
    }
}

/// Record `tables` (`catalog.schema.table`) as `view`'s computed lineage.
async fn lineage(f: &Fixture, catalog: &str, view: &str, tables: &[(&str, &str, &str)]) {
    let mut conn = f.db.pool.acquire().await.unwrap();
    let set = tables
        .iter()
        .map(|(c, s, t)| (c.to_string(), s.to_string(), t.to_string()))
        .collect();
    egret_perm_compile::lineage::write_lineage(&mut conn, &view_ref(catalog, view), &set)
        .await
        .unwrap();
}

fn view_ref(catalog: &str, view: &str) -> egret_perm_compile::lineage::ViewRef {
    egret_perm_compile::lineage::ViewRef {
        catalog: catalog.into(),
        schema: "sales".into(),
        name: view.into(),
    }
}

async fn trino_exposure(f: &Fixture, views: FakeViews, tag_id: Uuid) -> serde_json::Value {
    let token = session(f).await;
    let router = app_views(
        f,
        Arc::new(AllowOnly(&["tag:read"])),
        Arc::new(FakeLister::default()),
        std::time::Duration::from_secs(15),
        Arc::new(egret_catalog_engine::NoCatalogSweeper),
        Arc::new(views),
        std::time::Duration::from_secs(15),
    );
    let (status, j) = get(
        router,
        &format!("/classifications/{tag_id}/view-exposure"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    j
}

/// What `trinoListError` says when egret's views could not all be listed.
const LIST_ERROR: &str = "egret's views could not all be listed, so this list is withheld \
                          rather than shown incomplete; check that the catalog is reachable, \
                          then reload the page";

/// The listed Trino view named `name`, or a panic naming the body.
fn trino_view<'a>(j: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    j["trinoViews"]
        .as_array()
        .unwrap_or_else(|| panic!("a list: {j}"))
        .iter()
        .find(|v| v["name"] == name)
        .unwrap_or_else(|| panic!("{name} is listed: {j}"))
}

fn trino_names(j: &serde_json::Value) -> Vec<String> {
    let mut n: Vec<String> = j["trinoViews"]
        .as_array()
        .unwrap_or_else(|| panic!("a list: {j}"))
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_string())
        .collect();
    n.sort();
    n
}

#[tokio::test]
async fn a_view_whose_lineage_reads_a_classified_table_is_listed_as_classified() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    lineage(&f, &s.catalog, "v_cls", &[(&s.catalog, "sales", "orders")]).await;
    // Same catalog, an unclassified table with no rule: not listed.
    lineage(
        &f,
        &s.catalog,
        "v_plain",
        &[(&s.catalog, "sales", "customers")],
    )
    .await;
    let views = FakeViews::default()
        .view(&s.catalog, "v_cls", Ok(Some("alice")))
        .view(&s.catalog, "v_plain", Ok(Some("alice")));

    let j = trino_exposure(&f, views, s.tag_id).await;

    assert_eq!(j["trinoListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(trino_names(&j), ["v_cls"], "body={j}");
    let v = trino_view(&j, "v_cls");
    assert_eq!(v["catalog"], s.catalog.as_str());
    assert_eq!(v["schema"], "sales");
    assert_eq!(v["reason"], "classified");
    assert_eq!(v["matchedBy"], "lineage");
    assert_eq!(v["lineageState"], "computed");
    assert_eq!(v["lineageDetail"], serde_json::Value::Null);
}

#[tokio::test]
async fn a_view_over_a_table_with_a_mask_rule_is_listed_as_mask_or_row_filter() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    let catalog_id: Uuid = sqlx::query_scalar("SELECT id FROM registry.catalogs WHERE name = $1")
        .bind(&s.catalog)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
         schema_name, table_name, column_name) \
         VALUES ($1, 'mask', 'MASK_NULL', 'path', $2, 'sales', 'customers', 'email')",
    )
    .bind(format!("ve-rule-{}", suffix()))
    .bind(catalog_id)
    .execute(&f.db.pool)
    .await
    .unwrap();
    lineage(
        &f,
        &s.catalog,
        "v_masked",
        &[(&s.catalog, "sales", "customers")],
    )
    .await;
    lineage(
        &f,
        &s.catalog,
        "v_other",
        &[(&s.catalog, "sales", "products")],
    )
    .await;
    let views = FakeViews::default()
        .view(&s.catalog, "v_masked", Ok(None))
        .view(&s.catalog, "v_other", Ok(None));

    let j = trino_exposure(&f, views, s.tag_id).await;

    assert_eq!(trino_names(&j), ["v_masked"], "body={j}");
    assert_eq!(trino_view(&j, "v_masked")["reason"], "mask_or_row_filter");
}

#[tokio::test]
async fn the_mode_is_definer_with_the_owner_invoker_without_and_unknown_when_unreadable() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    for v in ["v_def", "v_inv", "v_unk"] {
        lineage(&f, &s.catalog, v, &[(&s.catalog, "sales", "orders")]).await;
    }
    let views = FakeViews::default()
        .view(&s.catalog, "v_def", Ok(Some("alice")))
        .view(&s.catalog, "v_inv", Ok(None))
        .view(&s.catalog, "v_unk", Err("load_view: connection reset"));

    let j = trino_exposure(&f, views, s.tag_id).await;

    let def = trino_view(&j, "v_def");
    assert_eq!(def["securityMode"], "definer", "body={j}");
    assert_eq!(def["owner"], "alice");
    assert_eq!(def["securityModeError"], serde_json::Value::Null);
    let inv = trino_view(&j, "v_inv");
    assert_eq!(inv["securityMode"], "invoker", "body={j}");
    assert_eq!(inv["owner"], serde_json::Value::Null);
    assert_eq!(inv["ownerCleared"], serde_json::Value::Null);
    let unk = trino_view(&j, "v_unk");
    assert_eq!(unk["securityMode"], "unknown", "never definer; body={j}");
    assert_eq!(unk["owner"], serde_json::Value::Null);
    assert_eq!(unk["ownerCleared"], serde_json::Value::Null);
    assert_eq!(
        unk["securityModeError"], "the view's metadata could not be read",
        "a short message; the lakekeeper error stays in the server log; body={j}"
    );
}

/// A user with a username, holding `role` (created if needed) when given.
async fn user_holding(f: &Fixture, username: &str, role: Option<&str>) {
    let uid = f
        .users
        .insert_local(&format!("{username}@egret.local"), "T", "x")
        .await
        .unwrap()
        .0;
    sqlx::query("UPDATE users SET username = $1 WHERE id = $2")
        .bind(username)
        .bind(uid)
        .execute(&f.db.pool)
        .await
        .unwrap();
    if let Some(role) = role {
        let role_id: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name) VALUES ('platform', $1) \
             ON CONFLICT (scope, name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
        )
        .bind(role)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, \
             scope_id) VALUES ('user', $1, $2, 'platform', NULL)",
        )
        .bind(uid)
        .bind(role_id)
        .execute(&f.db.pool)
        .await
        .unwrap();
    }
}

/// `ownerCleared` reads the clearance membership the compiler uses: the owner
/// holds a required role of every gated classification the lineage reaches.
#[tokio::test]
async fn owner_cleared_says_whether_the_definer_holds_the_clearing_role() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    let role = format!("ve-role-{}", suffix());
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(s.tag_id)
    .bind(vec![role.clone()])
    .execute(&f.db.pool)
    .await
    .unwrap();
    let cleared = format!("ve_cleared_{}", suffix());
    let uncleared = format!("ve_uncleared_{}", suffix());
    user_holding(&f, &cleared, Some(&role)).await;
    user_holding(&f, &uncleared, None).await;
    for v in ["v_by_cleared", "v_by_uncleared"] {
        lineage(&f, &s.catalog, v, &[(&s.catalog, "sales", "orders")]).await;
    }
    let views = FakeViews::default()
        .view(&s.catalog, "v_by_cleared", Ok(Some(&cleared)))
        .view(&s.catalog, "v_by_uncleared", Ok(Some(&uncleared)));

    let j = trino_exposure(&f, views, s.tag_id).await;

    assert_eq!(
        trino_view(&j, "v_by_cleared")["ownerCleared"],
        true,
        "body={j}"
    );
    assert_eq!(
        trino_view(&j, "v_by_uncleared")["ownerCleared"],
        false,
        "body={j}"
    );
    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(s.tag_id)
        .execute(&f.db.pool)
        .await
        .unwrap();
}

/// Fail closed: a view whose lineage is missing or failed is listed, with its
/// lineage state, and never dropped as "reads nothing classified".
#[tokio::test]
async fn a_view_with_missing_or_failed_lineage_is_listed_with_its_lineage_state() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    {
        let mut conn = f.db.pool.acquire().await.unwrap();
        egret_perm_compile::lineage::record_lineage_failure(
            &mut conn,
            &view_ref(&s.catalog, "v_failed"),
            "engine trino-1: Access Denied",
        )
        .await
        .unwrap();
    }
    let views = FakeViews::default()
        .view(&s.catalog, "v_missing", Ok(Some("alice")))
        .view(&s.catalog, "v_failed", Ok(None));

    let j = trino_exposure(&f, views, s.tag_id).await;

    assert_eq!(trino_names(&j), ["v_failed", "v_missing"], "body={j}");
    let missing = trino_view(&j, "v_missing");
    assert_eq!(missing["lineageState"], "missing");
    assert_eq!(missing["reason"], "classified");
    assert_eq!(missing["securityMode"], "definer");
    assert_eq!(
        missing["ownerCleared"],
        serde_json::Value::Null,
        "unknown lineage, unknown clearance; body={j}"
    );
    let failed = trino_view(&j, "v_failed");
    assert_eq!(failed["lineageState"], "failed");
    assert_eq!(failed["lineageDetail"], "engine trino-1: Access Denied");
}

/// A listing that fails withholds the whole Trino list, as the StarRocks half
/// does: a partial list is never shown as complete.
#[tokio::test]
async fn a_view_listing_that_fails_is_null_with_the_error_never_a_partial_list() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    lineage(&f, &s.catalog, "v_cls", &[(&s.catalog, "sales", "orders")]).await;
    let mut partial = FakeViews::default().view(&s.catalog, "v_cls", Ok(None));
    partial.failures = vec!["the views of c.s could not be listed: boom".into()];

    let j = trino_exposure(&f, partial, s.tag_id).await;
    assert_eq!(j["trinoViews"], serde_json::Value::Null, "body={j}");
    assert_eq!(j["trinoListError"], LIST_ERROR, "body={j}");

    let none = FakeViews {
        unlistable: Some("the warehouses could not be listed: db down".into()),
        ..FakeViews::default()
    };
    let j = trino_exposure(&f, none, s.tag_id).await;
    assert_eq!(j["trinoViews"], serde_json::Value::Null, "body={j}");
    assert_eq!(
        j["trinoListError"], LIST_ERROR,
        "the internal detail stays in the server log; body={j}"
    );
}

/// Fix round 1, I1: the Trino half runs under its own deadline. A listing that
/// stalls ends as `trinoViews: null` with a timeout message, and the StarRocks
/// half still comes back.
#[tokio::test]
async fn a_stalled_view_listing_times_out_and_the_starrocks_half_still_answers() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    let classified = format!("{}.sales.orders", s.catalog);
    lister.answer(
        &s.engine_name,
        Ok(vec![mv("db1", "mv_ssn", &[&classified])]),
    );
    let token = session(&f).await;
    let router = app_views(
        &f,
        Arc::new(AllowOnly(&["tag:read"])),
        lister,
        std::time::Duration::from_secs(15),
        Arc::new(egret_catalog_engine::NoCatalogSweeper),
        Arc::new(FakeViews {
            stall: true,
            ..FakeViews::default()
        }),
        std::time::Duration::from_millis(300),
    );

    let started = std::time::Instant::now();
    let (status, j) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        get(
            router,
            &format!("/classifications/{}/view-exposure", s.tag_id),
            &token,
        ),
    )
    .await
    .expect("the report answers although the view listing never does");

    assert_eq!(status, StatusCode::OK, "body={j}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(j["trinoViews"], serde_json::Value::Null, "body={j}");
    assert_eq!(
        j["trinoListError"],
        "egret's views were not all read within 0.3 s, so this list is withheld rather than \
         shown incomplete; reload the page to try again",
        "body={j}"
    );
    assert_eq!(
        listed(&j),
        [row(&s.engine_name, "db1", "mv_ssn", "baseTable")],
        "the StarRocks half is intact; body={j}"
    );
}

/// A second gated classification on `sales.<table>` of `catalog`.
async fn second_gate(f: &Fixture, catalog: &str, table: &str, roles: &[&str], groups: &[Uuid]) {
    let wh: Uuid = sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE name = $1")
        .bind(catalog)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let tag = new_tag(f).await;
    tag_column(f, tag, wh, "sales", table, "c").await;
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, $3)",
    )
    .bind(tag)
    .bind(roles.iter().map(|r| r.to_string()).collect::<Vec<_>>())
    .bind(groups)
    .execute(&f.db.pool)
    .await
    .unwrap();
}

/// M2: cleared means cleared for EVERY reached gate, and a group-only gate
/// makes the answer unknown.
#[tokio::test]
async fn owner_cleared_needs_every_gate_and_a_group_only_gate_is_unknown() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    let role_a = format!("ve-role-a-{}", suffix());
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(s.tag_id)
    .bind(vec![role_a.clone()])
    .execute(&f.db.pool)
    .await
    .unwrap();
    let role_b = format!("ve-role-b-{}", suffix());
    second_gate(&f, &s.catalog, "pay", &[&role_b], &[]).await;
    second_gate(&f, &s.catalog, "hr", &[], &[Uuid::new_v4()]).await;
    let owner = format!("ve_owner_{}", suffix());
    user_holding(&f, &owner, Some(&role_a)).await;
    // Reads orders (gate A, held) and pay (gate B, not held).
    lineage(
        &f,
        &s.catalog,
        "v_two_gates",
        &[
            (&s.catalog, "sales", "orders"),
            (&s.catalog, "sales", "pay"),
        ],
    )
    .await;
    // Reads orders (held) and hr (group-only).
    lineage(
        &f,
        &s.catalog,
        "v_group_gate",
        &[(&s.catalog, "sales", "orders"), (&s.catalog, "sales", "hr")],
    )
    .await;
    let views = FakeViews::default()
        .view(&s.catalog, "v_two_gates", Ok(Some(&owner)))
        .view(&s.catalog, "v_group_gate", Ok(Some(&owner)));

    let j = trino_exposure(&f, views, s.tag_id).await;

    assert_eq!(
        trino_view(&j, "v_two_gates")["ownerCleared"],
        false,
        "one of two gates held is not cleared; body={j}"
    );
    assert_eq!(
        trino_view(&j, "v_group_gate")["ownerCleared"],
        serde_json::Value::Null,
        "a group-only gate cannot be answered from roles; body={j}"
    );
    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(s.tag_id)
        .execute(&f.db.pool)
        .await
        .unwrap();
}

/// M2: the rule matching covers a tag rule, a row filter and a glob path.
#[tokio::test]
async fn a_tag_rule_a_row_filter_and_a_glob_path_each_select_their_table() {
    let Some(f) = setup().await else {
        panic!("no test database");
    };
    let s = seed(&f).await;
    let pool = &f.db.pool;
    let (catalog_id, wh): (Uuid, Uuid) =
        sqlx::query_as("SELECT id, warehouse_id FROM registry.catalogs WHERE name = $1")
            .bind(&s.catalog)
            .fetch_one(pool)
            .await
            .unwrap();
    // A tag rule: an informational tag on `tagged.email`, masked by tag.
    let info_key = format!("ve-info-{}", suffix());
    let info: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
            .bind(&info_key)
            .fetch_one(pool)
            .await
            .unwrap();
    tag_column(&f, info, wh, "sales", "tagged", "email").await;
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, tag_key) \
         VALUES ($1, 'mask', 'MASK_NULL', 'tag', $2)",
    )
    .bind(format!("ve-rule-{}", suffix()))
    .bind(&info_key)
    .execute(pool)
    .await
    .unwrap();
    // A row filter on `filtered`.
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, filter_expr, selector_kind, catalog_id, \
         schema_name, table_name) VALUES ($1, 'row_filter', $3, 'path', $2, 'sales', 'filtered')",
    )
    .bind(format!("ve-rule-{}", suffix()))
    .bind(catalog_id)
    .bind("region = 'eu'")
    .execute(pool)
    .await
    .unwrap();
    // A glob mask on `glob_*`.
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
         schema_name, table_name, column_name) \
         VALUES ($1, 'mask', 'MASK_NULL', 'path', $2, 'sales', 'glob_*', 'x')",
    )
    .bind(format!("ve-rule-{}", suffix()))
    .bind(catalog_id)
    .execute(pool)
    .await
    .unwrap();
    for (v, t) in [
        ("v_tag", "tagged"),
        ("v_filter", "filtered"),
        ("v_glob", "glob_orders"),
        ("v_none", "plain"),
    ] {
        lineage(&f, &s.catalog, v, &[(&s.catalog, "sales", t)]).await;
    }
    let views = FakeViews::default()
        .view(&s.catalog, "v_tag", Ok(None))
        .view(&s.catalog, "v_filter", Ok(None))
        .view(&s.catalog, "v_glob", Ok(None))
        .view(&s.catalog, "v_none", Ok(None));

    let j = trino_exposure(&f, views, s.tag_id).await;

    assert_eq!(trino_names(&j), ["v_filter", "v_glob", "v_tag"], "body={j}");
    for v in ["v_filter", "v_glob", "v_tag"] {
        assert_eq!(
            trino_view(&j, v)["reason"],
            "mask_or_row_filter",
            "{v}; body={j}"
        );
    }
}

/// Final review (Task 4b M-1): a materialized view over a matched
/// materialized view holds the same classified values, so it is listed too,
/// at any depth (a fixpoint within one engine's listing). Its base map names
/// the inner view as `default_catalog.<db>.<name>`; one with no map matches
/// by text when its definition names the inner view's database and name.
#[tokio::test]
async fn a_materialized_view_over_a_matched_one_is_listed_at_any_depth() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    let classified = format!("{}.sales.orders", s.catalog);
    lister.answer(
        &s.engine_name,
        Ok(vec![
            // Listed before the view it reads: the order does not matter.
            mv("db3", "mv_layer3", &["default_catalog.db2.mv_layer2"]),
            mv("db2", "mv_layer2", &["DEFAULT_CATALOG.db1.mv_base"]),
            mv("db1", "mv_base", &[&classified]),
            mv_text("db4", "mv_text_layer", "SELECT * FROM db3.mv_layer3"),
            // Over an unmatched view: not listed.
            mv("db1", "mv_plain", &["ice.other.t"]),
            mv("db5", "mv_over_plain", &["default_catalog.db1.mv_plain"]),
        ]),
    );

    let j = exposure(&f, lister, s.tag_id).await;

    assert_eq!(
        listed(&j),
        vec![
            row(&s.engine_name, "db1", "mv_base", "baseTable"),
            row(&s.engine_name, "db2", "mv_layer2", "baseTable"),
            row(&s.engine_name, "db3", "mv_layer3", "baseTable"),
            row(&s.engine_name, "db4", "mv_text_layer", "text"),
        ],
        "body={j}"
    );
}

/// Final re-review N-3: `default_catalog.<db>.<name>` is the base-map spelling
/// StarRocks 4.1.6 was measured to write (Task 6), but a base map naming the
/// matched view as `<db>.<name>` is accepted too, so a spelling difference
/// cannot make a dependent view silently absent.
#[tokio::test]
async fn a_base_map_naming_the_matched_view_without_the_catalog_is_listed() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    let classified = format!("{}.sales.orders", s.catalog);
    lister.answer(
        &s.engine_name,
        Ok(vec![
            mv("db1", "mv_base", &[&classified]),
            mv("db2", "mv_short", &["DB1.mv_base"]),
            mv("db3", "mv_long", &["default_catalog.db1.mv_base"]),
            mv("db4", "mv_plain", &["db1.mv_other"]),
        ]),
    );

    let j = exposure(&f, lister, s.tag_id).await;

    assert_eq!(
        listed(&j),
        vec![
            row(&s.engine_name, "db1", "mv_base", "baseTable"),
            row(&s.engine_name, "db2", "mv_short", "baseTable"),
            row(&s.engine_name, "db3", "mv_long", "baseTable"),
        ],
        "body={j}"
    );
}
