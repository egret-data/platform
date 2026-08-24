//! `describe_as` must resolve a real column schema through the port, not a stub,
//! and `execute_as` must return real rows through it under the same scoping rule.
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
    /// A live connection owned by a DIFFERENT workspace, for the scoping test.
    foreign_database_id: Uuid,
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
    // Not a check-then-insert. The eight tests in this binary run in parallel and
    // all seed the SAME `engine_user`, so a `SELECT` followed by an `INSERT`
    // races on the FIRST run for any new `EGRET_TEST_TRINO_USER`, failing a
    // different test each run — which is what a race looks like from the
    // outside. Reproduced deterministically before it was fixed: delete the row,
    // re-run the binary, and the failure returns.
    //
    // `ON CONFLICT DO NOTHING` with NO conflict target, which is the part that
    // took two measurements to get right. `users` is UNIQUE on `email` AND on
    // `username`, and a targeted `ON CONFLICT (col)` suppresses a violation of
    // THAT index only — so each single-column arbiter fixes one failure and
    // reintroduces the other, measured both ways on a live database:
    //
    //   * `ON CONFLICT (email)` — a pre-existing row holding the username under
    //     a different email raises `users_username_key` and all eight tests
    //     panic at seed time with a message naming email. `egret_test` already
    //     contains such a row for the DEFAULT `EGRET_TEST_TRINO_USER=admin`,
    //     since `ensure_username` derives `admin` from `admin@…`'s local part.
    //   * `ON CONFLICT (username)` — two concurrent inserts of the identical row
    //     raise `users_email_key`, because Postgres reports whichever index it
    //     happens to check first. That is the cold-start race this fix exists
    //     for, back again.
    //
    // Untargeted `DO NOTHING` covers every unique index, which is the only form
    // that is correct under both. It returns no row when it fires, so the
    // fallback `SELECT` reads the row that blocked it — matched on EITHER key,
    // since either is what blocked it. That `SELECT` cannot come up empty: the
    // insert only did nothing because a conflicting row exists.
    let email = format!("port-describe-{engine_user}@egret.local");
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) \
         ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(&email)
    .bind(&engine_user)
    .fetch_optional(&db.pool)
    .await
    .expect("claim the engine user");
    let uid: Uuid = match claimed {
        Some(id) => id,
        None => {
            sqlx::query_scalar("SELECT id FROM users WHERE username = $1 OR email = $2 LIMIT 1")
                .bind(&engine_user)
                .bind(&email)
                .fetch_one(&db.pool)
                .await
                .expect("a conflicting row must exist — that is why the insert did nothing")
        }
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

    // A SECOND workspace owning an otherwise-identical connection. `describe_as`
    // must refuse this id when asked for it under `ws` — it exists, it is
    // reachable, it is simply not this workspace's.
    let other_ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Port Describe Other', $2, 'active') RETURNING id",
    )
    .bind(format!("port-other-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("insert other workspace");
    let foreign_database_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, options, workspace_id, delegated) \
         VALUES ($1,'trino',$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(format!("other-{}", Uuid::new_v4().simple()))
    .bind(trino.host_str().expect("trino host"))
    .bind(trino.port().unwrap_or(8080) as i32)
    .bind(&options)
    .bind(other_ws)
    .bind(token.is_some())
    .fetch_one(&db.pool)
    .await
    .expect("insert foreign trino database");

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
        foreign_database_id,
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

/// The port scopes `database_id` to `ws` itself, so a caller that forgets its own
/// check is still safe. `database_id` reaches it from a request body — an
/// unscoped describe would let an endpoint author aim at another workspace's
/// connection and read its schema.
///
/// `NotFound`, not `Forbidden`: absence and denial must be indistinguishable, or
/// the call becomes an oracle for which connections exist elsewhere.
#[tokio::test]
async fn describe_as_refuses_a_database_from_another_workspace() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;

    // The SQL is perfectly valid and the connection is live — the ONLY reason to
    // fail is that this database belongs to a different workspace.
    let err = port
        .describe_as(
            f.ws,
            f.foreign_database_id,
            f.user,
            "SELECT 1 AS n, 'x' AS s",
        )
        .await
        .expect_err("a database outside the workspace must not be describable");
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "a foreign database must be NotFound (not Forbidden, which would confirm it exists), got {err:?}"
    );

    // Control: the same call under the OWNING workspace succeeds, so the refusal
    // above is the scope check and not a broken fixture.
    let cols = port
        .describe_as(f.ws, f.database_id, f.user, "SELECT 1 AS n, 'x' AS s")
        .await
        .expect("the workspace's own database still describes");
    assert_eq!(cols.len(), 2);
}

/// A bounded serving limit: the Data API's per-endpoint cap, not the job runner's.
fn small_limits() -> Limits {
    Limits {
        max_rows: 100,
        timeout: std::time::Duration::from_secs(30),
    }
}

/// `execute_as` is the serving half of the port: real rows off a live
/// coordinator, under the caller's identity, and carrying the SAME scoping
/// guarantee as `describe_as` — a `database_id` outside `ws` is `NotFound`
/// before the principal is resolved and before any network call.
#[tokio::test]
async fn execute_as_returns_rows_and_scopes_to_the_workspace() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;

    let out = port
        .execute_as(f.ws, f.database_id, f.user, "SELECT 1 AS n", small_limits())
        .await
        .expect("execute must succeed");
    assert_eq!(out.rows.len(), 1, "one row from a one-row SELECT");
    assert_eq!(
        out.columns[0].name, "n",
        "column names come from the SELECT"
    );
    assert_eq!(
        out.rows[0][0],
        serde_json::json!(1),
        "the actual cell value"
    );
    assert!(
        !out.truncated,
        "one row under a cap of 100 is not truncated"
    );

    // Same scoping guarantee describe_as carries: a database from another
    // workspace is NotFound, checked before any network call. The SQL is valid
    // and the connection is live, so the ONLY reason to fail is the scope.
    let err = port
        .execute_as(
            f.ws,
            f.foreign_database_id,
            f.user,
            "SELECT 1",
            small_limits(),
        )
        .await
        .expect_err("a foreign database must not execute");
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "a foreign database must be NotFound (not Forbidden, which would confirm it exists), got {err:?}"
    );
}

