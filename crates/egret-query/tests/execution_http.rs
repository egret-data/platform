//! Router smoke tests. Unauthenticated requests must be rejected.
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_query::execution::model::Limits;
use egret_query::DatabaseRepo;
use egret_query::{engine_router, EngineState, JobManager, QueryStore};
use egret_storage::{ObjectStore, StorageError};
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Default)]
struct MemStore(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

#[async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.lock().unwrap().insert(uri.into(), bytes);
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(uri.into()))
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.lock().unwrap().remove(uri);
        Ok(())
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        let mut m = self.0.lock().unwrap();
        let bytes = m
            .get(src)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(src.into()))?;
        m.insert(dst.into(), bytes);
        Ok(())
    }
    async fn list(
        &self,
        _prefix: &str,
        _delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        Ok(egret_storage::Listing {
            folders: Vec::new(),
            objects: Vec::new(),
        })
    }
}

/// Incidental PDP for the `requires_auth` smoke tests: those requests are
/// rejected at the session layer (401) before any authz check runs, so an
/// allow-all stub keeps them light. The `pdp_backed_*` test below builds a
/// real `CedarPdp` instead — an allow-all stub would falsely 200 the denied
/// case.
struct AllowAllPdp;
#[async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

fn build_state(db: egret_metastore::Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> EngineState {
    let store = QueryStore::new(db.clone());
    let databases = DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore::default());
    let limits = Limits {
        max_rows: 100_000,
        timeout: std::time::Duration::from_secs(60),
    };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let jobs = JobManager::new(
        store.clone(),
        databases.clone(),
        Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
        storage.clone(),
        "s3://warehouse".into(),
        "query-results".into(),
        limits.clone(),
        100,
    );
    EngineState {
        store,
        jobs,
        databases,
        storage,
        bucket_uri: "s3://warehouse".into(),
        results_prefix: "query-results".into(),
        limits,
        sessions,
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        users: egret_auth::users::UserRepo::new(db),
    }
}

async fn state() -> Option<EngineState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    Some(build_state(db, Arc::new(AllowAllPdp)))
}

#[tokio::test]
async fn submit_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = engine_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ws/eng/queries")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"database_id":"00000000-0000-0000-0000-000000000000","sql":"SELECT 1"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn history_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = engine_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/queries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Create a local user, add them to `ws` under workspace `role` (the
/// `sync_workspace_grant` trigger mirrors this into `access_grants`, which the
/// PDP reads), and return an authenticated session cookie token.
async fn member_session(
    db: &egret_metastore::Db,
    sessions: &egret_auth::session::SessionService,
    ws: Uuid,
    role: &str,
) -> String {
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("m_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role) \
         VALUES ($1, 'user', $2, $3)",
    )
    .bind(ws)
    .bind(uid)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add workspace member");
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// PDP-backed workspace authz (this seam migrated onto
/// `egret_core::authz::require`): a workspace member holding the built-in
/// `ws_editor` role — migration 0009 grants it `query:run` — can list their
/// query history (200), resolved by a real `CedarPdp`, not the allow-all
/// stub; a `ws_viewer` member (lacks `query:run`) is denied (403). The 403
/// fires on the handler's first line, exercising the migrated decision point.
#[tokio::test]
async fn pdp_backed_history_query_run_gate() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("owner_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert owner");
    let slug = format!("query-authz-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Query Authz Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    // ws_editor holds query:run (migration 0009); ws_viewer does not.
    let editor = member_session(&db, &sessions, ws, "ws_editor").await;
    let viewer = member_session(&db, &sessions, ws, "ws_viewer").await;

    let app = engine_router().with_state(st);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/queries"))
                .header("Cookie", format!("egret_session={editor}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "ws_editor (holds query:run) must list query history"
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/queries"))
                .header("Cookie", format!("egret_session={viewer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer (lacks query:run) must be denied"
    );
}
