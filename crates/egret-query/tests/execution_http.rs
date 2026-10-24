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

/// B3a: fake engine-token minter. `Some(token)` → mint ok (an OIDC user with a
/// refresh token); `None` → `NoRefreshToken` (a non-OIDC user, must be denied).
struct FakeMinter {
    token: Option<String>,
}
#[async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for FakeMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        match &self.token {
            Some(t) => Ok(egret_analytics_api::engine_token::EngineToken {
                access_token: t.clone(),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            }),
            None => Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken),
        }
    }
}

/// Stub minter for the non-delegation tests: never called on a non-delegated
/// database (the submit handler only mints when `db.delegated`).
struct NoMinter;
#[async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

/// Shared slot the capturing factory fills with the `(session_user, bearer_token)`
/// the submit path resolved and threaded down to the async job.
type CapturedArgs = Arc<std::sync::Mutex<Option<(Option<String>, Option<String>)>>>;

/// Fake connector factory that records the `(session_user, bearer_token)` the
/// submit path threaded down to the async job, then fails the connection (the
/// query itself never runs — we only assert what delegation resolved). The
/// captured tuple is filled once the spawned job reaches `run_query`.
#[derive(Clone, Default)]
struct CapturingFactory {
    captured: CapturedArgs,
}
#[async_trait]
impl egret_query::execution::connector::ConnectorFactory for CapturingFactory {
    async fn build(
        &self,
        _db: &egret_query::Database,
        _secret: Option<String>,
        session_user: Option<String>,
        bearer_token: Option<String>,
    ) -> Result<Box<dyn egret_query::execution::connector::Connector>, egret_query::EngineError>
    {
        *self.captured.lock().unwrap() = Some((session_user, bearer_token));
        Err(egret_query::EngineError::Connect("captured (test)".into()))
    }
}

/// Fixed AES key so `build_state`'s secret store can seal/open credentials in
/// tests (the delegated impersonate row binds a real, decryptable delegator
/// secret). Existing tests bind no `secret_id`, so their `open_secret` returns
/// `Ok(None)` regardless of whether crypto is configured — no behavior change.
fn test_crypto() -> egret_secrets::Crypto {
    egret_secrets::Crypto::new(&[7u8; 32]).expect("test crypto")
}

fn build_state(
    db: egret_metastore::Db,
    pdp: Arc<dyn egret_core::authz::Pdp>,
    minter: Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
    factory: Arc<dyn egret_query::execution::connector::ConnectorFactory>,
) -> EngineState {
    let store = QueryStore::new(db.clone());
    let databases = DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(test_crypto())),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), Some(test_crypto())),
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
        factory,
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
        engine_minter: minter,
    }
}

async fn state() -> Option<EngineState> {
    let url = egret_test_db::url().await?;
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    Some(build_state(
        db,
        Arc::new(AllowAllPdp),
        Arc::new(NoMinter),
        Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
    ))
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
    member_session_with_id(db, sessions, ws, role).await.1
}

/// [`member_session`], also returning the new user's id.
async fn member_session_with_id(
    db: &egret_metastore::Db,
    sessions: &egret_auth::session::SessionService,
    ws: Uuid,
    role: &str,
) -> (Uuid, String) {
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("m_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .bind(uid)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add workspace member");
    let token = sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    (uid, token)
}

