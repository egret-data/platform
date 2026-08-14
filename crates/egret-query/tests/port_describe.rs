//! `describe_as` must resolve a real column schema through the port, not a stub.
//!
//! Engine + DB gated. Required:
//!   * `DATABASE_URL`          — the metastore the registry/user repos read.
//!   * `EGRET_TEST_TRINO_URL`  — a live coordinator, e.g. `https://localhost:8453`.
//!
//! Optional, and what the dev stack actually needs:
//!   * `EGRET_TEST_TRINO_TOKEN` — a per-user engine JWT. When set, the fixture
//!     registers a **delegated** (jwt-mode) connection and the stub minter hands
//!     this token back, which is exactly what the real `EngineTokenMinter` does.
//!     The dev coordinator runs `http-server.authentication.type=JWT`, so an
//!     unauthenticated request is refused (403) whatever the SQL — without a
//!     token this file cannot tell a parse error from a closed door. It also
//!     exercises the delegation branch the port exists for.
//!   * `EGRET_TEST_TRINO_USER` — the engine-visible identity (default `admin`).
//!     Must equal the token's `preferred_username`: with a bearer the connector
//!     also sends `X-Trino-User`, and a mismatch is an impersonation attempt.
//!     The fixture's local user is created with this as its `username`, which is
//!     what `delegation_for` resolves as the session user.
//!   * `EGRET_TEST_TRINO_CERT` — path to the coordinator's PEM, pinned as
//!     `ConnOptions.tls_cert` for an `https` URL.
//!
//! Over plain HTTP the connector sends `X-Forwarded-Proto: https` (the
//! coordinator runs `http-server.process-forwarded=true`), which is what lets
//! JWT auth work off the `:8090` port — but sending a bearer over a non-TLS
//! transport is fail-closed, so such a run also needs `EGRET_ENGINE_TLS_INSECURE=1`,
//! the documented operator opt-out.
//!
//! A self-skipped run proves nothing. See the task report for the live output.

use std::sync::Arc;

use async_trait::async_trait;
use egret_query::execution::model::Limits;
use egret_query::DatabaseRepo;
use egret_query::{EngineState, JobManager, QueryStore};
use egret_query_api::QueryExecPort;
use egret_storage::{ObjectStore, StorageError};
use uuid::Uuid;

/// The port only needs the registry + user repos; the object store is inert
/// here because `describe` never materializes a result set.
#[derive(Default)]
struct MemStore;

#[async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, _uri: &str, _bytes: Vec<u8>) -> Result<(), StorageError> {
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        Err(StorageError::NotFound(uri.into()))
    }
    async fn delete(&self, _uri: &str) -> Result<(), StorageError> {
        Ok(())
    }
    async fn copy(&self, src: &str, _dst: &str) -> Result<(), StorageError> {
        Err(StorageError::NotFound(src.into()))
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

/// `describe_as` never authorizes — the caller (validate-before-publish) does.
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

/// Stands in for the real broker: hands back the pre-minted engine JWT supplied
/// via `EGRET_TEST_TRINO_TOKEN`. `None` reproduces a non-OIDC caller, which the
/// delegation path must refuse rather than fall back to service credentials —
/// but the fixture only registers a delegated connection when a token exists, so
/// that arm is never reached here.
struct StubMinter(Option<String>);
#[async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for StubMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        match &self.0 {
            Some(t) => Ok(egret_analytics_api::engine_token::EngineToken {
                access_token: t.clone(),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            }),
            None => Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken),
        }
    }
}

/// `describe` builds its connector directly; the JobManager's factory is unused.
struct UnusedFactory;
#[async_trait]
impl egret_query::execution::connector::ConnectorFactory for UnusedFactory {
    async fn build(
        &self,
        _db: &egret_query::Database,
        _secret: Option<String>,
        _session_user: Option<String>,
        _bearer_token: Option<String>,
    ) -> Result<Box<dyn egret_query::execution::connector::Connector>, egret_query::EngineError>
    {
        Err(egret_query::EngineError::Connect("unused (test)".into()))
    }
}

struct Fixture {
    st: EngineState,
    ws: Uuid,
    database_id: Uuid,
    user: egret_core::UserId,
}

