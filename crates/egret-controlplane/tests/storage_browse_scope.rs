//! KI-206 and KI-129 at the route: `GET /storage/{id}/objects` and
//! `GET /storage/{id}/download` as a `data_steward` (the seeded role that
//! holds `storage:read`) and as a platform admin, through the real Cedar PDP.
//!
//! - KI-206: the prefixes egret writes in its own bucket (stored SQL Lab
//!   results, Data App sources, workflow files) are not listed and not served
//!   to a `storage:read` holder, in any spelling of the key; a normal key still
//!   is; a platform admin opens them.
//! - KI-129: a connection with a Key prefix browses and downloads under that
//!   prefix, not the bucket root.
//!
//! The object store is a fake behind `StorageScope::opener` that models a
//! lenient S3 server: it cleans `.`/`..`/empty segments and ignores case, as a
//! server on a case-insensitive disk or one that normalises paths would. DB-gated;
//! FAILS without a database.

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
    for (k, v) in [
        ("demo/orders.csv", "id,total\n1,10\n"),
        (
            "query-results/3f1c.jsonl",
            "[\"someone else's post-mask row\"]\n",
        ),
        ("apps/ws/app/draft/1.py", "import marimo\n"),
        ("workflow/ws/flow/executions/e/out.txt", "flow output\n"),
        ("sandbox/inner/x.csv", "x\n"),
        ("sandbox/s.csv", "sandbox file\n"),
    ] {
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
    }
}

/// A session holding the seeded platform role `role` (`data_steward` or
/// `platform_admin`).
async fn session_with_role(f: &Fixture, role: &str) -> String {
    let uid = f
        .users
        .insert_local(&format!("{}@egret.local", unique("f2")), "T", "x")
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
        .create(None, &unique("f2-key"), None, "s3cr3t")
        .await
        .expect("secret");
    egret_storage::StorageRegistry::new(f.db.clone())
        .create_storage_conn(&egret_storage::StorageConnInput {
            name: unique("f2-conn"),
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

#[tokio::test]
async fn a_data_steward_does_not_see_system_prefixes_in_the_bucket_root() {
    let f = setup().await;
    let conn = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    let (status, body) = get(&f, &format!("/storage/{conn}/objects"), &steward).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let listing = json(&body);
    assert_eq!(strings(&listing, "folders"), ["demo/", "sandbox/"]);
    assert_eq!(
        strings(&listing, "hiddenSystemFolders"),
        ["apps/", "query-results/", "workflow/"],
        "the page can say what it left out"
    );
}

#[tokio::test]
async fn a_data_steward_cannot_list_or_download_under_a_system_prefix() {
    let f = setup().await;
    let conn = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    for prefix in ["query-results/", "Query-Results/", "apps/", "workflow/ws/"] {
        let (status, body) = get(
            &f,
            &format!("/storage/{conn}/objects?prefix={prefix}"),
            &steward,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "list {prefix}");
        let msg = json(&body)["error"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            msg.contains("platform admin"),
            "the 403 says why and who can: {msg}"
        );
    }
    for key in [
        "query-results/3f1c.jsonl",
        "QUERY-RESULTS/3f1c.jsonl",
        // URL-encoded once (axum decodes it) and twice (a server that decodes again).
        "query%2Dresults%2F3f1c.jsonl",
        "query%252Dresults%252F3f1c.jsonl",
        "apps/ws/app/draft/1.py",
        "workflow/ws/flow/executions/e/out.txt",
    ] {
        let (status, body) =
            get(&f, &format!("/storage/{conn}/download?key={key}"), &steward).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "download {key}: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn path_tricks_are_refused() {
    let f = setup().await;
    let root = connection(&f, None).await;
    let sandbox = connection(&f, Some("sandbox")).await;
    let steward = session_with_role(&f, "data_steward").await;

    for (conn, key) in [
        (sandbox, "../query-results/3f1c.jsonl"),
        (sandbox, "inner/../../query-results/3f1c.jsonl"),
        (sandbox, "%2e%2e/query-results/3f1c.jsonl"),
        (sandbox, "%252e%252e%252fquery-results/3f1c.jsonl"),
        (sandbox, "..%5cquery-results/3f1c.jsonl"),
        (root, "/query-results/3f1c.jsonl"),
        (root, "demo//../query-results/3f1c.jsonl"),
        (root, "./query-results/3f1c.jsonl"),
        (sandbox, "../demo/orders.csv"),
    ] {
        let (status, body) =
            get(&f, &format!("/storage/{conn}/download?key={key}"), &steward).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
            "download {key} answered {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    for prefix in ["../", "%2e%2e/", "/", "demo//"] {
        let (status, _) = get(
            &f,
            &format!("/storage/{sandbox}/objects?prefix={prefix}"),
            &steward,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "list {prefix}");
    }
}

#[tokio::test]
async fn a_data_steward_still_browses_and_downloads_a_normal_key() {
    let f = setup().await;
    let conn = connection(&f, None).await;
    let steward = session_with_role(&f, "data_steward").await;

    let (status, body) = get(
        &f,
        &format!("/storage/{conn}/objects?prefix=demo/"),
        &steward,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(strings(&json(&body), "objects"), ["demo/orders.csv"]);

    let (status, body) = get(
        &f,
        &format!("/storage/{conn}/download?key=demo/orders.csv"),
        &steward,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"id,total\n1,10\n");
}

#[tokio::test]
async fn a_platform_admin_opens_system_prefixes() {
    let f = setup().await;
    let conn = connection(&f, None).await;
    let admin = session_with_role(&f, "platform_admin").await;

    let (status, body) = get(&f, &format!("/storage/{conn}/objects"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    let listing = json(&body);
    assert_eq!(
        strings(&listing, "folders"),
        ["apps/", "demo/", "query-results/", "sandbox/", "workflow/"]
    );
    assert!(strings(&listing, "hiddenSystemFolders").is_empty());

    let (status, body) = get(
        &f,
        &format!("/storage/{conn}/download?key=query-results/3f1c.jsonl"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"[\"someone else's post-mask row\"]\n");

    // Path tricks stay refused for an admin too.
    let (status, _) = get(
        &f,
        &format!("/storage/{conn}/download?key=demo/../query-results/3f1c.jsonl"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// KI-129.
#[tokio::test]
async fn a_key_prefix_scopes_browse_and_download() {
    let f = setup().await;
    let conn = connection(&f, Some("sandbox")).await;
    let steward = session_with_role(&f, "data_steward").await;

    let (status, body) = get(&f, &format!("/storage/{conn}/objects"), &steward).await;
    assert_eq!(status, StatusCode::OK);
    let listing = json(&body);
    assert_eq!(listing["root"], "sandbox/");
    assert_eq!(strings(&listing, "folders"), ["inner/"]);
    assert_eq!(strings(&listing, "objects"), ["s.csv"]);

    let (status, body) = get(
        &f,
        &format!("/storage/{conn}/objects?prefix=inner/"),
        &steward,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(strings(&json(&body), "objects"), ["inner/x.csv"]);

    let (status, body) = get(&f, &format!("/storage/{conn}/download?key=s.csv"), &steward).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"sandbox file\n");

    // The bucket root's objects are not reachable through the prefix.
    let (status, _) = get(
        &f,
        &format!("/storage/{conn}/download?key=demo/orders.csv"),
        &steward,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}
