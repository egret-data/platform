//! P4 Task 5 — what a caller sees when something goes wrong, and what an
//! operator can see afterwards.
//!
//! Two properties, and everything here serves one of them:
//!
//! * **Nothing from the engine reaches the body.** Trino and StarRocks errors
//!   carry table names, column names and the coordinator's host:port. A caller
//!   holding only a machine credential must not be able to learn any of it, so
//!   every failure is a fixed code and a fixed message plus a `request_id` they
//!   can quote.
//! * **The log survives what it describes.** `dataapi.access_log` carries no
//!   foreign key on `endpoint_id`, deliberately, so deleting an endpoint does
//!   not erase the record of what it was doing. That is asserted here rather
//!   than only asserted in a migration comment.
//!
//! The fake engine is driven by the SQL TEXT, not by shared mutable state, so
//! each failure is reproducible from the published endpoint alone. `describe_as`
//! always succeeds: every endpoint here publishes cleanly and then fails at CALL
//! time, which is the real sequence — a table is dropped, a grant is revoked, a
//! coordinator goes down, all after the snapshot was frozen.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::{ColumnMeta, Error, TabularResult, UserId};
use egret_dataapi::serving::AccessLogStore;
use egret_dataapi::{management_router, serving_router, ClientStore, DataApiState, EndpointStore};
use egret_machine_idp::{generate_rsa_key, sign_claims, IdpError, KeyMaterial, SigningKeyStore};
use egret_metastore::Db;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use uuid::Uuid;

/// The host the fake coordinator "lives" on. Asserted as ABSENT from every
/// body — it is the string a connect failure would carry out with it.
const ENGINE_HOST: &str = "trino-coordinator.internal:8080";

/// The relation a published endpoint names after someone dropped it. Asserted
/// as ABSENT from every body: a caller who cannot read a table must not learn
/// that it was ever named.
const MISSING_TABLE: &str = "hive.analytics.does_not_exist";

// ---------------------------------------------------------------------------
// An engine that fails the way real engines fail.
// ---------------------------------------------------------------------------

fn col(name: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        ty: "integer".into(),
    }
}

struct FakePort;

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for FakePort {
    /// Always succeeds: publish must be able to freeze a column list, because
    /// every case in this file is about an endpoint that published cleanly and
    /// broke later.
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![col("n")])
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        sql: &str,
        limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        // Exactly the three shapes `port_impl::engine_error` can hand the
        // serving path, spelled with the payloads a real engine attaches.
        if sql.contains("does_not_exist") {
            return Err(Error::NotFound(MISSING_TABLE.into()));
        }
        if sql.contains("unreachable_engine") {
            return Err(Error::Internal(anyhow::anyhow!(
                "query port: connect {ENGINE_HOST}: connection refused"
            )));
        }
        if sql.contains("denied_table") {
            // A Ranger deny. The SQL is perfect; the grant is missing.
            return Err(Error::Forbidden);
        }

        // Otherwise: `rows_wanted` rows, capped the way a real connector caps —
        // it fetches `max_rows + 1`, keeps `max_rows`, and reports `truncated`.
        // `truncated` is therefore the ONLY signal that the cap was hit, which
        // is what the serving path must key on: a `row_count` equal to the cap
        // cannot distinguish "exactly at" from "over".
        let rows_wanted: usize = if sql.contains("two_rows") { 2 } else { 1 };
        let kept = rows_wanted.min(limits.max_rows);
        Ok(TabularResult {
            columns: vec![col("n")],
            rows: (0..kept)
                .map(|i| vec![serde_json::json!(i as i64)])
                .collect(),
            row_count: kept as u64,
            bytes_scanned: None,
            truncated: rows_wanted > limits.max_rows,
        })
    }
}

// ---------------------------------------------------------------------------
// The IdP key, and tokens minted against it.
// ---------------------------------------------------------------------------