async fn trino_engine_state() -> Option<Fixture> {
    let trino = std::env::var("EGRET_TEST_TRINO_URL").ok()?;
    let url = std::env::var("DATABASE_URL").ok()?;
    let trino = url::Url::parse(&trino).expect("EGRET_TEST_TRINO_URL is a URL");

    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");

    // The local user's `username` IS the engine-visible identity: `delegation_for`
    // resolves it via `ensure_username`, and the connector sends it as
    // `X-Trino-User`. Seeded once and reused, so a re-run does not have to
    // allocate `tester-2` and desync from the token's `preferred_username`.
    let engine_user = std::env::var("EGRET_TEST_TRINO_USER").unwrap_or_else(|_| "admin".into());
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(&engine_user)
        .fetch_optional(&db.pool)
        .await
        .expect("lookup user");
    let uid: Uuid = match existing {
        Some(id) => id,
        None => sqlx::query_scalar(
            "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) RETURNING id",
        )
        .bind(format!("port-describe-{engine_user}@egret.local"))
        .bind(&engine_user)
        .fetch_one(&db.pool)
        .await
        .expect("insert user"),
    };
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Port Describe', $2, 'active') RETURNING id",
    )
    .bind(format!("port-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // A Trino registration pointed at the live coordinator. With a token it is a
    // DELEGATED jwt-mode connection (the dev coordinator enforces JWT auth); the
    // cert, when given, is pinned so the bearer travels over TLS.
    let token = std::env::var("EGRET_TEST_TRINO_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let cert = std::env::var("EGRET_TEST_TRINO_CERT")
        .ok()
        .map(|p| std::fs::read_to_string(&p).expect("read EGRET_TEST_TRINO_CERT"));
    let mut options = serde_json::json!({
        "user": engine_user,
        "http_scheme": trino.scheme(),
    });
    if let Some(pem) = &cert {
        options["tls_cert"] = serde_json::json!(pem);
    }
    let database_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, options, workspace_id, delegated) \
         VALUES ($1,'trino',$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(format!("t-{}", Uuid::new_v4().simple()))
    .bind(trino.host_str().expect("trino host"))
    .bind(trino.port().unwrap_or(8080) as i32)
    .bind(&options)
    .bind(ws)
    .bind(token.is_some())
    .fetch_one(&db.pool)
    .await
    .expect("insert trino database");

    let store = QueryStore::new(db.clone());
    let databases = DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore);
    let limits = Limits {
        max_rows: 100_000,
        timeout: std::time::Duration::from_secs(60),
    };
    let jobs = JobManager::new(
        store.clone(),
        databases.clone(),
        Arc::new(UnusedFactory),
        storage.clone(),
        "s3://warehouse".into(),
        "query-results".into(),
        limits.clone(),
        100,
    );
    let st = EngineState {
        store,
        jobs,
        databases,
        storage,
        bucket_uri: "s3://warehouse".into(),
        results_prefix: "query-results".into(),
        limits,
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: Arc::new(AllowAllPdp),
        users: egret_auth::users::UserRepo::new(db),
        engine_minter: Arc::new(StubMinter(token)),
    };

    Some(Fixture {
        st,
        ws,
        database_id,
        user: egret_core::UserId(uid),
    })
}

#[tokio::test]
async fn describe_as_returns_the_select_list() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;

    let cols = port
        .describe_as(f.ws, f.database_id, f.user, "SELECT 1 AS n, 'x' AS s")
        .await
        .expect("describe must succeed");

    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["n", "s"],
        "column names come from the SELECT list"
    );
    assert_eq!(cols.len(), 2, "no extra columns");
}

#[tokio::test]
async fn describe_as_rejects_invalid_sql() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;

    let err = port
        .describe_as(f.ws, f.database_id, f.user, "SELECT FROM WHERE")
        .await
        .expect_err("a syntax error must surface as an error, not an empty column list");
    // A 400 the endpoint author can act on, not a 500 that tells them nothing.
    assert!(
        matches!(err, egret_core::Error::InvalidInput(_)),
        "bad SQL must map to InvalidInput, got {err:?}"
    );
    // Guard against a false pass: a coordinator that rejects the CONNECTION also
    // produces an error here, and would let this test go green while the engine
    // never saw the SQL. The message must come from the parser.
    let msg = err.to_string();
    assert!(
        !msg.contains("forbidden") && !msg.contains("connect"),
        "the engine must have parsed the SQL, not refused the connection: {msg}"
    );
}