/// The cap must be visible to the caller. A silent truncation would let a Data
/// API response look like the complete answer when it is a prefix of one.
#[tokio::test]
async fn execute_as_reports_truncation_rather_than_silently_capping() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;
    let limits = Limits {
        max_rows: 2,
        timeout: std::time::Duration::from_secs(30),
    };

    let out = port
        .execute_as(
            f.ws,
            f.database_id,
            f.user,
            "SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3",
            limits,
        )
        .await
        .expect("execute must succeed");
    assert!(
        out.truncated,
        "the caller must be able to tell it did not get everything"
    );
    assert_eq!(out.rows.len(), 2, "capped at max_rows");
}

/// The error variants must not be flattened on the serving path either: bad SQL
/// stays a 400 the endpoint author can act on, and must come from the engine's
/// parser rather than a refused connection.
#[tokio::test]
async fn execute_as_rejects_invalid_sql_as_invalid_input() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;

    let err = port
        .execute_as(
            f.ws,
            f.database_id,
            f.user,
            "SELECT FROM WHERE",
            small_limits(),
        )
        .await
        .expect_err("a syntax error must surface as an error, not an empty result");
    assert!(
        matches!(err, egret_core::Error::InvalidInput(_)),
        "bad SQL must map to InvalidInput, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        !msg.contains("forbidden") && !msg.contains("connect"),
        "the engine must have parsed the SQL, not refused the connection: {msg}"
    );
}

/// `limits.timeout` is a real budget, not decoration.
///
/// The serving path has no JobManager, so `execute_as` must impose the bound
/// `run_query` imposes for the async path. `Connector::execute` does NOT do it:
/// StarRocks never reads `limits.timeout` and sets no mysql_async read/wait
/// timeout, so an unwrapped serving request hangs until the connection dies;
/// Trino turns it into a PER-REQUEST reqwest timeout whose clock restarts on
/// every `nextUri` poll, so total wall-clock is unbounded. This test runs on
/// Trino and pins the wrap that fixes both.
///
/// Why the assertions are what they are:
///
/// * The budget is **sub-second on purpose**. The Trino connector also sets
///   `query_max_run_time = limits.timeout.as_secs().max(1)`, i.e. the engine's
///   own kill switch is `floor(budget)` seconds — for any budget >= 1s that
///   fires *before* the wrap and the test would pass without it. Under 1s the
///   session property clamps to 1s and the wrap is strictly first.
/// * The exact message is asserted, not just the variant. `"query timed out"`
///   is reachable only through `EngineError::Timeout`, and `TrinoConnector`
///   never returns that variant — so this string is the wrap's fingerprint.
///   Trino's own `EXCEEDED_TIME_LIMIT` arrives as `Engine(..)` and a reqwest
///   per-request timeout as `Connect(..)`; both are excluded by it.
/// * The elapsed bound is the "rather than hanging" half — the property that
///   actually fails open on StarRocks.
#[tokio::test]
async fn execute_as_enforces_the_timeout_budget_rather_than_hanging() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;
    let budget = std::time::Duration::from_millis(750);
    let limits = Limits {
        max_rows: 10,
        timeout: budget,
    };

    // Six billion rows through the tpch generator: it cannot finish inside the
    // budget, and every individual `nextUri` poll returns promptly — which is
    // exactly the shape the per-request timeout fails to bound.
    let started = std::time::Instant::now();
    let err = port
        .execute_as(
            f.ws,
            f.database_id,
            f.user,
            "SELECT count(*) FROM tpch.sf1000.lineitem",
            limits,
        )
        .await
        .expect_err("a query past its budget must return, not run to completion");
    let elapsed = started.elapsed();

    assert!(
        matches!(err, egret_core::Error::InvalidInput(_)),
        "an over-budget query is the author's problem (400), got {err:?}"
    );
    assert!(
        err.to_string().contains("query timed out"),
        "the budget must be enforced by execute_as's own wrap, not by the \
         engine's session limit or a per-request socket timeout: {err}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "execute_as must return near its budget, not hang: took {elapsed:?}"
    );
}

/// The scope check must run **first**, not merely run.
///
/// `execute_as_returns_rows_and_scopes_to_the_workspace` proves the check
/// happens; it cannot prove the ordering, because a foreign id yields `NotFound`
/// under either order. This does: with a principal that does not exist, the
/// scope-check-first ordering still reports `NotFound`, whereas resolving the
/// principal first would report `Internal("principal not found")` and thereby
/// tell the caller the database itself was acceptable — an oracle, and a lookup
/// spent on an id that should have been refused for free.
#[tokio::test]
async fn execute_as_scopes_the_database_before_resolving_the_principal() {
    let Some(f) = trino_engine_state().await else {
        eprintln!("skip");
        return;
    };
    let port: &dyn QueryExecPort = &f.st;
    let ghost = egret_core::UserId(Uuid::new_v4());

    let err = port
        .execute_as(
            f.ws,
            f.foreign_database_id,
            ghost,
            "SELECT 1",
            small_limits(),
        )
        .await
        .expect_err("a foreign database must not execute");
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "the workspace scope check must run before the principal lookup, or an \
         unknown principal turns the refusal into Internal and leaks that the \
         database itself was acceptable: got {err:?}"
    );
}