fn active_key() -> &'static KeyMaterial {
    static KEY: OnceLock<KeyMaterial> = OnceLock::new();
    KEY.get_or_init(|| generate_rsa_key("egret-active").expect("generate key"))
}

struct FakeKeys;

#[async_trait::async_trait]
impl SigningKeyStore for FakeKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        Ok(vec![active_key().public_jwk.clone()])
    }
}

fn mint(ws: Uuid, user: Uuid, client_id: &str) -> String {
    sign_claims(
        active_key(),
        &serde_json::json!({
            "iss": "https://egret.test",
            "sub": client_id,
            "aud": "data-api",
            "exp": chrono::Utc::now().timestamp() + 600,
            "ws": ws.to_string(),
            "egret_user_id": user.to_string(),
        }),
    )
    .expect("sign")
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fixture {
    db: Db,
    serving: axum::Router,
    mgmt: axum::Router,
    session: String,
    ws_slug: String,
    ws_id: Uuid,
    sa_user: Uuid,
    admin: Uuid,
    database_id: Uuid,
    engine_host: String,
}

async fn insert_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn grant(db: &Db, user: Uuid, ws: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_t5_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert role");
    for p in perms {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&db.pool)
            .await
            .expect("grant perm");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant role");
}

/// Give `user` `data_api_endpoint:invoke` on ONE endpoint — spec §6.2 step 4's
/// grant, written as one `access_grants` row against the `data_api_invoker` role
/// migration 0124 seeds.
///
/// Every fixture that publishes an endpoint and then expects the call to reach
/// the engine needs this now: without a grant the serving path answers 403 at
/// step 4, before anything this file is about. Issued in SQL rather than through
/// `POST …/endpoints/{id}/grants` because this suite's serving principal is a
/// plain `users` row with no `dataapi.service_accounts` binding, which that
/// route (correctly) refuses. The ROUTE is exercised in `serving_authz.rs`.
async fn grant_invoke(db: &Db, user: Uuid, endpoint: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect) \
         SELECT 'user', $1, r.id, 'object', 'data_api_endpoint', $2, 'allow' \
           FROM roles r WHERE r.scope = 'workspace' AND r.name = 'data_api_invoker' \
         ON CONFLICT DO NOTHING",
    )
    .bind(user)
    .bind(endpoint)
    .execute(&db.pool)
    .await
    .expect("grant invoke");
}

async fn fixture() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let admin = insert_user(&db, "t5_admin").await;
    let ws_slug = format!("t5-{}", Uuid::new_v4().simple());
    let ws_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Data API Task 5', $2, 'active') RETURNING id",
    )
    .bind(&ws_slug)
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let sa_user = insert_user(&db, "t5_sa").await;
    let database_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, delegated, created_by) \
         VALUES ($1, $2, 'trino', $3, 8080, false, $4) RETURNING id",
    )
    .bind(ws_id)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ENGINE_HOST.split(':').next().unwrap())
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("insert database");

    let author = insert_user(&db, "t5_author").await;
    grant(
        &db,
        author,
        ws_id,
        &["data_api_endpoint:read", "data_api_endpoint:manage"],
    )
    .await;

    // The REAL PDP, built after the grant (`CedarPdp` snapshots at build time).
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = DataApiState {
        db: db.clone(),
        clients: ClientStore::new(db.clone()),
        endpoints: EndpointStore::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        query: Arc::new(FakePort),
        keys: Arc::new(FakeKeys),
        issuer: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: egret_dataapi::serving::ResponseCache::new(),
        rate_limits: Arc::new(egret_dataapi::serving::TokenBucket::new()),
    };

    let session = st
        .sessions
        .create(egret_core::ids::UserId(author), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    Some(Fixture {
        db,
        serving: axum::Router::new()
            .nest("/data-api/v1", serving_router())
            .with_state(st.clone()),
        mgmt: axum::Router::new()
            .nest("/api", management_router())
            .with_state(st),
        session,
        ws_slug,
        ws_id,
        sa_user,
        admin,
        database_id,
        engine_host: ENGINE_HOST.to_string(),
    })
}

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

