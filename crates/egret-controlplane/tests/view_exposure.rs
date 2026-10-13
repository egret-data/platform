//! `GET /api/control-plane/classifications/{id}/view-exposure` (view governance
//! A4): the materialized views that already exist in the databases holding a
//! classification's columns. DB-backed through the per-process test DB; the
//! StarRocks FE is a fake `MaterializedViewLister`, so no live engine is needed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{control_plane_router, CpState, ExposureEngine, MaterializedViewLister};
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

type Listing = Result<Vec<String>, String>;

/// `(engine name, "catalog.database")` -> views, or an error message.
#[derive(Default)]
struct FakeLister {
    answers: Mutex<BTreeMap<(String, String), Listing>>,
    asked: Mutex<Vec<(String, String)>>,
}

impl FakeLister {
    fn answer(&self, engine: &str, db: &str, r: Result<Vec<&str>, &str>) {
        self.answers.lock().unwrap().insert(
            (engine.to_string(), db.to_string()),
            r.map(|v| v.into_iter().map(String::from).collect())
                .map_err(String::from),
        );
    }
}

#[async_trait::async_trait]
impl MaterializedViewLister for FakeLister {
    async fn list(
        &self,
        engine: &ExposureEngine,
        catalog: &str,
        database: &str,
    ) -> Result<Vec<String>, String> {
        let key = (engine.name.clone(), format!("{catalog}.{database}"));
        self.asked.lock().unwrap().push(key.clone());
        self.answers
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Ok(vec![]))
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

struct Seeded {
    tag_id: Uuid,
    engine_name: String,
    catalog: String,
}

/// A tag with one classified column in `sales`, reachable from one StarRocks
/// engine (engine -> authz service, catalog attached, warehouse tagged).
async fn seed(f: &Fixture) -> Seeded {
    let pool = &f.db.pool;
    let suffix = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) \
         VALUES (910000001, 'starrocks', '{}'::jsonb) ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    let service: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'starrocks', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("ve-svc-{suffix}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'W') RETURNING id")
            .bind(format!("ve-ws-{suffix}"))
            .fetch_one(pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(format!("ve-sc-{suffix}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("ve_wh_{suffix}"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .unwrap();
    let engine_name = format!("ve-eng-{suffix}");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(&engine_name)
    .bind(service)
    .fetch_one(pool)
    .await
    .unwrap();
    let catalog = format!("ve_cat_{suffix}");
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
    .bind(engine)
    .bind(cat)
    .execute(pool)
    .await
    .unwrap();
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1,$2) RETURNING id")
            .bind(format!("ve-tag-{suffix}"))
            .bind(format!("VE {suffix}"))
            .fetch_one(pool)
            .await
            .unwrap();
    // Two columns in the same database: the database must be listed ONCE.
    for col in ["ssn", "email"] {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, \
             table_name, column_name) VALUES ($1,$2,'sales','orders',$3)",
        )
        .bind(tag_id)
        .bind(wh)
        .bind(col)
        .execute(pool)
        .await
        .unwrap();
    }
    Seeded {
        tag_id,
        engine_name,
        catalog,
    }
}

#[tokio::test]
async fn lists_the_views_in_the_classified_database_once() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let s = seed(&f).await;
    let lister = Arc::new(FakeLister::default());
    lister.answer(
        &s.engine_name,
        &format!("{}.sales", s.catalog),
        Ok(vec!["mv_copy_of_ssn", "mv_daily"]),
    );
    let token = session(&f).await;
    let router = app(&f, Arc::new(AllowOnly(&["tag:read"])), lister.clone());

    let (status, j) = get(
        router,
        &format!("/classifications/{}/view-exposure", s.tag_id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert_eq!(j["trinoViews"], serde_json::json!([]), "body={j}");
    let mvs = j["starrocksMaterializedViews"].as_array().expect("a list");
    let names: Vec<&str> = mvs.iter().map(|m| m["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["mv_copy_of_ssn", "mv_daily"], "body={j}");
    assert_eq!(mvs[0]["engine"], s.engine_name);
    assert_eq!(mvs[0]["catalog"], s.catalog);
    assert_eq!(mvs[0]["database"], "sales");
    assert_eq!(
        lister.asked.lock().unwrap().len(),
        1,
        "two classified columns in one database are one listing"
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
    lister.answer(
        &s.engine_name,
        &format!("{}.sales", s.catalog),
        Err("connect timed out"),
    );
    let token = session(&f).await;
    let router = app(&f, Arc::new(AllowOnly(&["tag:read"])), lister);

    let (status, j) = get(
        router,
        &format!("/classifications/{}/view-exposure", s.tag_id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(
        j["starrocksMaterializedViews"],
        serde_json::Value::Null,
        "a failed listing must be null, never []; body={j}"
    );
    let err = j["starrocksListError"].as_str().expect("an error line");
    assert!(
        err.contains(&s.engine_name) && err.contains("sales") && err.contains("connect timed out"),
        "the error names the engine and database; got {err}"
    );
}

#[tokio::test]
async fn a_classification_with_no_starrocks_column_is_empty_without_an_error() {
    let Some(f) = setup().await else {
        eprintln!("skip: no test database");
        return;
    };
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1,$2) RETURNING id")
            .bind(format!("ve-bare-{}", Uuid::new_v4().simple()))
            .bind(format!("Bare {}", Uuid::new_v4().simple()))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let lister = Arc::new(FakeLister::default());
    let token = session(&f).await;
    let router = app(&f, Arc::new(AllowOnly(&["tag:read"])), lister.clone());

    let (status, j) = get(
        router,
        &format!("/classifications/{tag_id}/view-exposure"),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(
        j["starrocksMaterializedViews"],
        serde_json::json!([]),
        "body={j}"
    );
    assert_eq!(j["starrocksListError"], serde_json::Value::Null, "body={j}");
    assert!(lister.asked.lock().unwrap().is_empty());
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
    assert!(lister.asked.lock().unwrap().is_empty());
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
