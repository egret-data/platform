//! KI-236 at the route: `GET /storage/{id}/download` as a `data_steward` (the
//! seeded role that holds `storage:read`) and as a platform admin, through the
//! real Cedar PDP.
//!
//! The files under the storage of an egret warehouse, or of a registered
//! lakehouse catalog bound to a storage connection, are tables' data and
//! metadata. Their raw bytes skip the column masks and row filters the engines
//! apply, so a `storage:read` holder may list them but not download them, in
//! any spelling of the key, through any connection on the same bucket. Keys
//! outside every such location still download; a platform admin downloads
//! everything.
//!
//! Same lenient fake S3 as `storage_browse_scope.rs` (cleans paths, ignores
//! case). Every test uses its own key prefixes, so the locations one test
//! registers do not cover another's keys. DB-gated; FAILS without a database.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{
    control_plane_router, ConnStoreOpener, CpState, ExposureEngine, MaterializedViewLister, MvRow,
    StorageScope, SystemPrefixes, UnwiredCatalogLister,
};
use egret_metastore::Db;
use egret_storage::{Listing, ObjectMeta, ObjectStore, StorageConfig, StorageError};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const BUCKET: &str = "warehouse";

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("never lists materialized views")
    }
}

/// A lenient S3 server: keys are cleaned (`a/../b` is `b`, `//` and `.` go,
/// a leading `/` goes) and compared without case.
#[derive(Default)]
struct LenientStore {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
}

fn clean(key: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in key.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut k = out.join("/").to_lowercase();
    if key.ends_with('/') && !k.is_empty() {
        k.push('/');
    }
    k
}

fn key_of(uri: &str) -> String {
    let rest = uri.strip_prefix("s3://").expect("s3 uri");
    let (bucket, key) = rest.split_once('/').expect("bucket/key");
    assert_eq!(bucket, BUCKET);
    clean(key)
}

#[async_trait::async_trait]
impl ObjectStore for LenientStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.objects.lock().unwrap().insert(key_of(uri), bytes);
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.objects
            .lock()
            .unwrap()
            .get(&key_of(uri))
            .cloned()
            .ok_or_else(|| StorageError::NotFound(uri.to_string()))
    }
    async fn delete(&self, _uri: &str) -> Result<(), StorageError> {
        unimplemented!("browse never deletes")
    }
    async fn copy(&self, _src: &str, _dst: &str) -> Result<(), StorageError> {
        unimplemented!("browse never copies")
    }
    /// Folder-style (delimiter `/`) listing, as S3 answers it.
    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError> {
        assert_eq!(delimiter, Some("/"));
        let p = clean(prefix);
        let p = if prefix.ends_with('/') || p.is_empty() {
            p
        } else {
            p.trim_end_matches('/').to_string()
        };
        let mut folders = std::collections::BTreeSet::new();
        let mut objects = Vec::new();
        for (k, v) in self.objects.lock().unwrap().iter() {
            let Some(rest) = k.strip_prefix(&p) else {
                continue;
            };
            match rest.find('/') {
                Some(i) => {
                    folders.insert(format!("{p}{}", &rest[..=i]));
                }
                None => objects.push(ObjectMeta {
                    key: k.clone(),
                    size: v.len() as i64,
                    last_modified: None,
                }),
            }
        }
        Ok(Listing {
            folders: folders.into_iter().collect(),
            objects,
        })
    }
}