struct Res {
    status: StatusCode,
    body: serde_json::Value,
    text: String,
    retry_after: Option<String>,
}

impl Res {
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_else(|| {
            panic!("no error.code in {}", self.text);
        })
    }
    fn request_id(&self) -> &str {
        self.body["error"]["request_id"]
            .as_str()
            .unwrap_or_else(|| panic!("no error.request_id in {}", self.text))
    }
}

/// One access-log row, read back.
#[derive(Debug, sqlx::FromRow)]
struct LogRow {
    workspace_id: Uuid,
    endpoint_id: Option<Uuid>,
    version: Option<i32>,
    service_account_id: Option<Uuid>,
    client_id: Option<String>,
    status_code: i32,
    row_count: Option<i64>,
    duration_ms: i32,
    cache_hit: bool,
    error_code: Option<String>,
    request_id: String,
}

impl Fixture {
    async fn call(&self, path: &str) -> Res {
        self.call_as_client(self.sa_user, "client-a", path).await
    }

    async fn call_as(&self, principal: Uuid, path: &str) -> Res {
        self.call_as_client(principal, "client-a", path).await
    }

    async fn call_as_client(&self, principal: Uuid, client_id: &str, path: &str) -> Res {
        let token = mint(self.ws_id, principal, client_id);
        self.send(path, Some(&token)).await
    }

    async fn call_without_token(&self, path: &str) -> Res {
        self.send(path, None).await
    }

    /// A raw request at an arbitrary method and URI, for the two answers axum
    /// gives BEFORE `serve` is reached.
    async fn raw(&self, method: &str, uri: &str) -> Res {
        let token = mint(self.ws_id, self.sa_user, "client-a");
        let res = self
            .serving
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Res {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            text,
            retry_after: None,
        }
    }

