//! KI-207 at the route: a secret egret mints for itself
//! (`WorkspaceSecretStore::create_system`) is listed with `system_managed:
//! true`, and `PUT`/`DELETE /secrets/{id}` refuse to rename, rotate or delete
//! it (409, saying why) while a person's secret is still managed as before.
//! DB-gated; FAILS without a database.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow, StorageScope,
    SystemPrefixes, UnwiredCatalogLister,
};
use egret_metastore::Db;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("never lists materialized views")
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
    format!("{prefix}-{}", Uuid::new_v4().simple())
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
        storage_scope: StorageScope::s3(SystemPrefixes::default()),
    };
    Fixture {
        app: control_plane_router().with_state(state),
        db,
        users,
        sessions,
        secrets,
    }
}

async fn admin(f: &Fixture) -> String {
    let uid = f
        .users
        .insert_local(&format!("{}@egret.local", unique("f2a")), "T", "x")
        .await
        .expect("user");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles \
          WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(uid.0)
    .execute(&f.db.pool)
    .await
    .expect("platform_admin");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn call(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = f.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes) })),
    )
}

#[tokio::test]
async fn a_system_secret_is_marked_and_cannot_be_renamed_rotated_or_deleted() {
    let f = setup().await;
    let token = admin(&f).await;
    let sys_name = unique("engine-internal");
    let sys = f
        .secrets
        .create_system(
            &sys_name,
            Some("Trino internal-communication shared secret"),
            "minted",
        )
        .await
        .expect("system secret");
    let mine = f
        .secrets
        .create(None, &unique("f2-mine"), None, "mine")
        .await
        .expect("user secret");

    let (status, list) = call(&f, "GET", "/secrets", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let flag = |id: Uuid| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id.to_string())
            .unwrap_or_else(|| panic!("{id} listed"))["system_managed"]
            .clone()
    };
    assert_eq!(flag(sys), json!(true));
    assert_eq!(flag(mine), json!(false));

    for body in [
        json!({ "name": "renamed" }),
        json!({ "value": "rotated" }),
        json!({ "type": "Opaque", "data": { "value": "rotated" } }),
    ] {
        let (status, err) = call(
            &f,
            "PUT",
            &format!("/secrets/{sys}"),
            &token,
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "PUT {body}: {err}");
        assert!(
            err["error"]
                .as_str()
                .unwrap_or_default()
                .contains("managed by egret"),
            "the 409 says why: {err}"
        );
    }
    let (status, err) = call(&f, "DELETE", &format!("/secrets/{sys}"), &token, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "DELETE: {err}");

    // Nothing changed.
    let meta = f.secrets.get_meta(None, sys).await.expect("still there");
    assert_eq!(meta.name, sys_name);
    assert_eq!(
        f.secrets.resolve_plaintext(None, sys).await.unwrap(),
        "minted"
    );

    // A person's secret is managed as before.
    let (status, _) = call(
        &f,
        "PUT",
        &format!("/secrets/{mine}"),
        &token,
        Some(json!({ "value": "v2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&f, "DELETE", &format!("/secrets/{mine}"), &token, None).await;
    assert_eq!(status, StatusCode::OK);
}