struct FakeOpener(Arc<LenientStore>);
#[async_trait::async_trait]
impl ConnStoreOpener for FakeOpener {
    async fn open(&self, cfg: &StorageConfig) -> Result<Arc<dyn ObjectStore>, StorageError> {
        assert_eq!(cfg.bucket, BUCKET);
        Ok(self.0.clone())
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    sessions: SessionService,
    app: axum::Router,
    secrets: egret_secrets::WorkspaceSecretStore,
    store: Arc<LenientStore>,
    dsn: String,
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn setup() -> Fixture {
    let dsn = egret_test_db::url()
        .await
        .expect("a database is required (DATABASE_URL)");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = Arc::new(LenientStore::default());
    let store_handle = store.clone();
    for (k, v) in [("landing/orders.csv", "id,total\n1,10\n")] {
        store
            .put(&format!("s3://{BUCKET}/{k}"), v.as_bytes().to_vec())
            .await
            .unwrap();
    }
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let state = CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets: secrets.clone(),
        sessions: sessions.clone(),
        browser: None,
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(db.clone()),
        tags: egret_bi::TagRepo::new(db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            db.clone(),
            egret_events::EventBus::new(),
        ),
        db: db.clone(),
        mv_lister: Arc::new(NoMvLister),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: Arc::new(UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
        audit_retention_days: Some(90),
        storage_scope: StorageScope {
            system: SystemPrefixes::new(BUCKET, ["query-results", "apps/", "workflow/"]),
            opener: Arc::new(FakeOpener(store)),
        },
    };
    Fixture {
        app: control_plane_router().with_state(state),
        db,
        users,
        sessions,
        secrets,
        store: store_handle,
        dsn,
    }
}

/// A session holding the seeded platform role `role` (`data_steward` or
/// `platform_admin`).
async fn session_with_role(f: &Fixture, role: &str) -> String {
    let uid = f
        .users
        .insert_local(&format!("{}@egret.local", unique("ki236")), "T", "x")
        .await
        .expect("user");
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles WHERE scope = 'platform' AND name = $2",
    )
    .bind(uid.0)
    .bind(role)
    .execute(&f.db.pool)
    .await
    .expect("grant role")
    .rows_affected();
    assert_eq!(n, 1, "seeded role {role}");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

/// A connection on the platform bucket with `key_prefix`.
async fn connection(f: &Fixture, key_prefix: Option<&str>) -> Uuid {
    let secret = f
        .secrets
        .create(None, &unique("ki236-key"), None, "s3cr3t")
        .await
        .expect("secret");
    egret_storage::StorageRegistry::new(f.db.clone())
        .create_storage_conn(&egret_storage::StorageConnInput {
            name: unique("ki236-conn"),
            purpose: None,
            provider: "s3".into(),
            endpoint: Some("http://rustfs:9000".into()),
            region: "us-east-1".into(),
            bucket: BUCKET.into(),
            key_prefix: key_prefix.map(str::to_string),
            access_key: "egret".into(),
            secret_ref: Some(secret),
            path_style: true,
        })
        .await
        .expect("connection")
        .id
}

async fn get(f: &Fixture, uri: &str, token: &str) -> (StatusCode, Vec<u8>) {
    let resp = f
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(uri)
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (
        status,
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| {
        panic!("not JSON: {}", String::from_utf8_lossy(bytes));
    })
}

fn strings(v: &Value, field: &str) -> Vec<String> {
    v[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} in {v}"))
        .iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            o => o["key"].as_str().expect("key").to_string(),
        })
        .collect()
}

/// The object `key` (from the bucket root) holding `bytes`.
async fn put(f: &Fixture, key: &str, bytes: &str) {
    f.store
        .put(&format!("s3://{BUCKET}/{key}"), bytes.as_bytes().to_vec())
        .await
        .unwrap();
}

/// An egret warehouse on a new connection with Key prefix `prefix`; returns
/// the connection.
async fn warehouse_on(f: &Fixture, prefix: &str) -> Uuid {
    let conn = connection(f, Some(prefix)).await;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$1) RETURNING id")
            .bind(unique("ki236-ws"))
            .fetch_one(&f.db.pool)
            .await
            .expect("workspace");
    egret_catalog_engine::CatalogStore::new(f.db.clone())
        .create_warehouse(&unique("ki236-wh"), ws, conn)
        .await
        .expect("warehouse");
    conn
}

/// A registered Hive catalog bound to a new connection with Key prefix `prefix`.
async fn hive_catalog_on(f: &Fixture, prefix: &str) {
    let conn = connection(f, Some(prefix)).await;
    sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, storage_connection_id) \
         VALUES ($1, 'hive', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'hive', 'uri', 'thrift://hms:9083'), \
                 'storage_connection_id', $2::uuid::text), $2)",
    )
    .bind(unique("ki236_hive").replace('-', "_"))
    .bind(conn)
    .execute(&f.db.pool)
    .await
    .expect("hive catalog");
}

fn lower_unique(prefix: &str) -> String {
    unique(prefix).to_lowercase()
}

async fn download(f: &Fixture, conn: Uuid, key: &str, token: &str) -> (StatusCode, Vec<u8>) {
    get(f, &format!("/storage/{conn}/download?key={key}"), token).await
}