    async fn send(&self, path: &str, token: Option<&str>) -> Res {
        let uri = format!("/data-api/v1/{}/{path}", self.ws_slug);
        let mut b = Request::builder().method("GET").uri(&uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let res = self
            .serving
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let retry_after = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Res {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            text,
            retry_after,
        }
    }

    async fn mgmt_send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("Cookie", format!("egret_session={}", self.session));
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let res = self
            .mgmt
            .clone()
            .oneshot(
                b.body(
                    body.map(|v| Body::from(v.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn publish_endpoint(&self, method: &str, path: &str, sql: &str) -> Uuid {
        self.publish_full(
            method,
            path,
            sql,
            serde_json::json!({}),
            serde_json::json!({}),
        )
        .await
    }

    async fn publish_endpoint_with_max_rows(
        &self,
        method: &str,
        path: &str,
        sql: &str,
        max_rows: u32,
    ) -> Uuid {
        self.publish_full(
            method,
            path,
            sql,
            serde_json::json!({ "max_rows": max_rows }),
            serde_json::json!({}),
        )
        .await
    }

    async fn publish_full(
        &self,
        method: &str,
        path: &str,
        sql: &str,
        limits: serde_json::Value,
        cache: serde_json::Value,
    ) -> Uuid {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, sql, limits, cache, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(method)
        .bind(self.database_id)
        .bind(sql)
        .bind(&limits)
        .bind(&cache)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft");

        let (status, body) = self
            .mgmt_send(
                "POST",
                &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", self.ws_slug),
                Some(serde_json::json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "publish failed: {body}");
        grant_invoke(&self.db, self.sa_user, ep).await;
        ep
    }

    async fn log_for(&self, request_id: &str) -> Option<LogRow> {
        sqlx::query_as("SELECT * FROM dataapi.access_log WHERE request_id = $1")
            .bind(request_id)
            .fetch_optional(&self.db.pool)
            .await
            .expect("read access log")
    }

    async fn log_count_for_ws(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE workspace_id = $1")
            .bind(self.ws_id)
            .fetch_one(&self.db.pool)
            .await
            .expect("count access log")
    }
}

// ---------------------------------------------------------------------------
// The leak, which is the whole point
// ---------------------------------------------------------------------------

/// The endpoint published fine and the relation was dropped afterwards. The
/// engine's message names it; the caller must not learn it.
#[tokio::test]
async fn an_engine_error_never_reaches_the_caller_verbatim() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/gone", "SELECT * FROM does_not_exist")
        .await;

    let res = f.call("gone").await;

    assert!(
        !res.text.contains("does_not_exist"),
        "leaked a table name: {}",
        res.text
    );
    assert!(
        !res.text.contains(&f.engine_host),
        "leaked the engine host: {}",
        res.text
    );
    assert!(
        res.text.contains("request_id"),
        "must be correlatable: {}",
        res.text
    );
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(res.code(), "internal");
    assert_eq!(res.body["error"]["message"], "internal error");
}

/// The same rule for the other payload an engine attaches: where it lives.
#[tokio::test]
async fn a_connect_failure_never_reaches_the_caller_verbatim() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/down", "SELECT n FROM unreachable_engine")
        .await;

    let res = f.call("down").await;

    assert!(
        !res.text.contains("trino-coordinator"),
        "leaked the coordinator host: {}",
        res.text
    );
    assert!(
        !res.text.contains("connection refused"),
        "leaked the transport detail: {}",
        res.text
    );
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(res.code(), "internal");
}

/// The author's SQL is fine; their policy denies the table. Telling them their
/// input is invalid sends them to debug the wrong thing.
#[tokio::test]
async fn a_ranger_denial_is_not_reported_as_bad_sql() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/denied", "SELECT n FROM denied_table")
        .await;

    let res = f.call("denied").await;

    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.text);
    assert_eq!(res.code(), "data_access_denied");
    assert!(
        !res.code().contains("param"),
        "a policy denial must never read as bad input: {}",
        res.text
    );
}

// ---------------------------------------------------------------------------
// The row cap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exceeding_the_row_cap_is_an_error_not_a_truncated_200() {
    let f = skip_without_db!();
    f.publish_endpoint_with_max_rows("GET", "/capped", "SELECT n FROM two_rows", 1)
        .await;

    let res = f.call("capped").await;

    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.text);
    assert_eq!(res.code(), "row_limit_exceeded");
    assert_eq!(
        res.body["error"]["message"], "result exceeded the 1-row cap",
        "the cap is the endpoint author's published number and spec §7 asks for it"
    );
}

/// The refusal is driven by `truncated`, NOT by `row_count == cap`.
///
/// This is the assertion the previous one cannot make: a result that lands
/// EXACTLY on the cap is complete, and a check written as `row_count >= cap`
/// would refuse it. Both tests pass with the correct implementation; only this
/// one goes red if the predicate is rewritten as a comparison.
#[tokio::test]
async fn a_result_exactly_at_the_cap_is_served_not_refused() {
    let f = skip_without_db!();
    f.publish_endpoint_with_max_rows("GET", "/exact", "SELECT n FROM two_rows", 2)
        .await;

    let res = f.call("exact").await;

    assert_eq!(
        res.status,
        StatusCode::OK,
        "a complete result that happens to fill the cap is not truncated: {}",
        res.text
    );
    assert_eq!(res.body["meta"]["row_count"], 2);
}

// ---------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------

/// Every error, whatever it is, is the same three fields — so a client can
/// branch on `code` without knowing which layer refused it.
#[tokio::test]
async fn every_error_carries_the_same_envelope() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/denied", "SELECT n FROM denied_table")
        .await;

    let cases = [
        (f.call_without_token("denied").await, 401, "key_invalid"),
        (f.call("nope").await, 404, "not_found"),
        (f.call("denied").await, 403, "data_access_denied"),
    ];
    for (res, status, code) in cases {
        assert_eq!(res.status.as_u16(), status, "{}", res.text);
        assert_eq!(res.code(), code, "{}", res.text);
        assert!(
            res.body["error"]["message"].is_string(),
            "no message in {}",
            res.text
        );
        assert!(
            Uuid::parse_str(res.request_id()).is_ok(),
            "request_id must be a real id, got {}",
            res.request_id()
        );
    }
}