/// PDP-backed workspace authz (this seam migrated onto
/// `egret_core::authz::require`): a workspace member holding the built-in
/// `ws_editor` role — migration 0009 grants it `query:run` — can list their
/// query history (200), resolved by a real `CedarPdp`, not the allow-all
/// stub; a `ws_viewer` member (lacks `query:run`) is denied (403). The 403
/// fires on the handler's first line, exercising the migrated decision point.
#[tokio::test]
async fn pdp_backed_history_query_run_gate() {
    let Some(url) = egret_test_db::url().await else {
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
    let st = build_state(
        db.clone(),
        pdp,
        Arc::new(NoMinter),
        Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
    );
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

/// Poll the capturing factory until the spawned job reaches `run_query` and
/// records the `(session_user, bearer_token)` it was built with (~2s budget).
async fn wait_capture(m: &CapturedArgs) -> (Option<String>, Option<String>) {
    for _ in 0..100 {
        if let Some(v) = m.lock().unwrap().clone() {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("connector factory was never invoked (async job did not reach run_query)");
}

/// B3a Task 4: per-user delegation on the SQL Lab submit path, asserted through
/// the real `POST /ws/{ws}/queries` HTTP handler with a fake minter and a
/// capturing connector factory. Three branches:
///  * DELEGATED db + an OIDC user (mint OK) → 202, and the async job is built
///    with `session_user = the user's username` (NOT their email) plus the minted
///    JWT as the bearer.
///  * DELEGATED db + a non-OIDC user (mint → `NoRefreshToken`) → 403 (no silent
///    service-cred fallback).
///  * NON-DELEGATED db → 202, `session_user = caller email`, NO bearer — byte-
///    identical to the prior shared-service behavior.
#[tokio::test]
async fn submit_delegation_branches() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");

    // A workspace + a member holding query:run (ws_editor, migration 0009 grants it).
    let email = format!("deleg_{}@egret.local", Uuid::new_v4().simple());
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(&email)
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let slug = format!("deleg-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Delegation Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_editor' ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("add workspace member");

    let delegated_db: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8080,$2,true) RETURNING id",
    )
    .bind(format!("d-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("insert delegated db");
    let plain_db: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8080,$2,false) RETURNING id",
    )
    .bind(format!("p-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("insert plain db");

    // Real CedarPdp so query:run resolves for the ws_editor member; the 403 case
    // below then fires from the delegation mint, not from an authz denial.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );

    let expected_username = egret_auth::users::UserRepo::new(db.clone())
        .ensure_username(egret_core::ids::UserId(uid))
        .await
        .expect("ensure_username");
    assert_ne!(
        expected_username, email,
        "sanity: username must differ from email so the assertions are meaningful"
    );

    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let token = sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let post = |dbid: Uuid| {
        Request::builder()
            .method("POST")
            .uri(format!("/ws/{slug}/queries"))
            .header("content-type", "application/json")
            .header("Cookie", format!("egret_session={token}"))
            .body(Body::from(format!(
                r#"{{"database_id":"{dbid}","sql":"SELECT 1"}}"#
            )))
            .unwrap()
    };

    // Branch 1: DELEGATED + mint OK → 202, and the job runs as the username + JWT.
    let cap1 = Arc::new(CapturingFactory::default());
    let st1 = build_state(
        db.clone(),
        pdp.clone(),
        Arc::new(FakeMinter {
            token: Some("fake-engine-jwt".into()),
        }),
        cap1.clone(),
    );
    let resp = engine_router()
        .with_state(st1)
        .oneshot(post(delegated_db))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "delegated + OIDC user (mint OK) must be accepted"
    );
    let (session_user, bearer) = wait_capture(&cap1.captured).await;
    assert_eq!(
        session_user,
        Some(expected_username.clone()),
        "delegated: session_user MUST be the username (preferred_username), not the email"
    );
    assert_eq!(
        bearer,
        Some("fake-engine-jwt".to_string()),
        "delegated: the minted per-user JWT MUST be attached as the bearer"
    );

    // Branch 2: DELEGATED + a non-OIDC user (no refresh token) → 403, no fallback.
    let st2 = build_state(
        db.clone(),
        pdp.clone(),
        Arc::new(FakeMinter { token: None }),
        Arc::new(CapturingFactory::default()),
    );
    let resp = engine_router()
        .with_state(st2)
        .oneshot(post(delegated_db))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "delegated + no refresh token → 403 (not a silent service-cred fallback)"
    );

    // Branch 3: NON-DELEGATED → byte-identical shared-service behavior: email, no bearer.
    let cap3 = Arc::new(CapturingFactory::default());
    let st3 = build_state(
        db.clone(),
        pdp.clone(),
        Arc::new(FakeMinter {
            token: Some("unused".into()),
        }),
        cap3.clone(),
    );
    let resp = engine_router()
        .with_state(st3)
        .oneshot(post(plain_db))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "non-delegated must be accepted"
    );
    let (session_user, bearer) = wait_capture(&cap3.captured).await;
    assert_eq!(
        session_user,
        Some(email.clone()),
        "non-delegated: session_user stays the caller email"
    );
    assert_eq!(bearer, None, "non-delegated: NO bearer is attached");
}

/// Impersonate mode: a DELEGATED connection whose `options.impersonate = true`
/// (bound to the global delegator secret) must run AS the caller via the
/// basic-auth + `X-Trino-User` connector path — NOT mint a per-user JWT. So the
/// submit path resolves `session_user = the caller's username` and `bearer =
/// None`, even though a working minter is wired (proving no mint happens).
#[tokio::test]
async fn submit_impersonate_runs_as_caller_no_mint() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");

    // A workspace + a member holding query:run (ws_editor, migration 0009 grants it).
    let email = format!("imp_{}@egret.local", Uuid::new_v4().simple());
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(&email)
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let slug = format!("imp-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Impersonate Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_editor' ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("add workspace member");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );

    // A working minter is wired on purpose: if the impersonate branch is correct,
    // it is NEVER called (bearer must stay None).
    let cap = Arc::new(CapturingFactory::default());
    let st = build_state(
        db.clone(),
        pdp,
        Arc::new(FakeMinter {
            token: Some("should-not-be-minted".into()),
        }),
        cap.clone(),
    );

    // The global (workspace_id NULL) delegator credential the impersonate row binds,
    // sealed with the same crypto `build_state`'s store uses so `open_secret` resolves.
    let secret_name = format!("egret-delegator-{}", Uuid::new_v4().simple());
    let secret_id = st
        .databases
        .workspace_secrets()
        .create(None, &secret_name, None, "egret_delegator:pw")
        .await
        .expect("create delegator secret");

    // An impersonate-mode delegated connection: delegated = true,
    // options.impersonate = true, bound to the delegator secret.
    let imp_db: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated, options, secret_id) \
         VALUES ($1,'trino','localhost',8080,$2,true,$3,$4) RETURNING id",
    )
    .bind(format!("imp-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .bind(serde_json::json!({"user": "egret_delegator", "impersonate": true}))
    .bind(secret_id)
    .fetch_one(&db.pool)
    .await
    .expect("insert impersonate db");

    let expected_username = egret_auth::users::UserRepo::new(db.clone())
        .ensure_username(egret_core::ids::UserId(uid))
        .await
        .expect("ensure_username");
    assert_ne!(
        expected_username, email,
        "sanity: username must differ from email so the assertion is meaningful"
    );

    let token = st
        .sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let resp = engine_router()
        .with_state(st)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ws/{slug}/queries"))
                .header("content-type", "application/json")
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::from(format!(
                    r#"{{"database_id":"{imp_db}","sql":"SELECT 1"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "impersonate-mode delegated query must be accepted"
    );

    let (session_user, bearer) = wait_capture(&cap.captured).await;
    assert_eq!(
        session_user,
        Some(expected_username.clone()),
        "impersonate: session_user MUST be the caller's username (runs AS the caller)"
    );
    assert_eq!(
        bearer, None,
        "impersonate: NO JWT is minted — the connector uses basic-auth + X-Trino-User"
    );
}

/// KI-092: `DELETE /ws/{ws}/queries` clears the CALLER's SQL Lab history and
/// nobody else's. A second member clearing their own history leaves the first
/// member's entries listed; a member without `query:run` is refused; the owner
/// clearing empties their list, and queries run afterwards are listed again.
#[tokio::test]
async fn clear_history_clears_only_the_callers_history() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");

    let creator: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("owner_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert creator");
    let slug = format!("query-history-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Query History Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(creator)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8080,$2,false) RETURNING id",
    )
    .bind(format!("h-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("insert db");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(
        db.clone(),
        pdp,
        Arc::new(NoMinter),
        Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
    );
    let sessions = st.sessions.clone();
    let (owner, owner_tok) = member_session_with_id(&db, &sessions, ws, "ws_editor").await;
    let (other, other_tok) = member_session_with_id(&db, &sessions, ws, "ws_editor").await;
    let viewer_tok = member_session(&db, &sessions, ws, "ws_viewer").await;

    let store = QueryStore::new(db.clone());
    for sql in ["SELECT 'owner 1'", "SELECT 'owner 2'"] {
        store
            .insert(ws, db_id, Some(owner), sql)
            .await
            .expect("owner query");
    }
    store
        .insert(ws, db_id, Some(other), "SELECT 'other'")
        .await
        .expect("other query");

    let app = engine_router().with_state(st);
    let call = |method: &'static str, tok: String| {
        let app = app.clone();
        let uri = format!("/ws/{slug}/queries");
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("Cookie", format!("egret_session={tok}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let sqls: Vec<String> = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes)
                .map(|v| {
                    v.iter()
                        .map(|q| q["sql"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            (status, sqls)
        }
    };

    // A member without query:run cannot clear anything.
    let (s, _) = call("DELETE", viewer_tok).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "ws_viewer must not clear history");

    // The other member clears THEIR history: the owner's entries stay.
    let (s, _) = call("DELETE", other_tok.clone()).await;
    assert_eq!(s, StatusCode::OK, "a member clears their own history");
    let (_, other_list) = call("GET", other_tok).await;
    assert!(
        other_list.is_empty(),
        "the caller's history is cleared: {other_list:?}"
    );
    let (_, owner_list) = call("GET", owner_tok.clone()).await;
    assert_eq!(
        owner_list.len(),
        2,
        "another member's clear must not touch the owner's history: {owner_list:?}"
    );

    // The owner clears: their list is empty; a later query is listed again.
    let (s, _) = call("DELETE", owner_tok.clone()).await;
    assert_eq!(s, StatusCode::OK);
    let (_, owner_list) = call("GET", owner_tok.clone()).await;
    assert!(
        owner_list.is_empty(),
        "owner's history is cleared: {owner_list:?}"
    );
    store
        .insert(ws, db_id, Some(owner), "SELECT 'after clear'")
        .await
        .expect("query after clear");
    let (_, owner_list) = call("GET", owner_tok).await;
    assert_eq!(owner_list, vec!["SELECT 'after clear'".to_string()]);

    // Clearing hides the entries from the history list only: the query rows
    // (status, results by id) stay.
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM query.queries WHERE workspace_id=$1")
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(kept, 4);
}