#[tokio::test]
async fn a_data_steward_cannot_download_a_warehouse_table_file() {
    let f = setup().await;
    let lake = lower_unique("lake");
    warehouse_on(&f, &lake).await;
    let data = format!("{lake}/01a1/orders-9f3e/data/00000-x.parquet");
    let meta = format!("{lake}/01a1/orders-9f3e/metadata/00001-y.metadata.json");
    put(&f, &data, "PAR1 rows nobody masked").await;
    put(&f, &meta, "{\"format-version\":2}").await;
    // Browsing the bucket root, as Storage -> demo-rustfs does.
    let root = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    let upper = data.to_uppercase();
    let encoded = data.replacen('/', "%2F", 1);
    let twice = data.replacen('/', "%252F", 1);
    for key in [
        data.as_str(),
        meta.as_str(),
        upper.as_str(),
        encoded.as_str(),
        twice.as_str(),
    ] {
        let (status, body) = download(&f, root, key, &steward).await;
        let text = String::from_utf8_lossy(&body);
        assert_eq!(status, StatusCode::FORBIDDEN, "download {key}: {text}");
        assert!(
            text.contains("warehouse «ki236-wh") && text.contains("platform admin"),
            "the 403 says whose tables and who can: {text}"
        );
    }

    // Through the warehouse's own connection (Key prefix = the warehouse's).
    let own = connection(&f, Some(&lake)).await;
    let rel = data.strip_prefix(&format!("{lake}/")).unwrap();
    let (status, _) = download(&f, own, rel, &steward).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "download {rel} under the prefix"
    );

    // Listing stays: names and sizes are what storage:read is for.
    let (status, body) = get(
        &f,
        &format!("/storage/{root}/objects?prefix={lake}/01a1/orders-9f3e/data/"),
        &steward,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(strings(&json(&body), "objects"), [data.clone()]);
}

#[tokio::test]
async fn a_data_steward_cannot_download_a_registered_lakehouse_catalog_file() {
    let f = setup().await;
    let hive = lower_unique("hive");
    hive_catalog_on(&f, &hive).await;
    let key = format!("{hive}/sales.db/orders/part-00000.orc");
    put(&f, &key, "ORC rows nobody masked").await;
    let root = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    let (status, body) = download(&f, root, &key, &steward).await;
    let text = String::from_utf8_lossy(&body);
    assert_eq!(status, StatusCode::FORBIDDEN, "download {key}: {text}");
    assert!(text.contains("catalog «ki236_hive"), "{text}");
}

#[tokio::test]
async fn a_data_steward_still_downloads_a_file_outside_every_table_location() {
    let f = setup().await;
    let lake = lower_unique("lake");
    warehouse_on(&f, &lake).await;
    // A name that only starts like the warehouse's prefix is not under it.
    let near = format!("{lake}-old/a.csv");
    put(&f, &near, "near\n").await;
    let root = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    let (status, body) = download(&f, root, "landing/orders.csv", &steward).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(body, b"id,total\n1,10\n");
    let (status, body) = download(&f, root, &near, &steward).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
async fn a_platform_admin_downloads_a_table_file() {
    let f = setup().await;
    let lake = lower_unique("lake");
    warehouse_on(&f, &lake).await;
    let data = format!("{lake}/ns/t/data/x.parquet");
    put(&f, &data, "PAR1 admin").await;
    let root = connection(&f, None).await;
    let admin = session_with_role(&f, "platform_admin").await;

    let (status, body) = download(&f, root, &data, &admin).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(body, b"PAR1 admin");
}

/// The embedded catalog keeps its own copy of a warehouse's storage profile,
/// which is where the engines write. A later edit of the connection does not
/// move the tables, so the profile's prefix is closed on its own, with no
/// connection pointing at it any more.
#[tokio::test]
async fn a_lakekeeper_warehouse_profile_is_closed_without_a_connection_at_its_prefix() {
    let f = setup().await;
    egret_catalog_engine::run_lakekeeper_migrations(&f.dsn)
        .await
        .expect("lakekeeper migrations");
    let old = lower_unique("lk-old");
    let project = unique("ki236-project");
    sqlx::query("INSERT INTO lakekeeper.project (project_id, project_name) VALUES ($1, $1)")
        .bind(&project)
        .execute(&f.db.pool)
        .await
        .expect("project");
    sqlx::query(
        "INSERT INTO lakekeeper.warehouse \
           (warehouse_name, storage_profile, status, tabular_delete_mode, project_id) \
         VALUES ($1, jsonb_build_object('type', 's3', 'bucket', $2::text, 'key-prefix', $3::text), \
                 'active', 'hard', $4)",
    )
    .bind(unique("ki236-lk"))
    .bind(BUCKET)
    .bind(&old)
    .bind(&project)
    .execute(&f.db.pool)
    .await
    .expect("lakekeeper warehouse");
    let data = format!("{old}/ns/t/data/x.parquet");
    put(&f, &data, "PAR1 written before the move").await;
    let root = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    let (status, body) = download(&f, root, &data, &steward).await;
    let text = String::from_utf8_lossy(&body);
    assert_eq!(status, StatusCode::FORBIDDEN, "download {data}: {text}");
    assert!(text.contains("warehouse «ki236-lk"), "{text}");
}