/// The two answers that are NOT the envelope, pinned so the exception is a
/// known one rather than a discovery.
///
/// Spec §7 promises "one stable, machine-readable envelope". axum answers both
/// of these before `serve` is reached — a method outside `endpoints.method`'s
/// CHECK pair, and a URI that does not match `/{ws}/{*path}` at all — so no
/// code of ours runs and there is nothing to put an envelope around. Both are
/// leak-free (empty body, no detail), and both are now stated in §7 rather than
/// only in a Rust comment.
///
/// If this ever needs to change, the fix is a router fallback, not a handler.
#[tokio::test]
async fn axum_answers_before_the_handler_and_those_two_answers_carry_no_envelope() {
    let f = skip_without_db!();

    // A method the route does not declare. Same answer for every path under
    // this router, existing or not, so it leaks nothing.
    let res = f
        .raw("DELETE", &format!("/data-api/v1/{}/anything", f.ws_slug))
        .await;
    assert_eq!(res.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(
        res.text.is_empty(),
        "405 must stay empty rather than grow a body with detail in it: {}",
        res.text
    );

    // A single-segment URI: `{*path}` needs at least one segment, so this
    // matches no route.
    let res = f.raw("GET", &format!("/data-api/v1/{}", f.ws_slug)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert!(
        res.text.is_empty(),
        "an unmatched URI is axum's 404, not ours: {}",
        res.text
    );
}

/// The three 503s state a floor so that a fleet of clients does not default to
/// a one-second backoff against a state only a human clears.
///
/// Reached through `cache_scope_stale`, which is the one of the three that a
/// test can create without a drifting engine: publish a `shared` cache against
/// a non-delegated connection, then flip the connection to delegated.
#[tokio::test]
async fn a_config_drift_503_states_how_long_to_wait() {
    let f = skip_without_db!();
    f.publish_full(
        "GET",
        "/stale",
        "SELECT 1 AS n",
        serde_json::json!({}),
        serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "shared" }),
    )
    .await;
    sqlx::query("UPDATE query.databases SET delegated = true WHERE id = $1")
        .bind(f.database_id)
        .execute(&f.db.pool)
        .await
        .expect("flip delegated");

    let res = f.call("stale").await;

    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE, "{}", res.text);
    assert_eq!(res.code(), "cache_scope_stale");
    assert_eq!(
        res.retry_after.as_deref(),
        Some("300"),
        "a bare 503 leaves the interval to the client's default, which is about \
         a second — on a condition only a re-publish clears"
    );
}

// ---------------------------------------------------------------------------
// The access log
// ---------------------------------------------------------------------------

/// The success row, and the correlation that makes the whole thing useful: the
/// id in the caller's body is the id in the table.
#[tokio::test]
async fn a_served_call_is_recorded_against_the_id_the_caller_holds() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/ok", "SELECT 1 AS n").await;

    let res = f.call("ok").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text);
    let request_id = res.body["meta"]["request_id"].as_str().unwrap().to_string();

    let row = f
        .log_for(&request_id)
        .await
        .unwrap_or_else(|| panic!("no access_log row for {request_id}"));
    assert_eq!(row.workspace_id, f.ws_id);
    assert_eq!(row.endpoint_id, Some(ep));
    assert_eq!(row.version, Some(1));
    assert_eq!(row.service_account_id, Some(f.sa_user));
    assert_eq!(row.client_id.as_deref(), Some("client-a"));
    assert_eq!(row.status_code, 200);
    assert_eq!(row.row_count, Some(1));
    assert!(!row.cache_hit);
    assert_eq!(row.error_code, None, "a 200 has no error code");
    assert_eq!(row.request_id, request_id);
    assert!(row.duration_ms >= 0);
}

