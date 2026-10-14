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

    assert_eq!(
        listed(&j),
        [
            row(&a, "x", "mv_a_own", "baseTable"),
            row(&b, "y", "mv_b_own", "baseTable"),
        ]
    );
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
        serde_json::json!([{"engine": down, "status": "provisioning"}]),
        "body={j}"
    );
    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(
        listed(&j),
        [row(&s.engine_name, "db1", "mv_up", "baseTable")]
    );
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
/// its own catalog; returns its name.
async fn second_engine(f: &Fixture, s: &Seeded) -> String {
    let service: i64 =
        sqlx::query_scalar("SELECT authz_service_id FROM engine_mgmt.engines WHERE name = $1")
            .bind(&s.engine_name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (id, name) = engine(f, service, "running").await;
    let (wh, _) = attached_catalog(f, id).await;
    tag_column(f, s.tag_id, wh, "hr", "staff", "salary").await;
    name
}

#[tokio::test]
async fn one_listing_ok_and_one_failed_is_null_and_names_only_the_failure() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let other = second_engine(&f, &s).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(&other, Ok(vec![mv("db1", "mv_hr", &["whatever.hr.staff"])]));
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
    let other = second_engine(&f, &s).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(&other, Ok(vec![]));
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