/// A failure records the SAME code the caller was given. That is what lets
/// support answer "I got `data_access_denied` at 14:02" without asking them to
/// reproduce anything.
#[tokio::test]
async fn a_failed_call_records_the_code_the_caller_was_given() {
    let f = skip_without_db!();
    let ep = f
        .publish_endpoint("GET", "/denied", "SELECT n FROM denied_table")
        .await;

    let res = f.call("denied").await;
    let row = f
        .log_for(res.request_id())
        .await
        .unwrap_or_else(|| panic!("no access_log row for {}", res.request_id()));

    assert_eq!(row.status_code, 403);
    assert_eq!(row.error_code.as_deref(), Some("data_access_denied"));
    assert_eq!(row.error_code.as_deref(), Some(res.code()));
    assert_eq!(row.endpoint_id, Some(ep));
    assert_eq!(row.row_count, None, "nothing was returned");
}

/// A 404 still names the workspace and the caller. Without it, "someone is
/// probing this workspace for endpoints" is invisible.
#[tokio::test]
async fn a_404_is_recorded_with_the_caller_but_no_endpoint() {
    let f = skip_without_db!();

    let res = f.call("no/such/thing").await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);

    let row = f
        .log_for(res.request_id())
        .await
        .unwrap_or_else(|| panic!("no access_log row for {}", res.request_id()));
    assert_eq!(row.status_code, 404);
    assert_eq!(row.error_code.as_deref(), Some("not_found"));
    assert_eq!(row.endpoint_id, None, "no endpoint resolved");
    assert_eq!(row.version, None);
    assert_eq!(row.service_account_id, Some(f.sa_user));
}

/// A call refused before the token verifies has no workspace to be recorded
/// against — `workspace_id` is `NOT NULL` and inventing one would create a
/// tenant bucket no operator owns. Pinned so the gap is a known one.
#[tokio::test]
async fn a_call_refused_before_authentication_is_not_recorded() {
    let f = skip_without_db!();
    let before = f.log_count_for_ws().await;

    let res = f.call_without_token("anything").await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);

    assert_eq!(
        f.log_count_for_ws().await,
        before,
        "a 401 cannot name a workspace, so it goes to tracing only"
    );
    assert!(
        f.log_for(res.request_id()).await.is_none(),
        "and the id in the body has no row"
    );
}

/// A cache hit is recorded as one, and as its own row. Two calls, two rows —
/// otherwise "cache hit rate per endpoint" is computed from a denominator that
/// silently drops the hits.
#[tokio::test]
async fn a_cache_hit_is_its_own_row_and_says_so() {
    let f = skip_without_db!();
    f.publish_full(
        "GET",
        "/cached",
        "SELECT 1 AS n",
        serde_json::json!({}),
        serde_json::json!({ "enabled": true, "ttl_seconds": 60 }),
    )
    .await;

    let miss = f.call("cached").await;
    assert_eq!(miss.status, StatusCode::OK, "{}", miss.text);
    let hit = f.call("cached").await;
    assert_eq!(hit.body["meta"]["cached"], true, "{}", hit.text);

    let miss_id = miss.body["meta"]["request_id"].as_str().unwrap();
    let hit_id = hit.body["meta"]["request_id"].as_str().unwrap();
    assert_ne!(miss_id, hit_id, "two calls, two ids");

    assert!(!f.log_for(miss_id).await.unwrap().cache_hit);
    let hit_row = f.log_for(hit_id).await.unwrap();
    assert!(
        hit_row.cache_hit,
        "the second call was served from the cache"
    );
    assert_eq!(hit_row.status_code, 200);
    assert_eq!(hit_row.row_count, Some(1));
}

/// The reason `endpoint_id` carries no foreign key. This is the assertion the
/// migration comment claims: an access log must survive the deletion of what it
/// describes, which is the opposite of `0121`'s choice for
/// `endpoints.database_id` and for the opposite reason.
///
/// With a `CASCADE` this test loses the row; with a `RESTRICT` the delete
/// itself fails. It goes red either way.
#[tokio::test]
async fn the_access_log_outlives_the_endpoint_it_describes() {
    let f = skip_without_db!();
    let ep = f.publish_endpoint("GET", "/doomed", "SELECT 1 AS n").await;
    let res = f.call("doomed").await;
    let request_id = res.body["meta"]["request_id"].as_str().unwrap().to_string();
    assert!(f.log_for(&request_id).await.is_some(), "precondition");

    // The endpoint and its versions go away entirely.
    sqlx::query("UPDATE dataapi.endpoints SET live_version_id = NULL WHERE id = $1")
        .bind(ep)
        .execute(&f.db.pool)
        .await
        .expect("clear live version");
    sqlx::query("DELETE FROM dataapi.endpoints WHERE id = $1")
        .bind(ep)
        .execute(&f.db.pool)
        .await
        .expect("delete endpoint");

    let row = f
        .log_for(&request_id)
        .await
        .expect("the access log must survive the endpoint it describes");
    assert_eq!(
        row.endpoint_id,
        Some(ep),
        "the id stays as a historical label, dangling and correct"
    );
}

/// Retention, the same shape as `AuditStore::prune`.
#[tokio::test]
async fn prune_deletes_only_what_is_past_its_retention() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/keep", "SELECT 1 AS n").await;
    let fresh = f.call("keep").await;
    let fresh_id = fresh.body["meta"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();

    // One row backdated past the retention window. Written directly because
    // there is no way to make a request happen 40 days ago.
    let old_id = format!("old-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO dataapi.access_log \
           (ts, workspace_id, status_code, duration_ms, request_id) \
         VALUES (now() - interval '40 days', $1, 200, 5, $2)",
    )
    .bind(f.ws_id)
    .bind(&old_id)
    .execute(&f.db.pool)
    .await
    .expect("insert aged row");

    let store = AccessLogStore::new(f.db.clone());
    let deleted = store.prune(30).await.expect("prune");

    assert!(
        deleted >= 1,
        "the 40-day-old row must go, deleted={deleted}"
    );
    assert!(
        f.log_for(&old_id).await.is_none(),
        "the aged row survived a 30-day retention"
    );
    assert!(
        f.log_for(&fresh_id).await.is_some(),
        "prune took a row inside the retention window"
    );

    // NOT asserted here: that `prune(0)` / `prune(-1)` keep nothing. They do —
    // `ts < now() - 0 days` is every row already written, and `max(0)` clamps a
    // negative to the same thing instead of letting it invert. But `prune` is
    // deliberately GLOBAL (it is a retention job, not a per-workspace one), so
    // asserting it would empty the table under the tests running beside this one
    // in the same binary, and the resulting flake would look like a serving bug.
    // The semantics are documented on `AccessLogStore::prune`, and they are
    // `AuditStore::prune`'s, pinned by `egret-audit/tests/audit_store.rs:38`.
}

/// `call_as` exists for the shape of the brief's denial test; kept exercised so
/// it is not dead code that rots.
#[tokio::test]
async fn a_second_principal_is_refused_the_same_way() {
    let f = skip_without_db!();
    let ep = f
        .publish_endpoint("GET", "/denied", "SELECT n FROM denied_table")
        .await;
    let other = insert_user(&f.db, "t5_sa_other").await;
    // The second principal needs its OWN invoke grant, and the assertion below
    // is why it matters that it has one: without it the answer would still be a
    // 403, but `not_granted` rather than `data_access_denied` — egret refusing
    // the call instead of the engine refusing the data. Two different systems'
    // problems behind one status code, which is exactly what the two codes
    // exist to separate.
    grant_invoke(&f.db, other, ep).await;

    let res = f.call_as(other, "denied").await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.text);
    assert_eq!(
        res.code(),
        "data_access_denied",
        "the ENGINE refused the data; egret let the call through: {}",
        res.text
    );
}
