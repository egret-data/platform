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
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no `DATABASE_URL` FAILS, so check the
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

/// Only for Task 8's test below, which needs `FakePort::fail_next_with` —
/// this file's own `FakePort` (just below) recognises a fixed set of SQL
/// substrings and has no way to hand back an arbitrary engine message.
/// Deliberately NOT `use serving_common::fixture` at the top level: this
/// file's own `fixture()` (further down) already owns that name, and every
/// other test in this file is unaffected by and should stay on the local one.
mod serving_common;

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
        // Exactly the shapes `port_impl::execute_error` can hand the serving
        // path, spelled with the payloads a real engine attaches — and, for the
        // two typed ones, built with the SAME `ExecFailure` constructor
        // `execute_error` uses, so a change to that contract breaks here too.
        if sql.contains("does_not_exist") {
            return Err(Error::NotFound(MISSING_TABLE.into()));
        }
        if sql.contains("unreachable_engine") {
            return Err(egret_query_api::ExecFailure::new(
                egret_query_api::ExecFailureKind::EngineUnavailable,
                format!("connect: connect {ENGINE_HOST}: connection refused"),
            )
            .into_error());
        }
        if sql.contains("slow_query") {
            return Err(egret_query_api::ExecFailure::new(
                egret_query_api::ExecFailureKind::Timeout,
                "timeout",
            )
            .into_error());
        }
        if sql.contains("broken_endpoint") {
            // The published statement stopped working: the engine parsed it and
            // refused it. `InvalidInput` carrying the engine's own words is what
            // `execute_error` produces for `EngineError::Engine`.
            return Err(Error::InvalidInput(format!(
                "line 1:15: Table '{MISSING_TABLE}' does not exist"
            )));
        }
        if sql.contains("egret_is_broken") {
            // egret's OWN fault, untagged — what `connect_as` produces when the
            // metastore or the secret store fails. Must stay `internal`.
            return Err(Error::Internal(anyhow::anyhow!(
                "open secret: connection pool timed out"
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
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");

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
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // This file is about serving-path error shapes, not Activity — a fixed
        // retention is enough to satisfy the field.
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
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
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
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
               (workspace_id, name, path, method, database_id, statements, limits, cache, \
                created_by) \
             VALUES ($1, $2, $3, $4, $5, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', $6::text)), $7, $8, $9) RETURNING id",
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
///
/// Since Package 3b this is a **502 `engine_unavailable`** rather than a 500
/// `internal` — the port now says the ENGINE was unreachable, as a value — and
/// the leak rule is unchanged, which is the half worth re-asserting: being more
/// specific about the cause must not become more specific about the address.
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
    assert_eq!(res.status, StatusCode::BAD_GATEWAY, "{}", res.text);
    assert_eq!(res.code(), "engine_unavailable");
}

// ---------------------------------------------------------------------------
// Package 3b: `internal` was one code for six unrelated causes
// ---------------------------------------------------------------------------
//
// Every test below drives the WHOLE handler — token, snapshot, render, port,
// wire envelope, access-log row — rather than asserting a code string against a
// hand-built error. That matters here specifically: the classification these
// pin lives at a seam between two crates that cannot see each other, and the
// only way to know the two halves agree is to run one request through both.

/// **The finding.** An endpoint whose published statement the engine rejects is
/// the AUTHOR's problem, and it used to be indistinguishable in
/// `access_log.error_code` from egret being broken.
///
/// 500 and not a 4xx: the caller sent a perfectly good request (spec §7 (c)).
#[tokio::test]
async fn a_broken_endpoint_is_not_reported_as_egret_being_broken() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/broken", "SELECT n FROM broken_endpoint")
        .await;

    let res = f.call("broken").await;

    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        res.text
    );
    assert_eq!(res.code(), "endpoint_query_failed");
    // Still leak-proof: the engine named the missing relation, the caller may
    // not learn it.
    assert!(
        !res.text.contains(MISSING_TABLE),
        "leaked a relation name: {}",
        res.text
    );

    // And the operator gets the distinction where they actually read it.
    let row = f
        .log_for(res.request_id())
        .await
        .expect("every call leaves a row");
    assert_eq!(
        row.error_code.as_deref(),
        Some("endpoint_query_failed"),
        "access_log.error_code is the column an operator aggregates"
    );
    assert_eq!(row.status_code, 500);
}

/// The other half of the same finding, and the one that keeps it honest: egret's
/// OWN faults must still be `internal`. If both landed on the same code the
/// split would have bought nothing.
#[tokio::test]
async fn an_egret_side_fault_is_still_internal() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/ourfault", "SELECT n FROM egret_is_broken")
        .await;

    let res = f.call("ourfault").await;

    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        res.text
    );
    assert_eq!(res.code(), "internal");
    assert_eq!(
        f.log_for(res.request_id())
            .await
            .expect("row")
            .error_code
            .as_deref(),
        Some("internal")
    );
}

/// Spec §7's `timeout` (504), which P4 recorded as undecidable at this seam
/// because the port spelled it as the string `"query timed out"`.
#[tokio::test]
async fn an_execution_timeout_is_a_504_not_a_500() {
    let f = skip_without_db!();
    f.publish_endpoint("GET", "/slow", "SELECT n FROM slow_query")
        .await;

    let res = f.call("slow").await;

    assert_eq!(res.status, StatusCode::GATEWAY_TIMEOUT, "{}", res.text);
    assert_eq!(res.code(), "timeout");
    let row = f.log_for(res.request_id()).await.expect("row");
    assert_eq!(row.error_code.as_deref(), Some("timeout"));
    assert_eq!(row.status_code, 504);
}

/// The property the whole split exists for, asserted as one statement: four
/// different 5xx causes, four different `access_log.error_code` values.
///
/// Collapsing any two of them back onto `internal` fails here, and this is the
/// test an operator's question — "which endpoints are failing?" — actually
/// depends on.
#[tokio::test]
async fn four_5xx_causes_leave_four_different_codes_in_the_access_log() {
    let f = skip_without_db!();
    let cases = [
        ("brk", "broken_endpoint", "endpoint_query_failed"),
        ("slw", "slow_query", "timeout"),
        ("dwn", "unreachable_engine", "engine_unavailable"),
        ("our", "egret_is_broken", "internal"),
    ];
    for (slug, table, expected) in cases {
        f.publish_endpoint(
            "GET",
            &format!("/{slug}"),
            &format!("SELECT n FROM {table}"),
        )
        .await;
        let res = f.call(slug).await;
        assert!(
            res.status.is_server_error(),
            "{slug} should be a 5xx, got {} {}",
            res.status,
            res.text
        );
        assert_eq!(res.code(), expected, "wire code for {slug}");
        assert_eq!(
            f.log_for(res.request_id())
                .await
                .expect("row")
                .error_code
                .as_deref(),
            Some(expected),
            "access_log.error_code for {slug}"
        );
    }
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
        // M2 Task 8's two additions, asserted here rather than only in
        // `serving_routing.rs` because THIS is the test about the envelope being
        // the same one every time — and `status` mirrors the HTTP status, so a
        // constant would satisfy any single-refusal test while disagreeing with
        // one of these three.
        assert_eq!(res.body["status"], "error", "{}", res.text);
        assert_eq!(
            res.body["message"], res.body["error"]["message"],
            "the human-readable half is lifted to the envelope, not reworded \
             there: {}",
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
///
/// # What Package 8 changed here, and what it did not
///
/// The second case used to be `GET /data-api/v1/{ws}` — a single-segment URI,
/// which `{*path}` cannot match. That URL is now a route: it is the contract
/// `serving::contract` serves to the calling service account, and it answers
/// with our envelope on every refusal. So the example moved DOWN one segment to
/// `/data-api/v1` itself, which no route under this mount can match.
///
/// The claim being pinned is unchanged — the only answers on this surface that
/// carry no envelope are the ones axum gives before any handler runs — and it is
/// now pinned against a URI that is genuinely unmatched rather than one that has
/// since been claimed. A test asserting "this URL 404s empty" would otherwise
/// have gone on passing right up until the moment the URL meant something,
/// which is exactly what happened.
#[tokio::test]
async fn axum_answers_before_the_handler_and_those_two_answers_carry_no_envelope() {
    let f = skip_without_db!();

    // A method the route does not declare. Same answer for every path under
    // this router, existing or not, so it leaks nothing.
    for uri in [
        format!("/data-api/v1/{}/anything", f.ws_slug),
        // The contract route declares GET alone, so it answers 405 the same way.
        format!("/data-api/v1/{}", f.ws_slug),
    ] {
        let res = f.raw("DELETE", &uri).await;
        assert_eq!(res.status, StatusCode::METHOD_NOT_ALLOWED, "{uri}");
        assert!(
            res.text.is_empty(),
            "405 must stay empty rather than grow a body with detail in it: {}",
            res.text
        );
    }

    // No segment at all: `/{ws}` needs one and `/{ws}/{*path}` needs two, so
    // this matches no route under the mount.
    let res = f.raw("GET", "/data-api/v1").await;
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

// ---------------------------------------------------------------------------
// Task 8 — an engine permission error names the missing step, not the SQL
// ---------------------------------------------------------------------------

/// The untyped shape THIS file's own fake port (above) cannot produce:
/// `denied_table` returns a typed `Error::Forbidden`, which is not what a real
/// Trino or StarRocks denial looks like — neither connector surfaces a typed
/// authorization error (see `serving::error::is_permission_denied`'s doc).
/// This test uses `serving_common`'s fixture and `FakePort::fail_next_with`
/// instead, so the engine's answer is an arbitrary message, exactly like a
/// real one.
#[tokio::test]
async fn an_engine_permission_error_names_the_missing_step() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // The fake port answers as a Ranger-enforcing engine does when the
    // principal may not read the table.
    f.port
        .fail_next_with("Access Denied: Cannot select from table sales");

    let res = f.call_as_client(f.principal_a, "client-a", "probe").await;

    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    // The SAME code the typed `Error::Forbidden` path produces
    // (`a_second_principal_is_refused_the_same_way` above). The two
    // recognitions were once two wire codes; one of them was named for Ranger
    // and would have stated the wrong cause whenever the real fault was a bad
    // engine credential, which the typed path cannot distinguish. This
    // assertion is what proves the heuristic still maps at all.
    assert_eq!(res.body["error"]["code"], "data_access_denied");
    // The message must point at the permission, not at the SQL. An author
    // reading a raw engine error debugs their query — which is correct, but not
    // here: the query was fine, and the missing piece is a data permission or a
    // working engine credential.
    let msg = res.body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("service account"), "{msg}");
    assert!(
        !msg.contains("Cannot select from table"),
        "the raw engine error must not be echoed: {msg}"
    );
    assert!(
        !msg.contains("sales"),
        "a table name drawn from the engine's message must not reach the wire: {msg}"
    );
}

/// **Egret's OWN fault must never be reported as a data-access denial**,
/// however its message reads.
///
/// `port_impl::connect_as` produces `Error::Internal` for failures that are
/// entirely ours — resolving the principal's email, opening the database secret —
/// and `"permission denied"` is one of the most common OS, Kubernetes and vault
/// error strings there is. While `engine_error` message-matched everything that
/// was not `Error::Forbidden`, a secret this process could not open answered
/// `403 data_access_denied` and sent an operator to audit grants that were fine.
///
/// The mutation that fails this test: widen `engine_error`'s guarded arm back to
/// `other => { if is_permission_denied(&other.to_string()) { … } }`.
#[tokio::test]
async fn an_internal_fault_is_never_classified_as_a_data_access_denial() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Verbatim the shape `open_secret`'s failure takes on a vault or a
    // file-backed secret whose mode is wrong.
    f.port
        .fail_next_internal("open secret: permission denied (os error 13)");

    let res = f.call_as_client(f.principal_a, "client-a", "probe").await;

    assert_eq!(
        res.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "our fault is a 500, not a 403 pointing at the engine's permissions: {}",
        res.body
    );
    assert_eq!(res.body["error"]["code"], "internal");
    // And the operator is not handed our internals either — the message stays
    // the fixed string, with the detail only in the log.
    assert!(
        !res.body.to_string().contains("os error 13"),
        "an internal detail must not reach the caller: {}",
        res.body
    );
}

// ---------------------------------------------------------------------------
// M1 Task 8 — the defect from spec §1.3, driven end to end
// ---------------------------------------------------------------------------

/// **The defect this whole change exists for, through publish AND serving.**
///
/// Spec §1.3: a conditional projection published green and then answered
/// `503 schema_drift` on `?detail=false` — and on the call with NO query string
/// at all, which is the default the endpoint's own generated OpenAPI document
/// advertises. Only `?detail=true` worked, because publish described the one
/// rendering the representative (truthy) values happened to take and froze THAT
/// branch's columns as the contract; every other branch then failed serving's
/// schema check with a row it had just fetched successfully.
///
/// Every other test in this change measures one half. This one is the only place
/// the two halves meet: the union can be computed perfectly at publish while
/// `shape_rows` still demands equality, and each half's own suite would stay
/// green. It lives here because this file owns the `schema_drift` cases, and
/// this test retires one of them.
///
/// # What stops it passing vacuously
///
/// `FakePort::set_columns_per_sql` answers `{id, name}` only for the statement
/// that CONTAINS `name`, and `{id}` for everything else — for describe and for
/// execute alike. So:
///
/// * if publish froze one branch instead of the union, the frozen list is
///   `{id, name}` (or `{id}`) and two of the three calls below drift;
/// * if serving still compared column SETS for equality, the two calls that
///   omit `name` are `503`;
/// * if nothing rendered at all, every call executes the same statement and the
///   `?detail=true` assertion — two keys, not one — fails.
///
/// Measured red at `fc530bc7` (the tree before any of this change landed): the
/// bare call and `?detail=false` both returned `503 schema_drift`.
#[tokio::test]
async fn a_conditional_endpoint_serves_its_own_default_call() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // First arm wins, so the fallback is last. Installed BEFORE publish, since
    // publish is where the union is described and frozen.
    f.port
        .set_columns_per_sql(&[("name", &["id", "name"][..]), ("", &["id"][..])]);

    f.publish_with_params(
        "/cond",
        f.database_id,
        serde_json::json!({ "enabled": false }),
        serde_json::json!({}),
        "SELECT id {% if detail %}, name {% endif %}FROM t",
        // Optional and with no default: `required` and `default` both take their
        // serde defaults, which is the exact shape spec §1.3 describes.
        serde_json::json!([{ "name": "detail", "type": "boolean", "location": "query" }]),
    )
    .await;

    // 1. The documented default: no query string at all. This was the 503.
    let bare = f.call_as(f.principal_a, "cond").await;
    assert_eq!(
        bare.status,
        StatusCode::OK,
        "the call the endpoint's own document advertises: {}",
        bare.body
    );
    assert_eq!(
        keys(&bare.body["data"][0]),
        vec!["id"],
        "an optional column a branch does not return must be ABSENT, not null: {}",
        bare.body
    );

    // 2. The parameter sent explicitly false — the same branch, reached the
    //    other way. Kept separate because the absent context and the `false`
    //    probe are two different renders at publish, and only one of them is
    //    produced by `render_permutations`.
    let off = f.call_as(f.principal_a, "cond?detail=false").await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(keys(&off.body["data"][0]), vec!["id"], "{}", off.body);

    // 3. The branch that always worked, so a fix that simply stopped checking
    //    anything is still visible: this one has to gain a column.
    let on = f.call_as(f.principal_a, "cond?detail=true").await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(keys(&on.body["data"][0]), vec!["id", "name"], "{}", on.body);

    // And the contract the caller is handed is the UNION, with `name` marked
    // optional — not the branch this particular call took. Without this the
    // three assertions above are satisfied by a serving path that ignores the
    // frozen list entirely.
    let declared: Vec<(&str, bool)> = on.body["meta"]["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("no meta.columns in {}", on.body))
        .iter()
        .map(|c| {
            (
                c["name"].as_str().expect("column name"),
                c["required"].as_bool().expect("column required"),
            )
        })
        .collect();
    assert_eq!(
        declared,
        vec![("id", true), ("name", false)],
        "publish froze one branch rather than the union of them: {}",
        on.body
    );
}

/// **The same defect BEYOND the probe budget, which no cap can reach.**
///
/// `render::MAX_PROBES` counts the base context itself, so a single parameter
/// reaches at most sixty-three candidates however high `MAX_ENUM_CANDIDATES`
/// is set. An enum with a hundred allowed values and a `default` at index 90
/// therefore has NO route into the probe set at all — and it is the value every
/// call with no query string sends.
///
/// This is the test the cap raise cannot satisfy, and it is why the declared
/// default is a CONTEXT (`lifecycle::resting`) rather than a candidate: a
/// context is always rendered and cannot be starved by a wide definition. The
/// sibling test above is the defect as reported; this one is the general rule.
#[tokio::test]
async fn an_enum_default_beyond_the_probe_budget_still_serves_its_default_call() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    f.port
        .set_columns_per_sql(&[("wide_col", &["id", "wide_col"][..]), ("", &["id"][..])]);

    let allowed: Vec<String> = (0..100).map(|i| format!("v{i}")).collect();
    f.publish_with_params(
        "/enumwide",
        f.database_id,
        serde_json::json!({ "enabled": false }),
        serde_json::json!({}),
        "SELECT id {% if mode | value == 'v90' %}, wide_col {% endif %}FROM t",
        serde_json::json!([{
            "name": "mode", "type": "enum", "location": "query",
            "default": "v90", "allowed": allowed
        }]),
    )
    .await;

    let bare = f.call_as(f.principal_a, "enumwide").await;
    assert_eq!(
        bare.status,
        StatusCode::OK,
        "the default is past every candidate the budget can reach: {}",
        bare.body
    );
    assert_eq!(
        keys(&bare.body["data"][0]),
        vec!["id", "wide_col"],
        "{}",
        bare.body
    );

    // The other side, so a fix that stopped checking anything is still visible.
    let off = f.call_as(f.principal_a, "enumwide?mode=v0").await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(keys(&off.body["data"][0]), vec!["id"], "{}", off.body);
}

/// The keys of one row object, in the order the body carries them.
fn keys(row: &serde_json::Value) -> Vec<&str> {
    row.as_object()
        .unwrap_or_else(|| panic!("expected an object row, got {row}"))
        .keys()
        .map(String::as_str)
        .collect()
}

// ---------------------------------------------------------------------------
// Whole-branch review, Important 1 — an enum's own DEFAULT is never described
// ---------------------------------------------------------------------------

/// **The headline defect, reproduced through a CONSTRAINED parameter.**
///
/// `render::candidates` answers an enum with its `allowed` list and nothing
/// else — the definition's own literals are not consulted for it — and then
/// takes only the first `MAX_ENUM_CANDIDATES` of them. `branch_renderings`
/// gives an absent context only to parameters with `default.is_none()`, which
/// is right (serving substitutes the default, so the parameter is never
/// absent), but it means the default's own branch has exactly one route into
/// the probe set: being a candidate.
///
/// So an enum whose `default` sits past the cap is never rendered at publish.
/// The union is built without its columns, publish is green, and the call with
/// NO query string — the one `openapi_gen` advertises as the default — answers
/// `503 schema_drift`. That is the §1.3 defect this whole change exists to
/// remove, back again through the one parameter type that is supposed to be
/// the most tightly described.
///
/// The perversity worth keeping: a plain `string` parameter would have been
/// probed with `'revenue'`, because a non-enum scalar harvests its candidates
/// from the definition's own literals and `'revenue'` is written right there in
/// the `{% if %}`. Constraining the type is what created the blind spot.
#[tokio::test]
async fn an_enum_default_past_the_candidate_cap_still_serves_its_default_call() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    f.port.set_columns_per_sql(&[
        ("revenue_rank", &["id", "revenue_rank"][..]),
        ("", &["id"][..]),
    ]);

    // Twelve allowed values with the DEFAULT at index 10 — past the cap of
    // eight, and deliberately not `allowed[0]`, which is the value
    // `representative_value` picks.
    f.publish_with_params(
        "/enumdef",
        f.database_id,
        serde_json::json!({ "enabled": false }),
        serde_json::json!({}),
        "SELECT id {% if mode | value == 'revenue' %}, revenue_rank {% endif %}FROM t",
        serde_json::json!([{
            "name": "mode",
            "type": "enum",
            "location": "query",
            "default": "revenue",
            "allowed": [
                "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9",
                "revenue", "z11"
            ]
        }]),
    )
    .await;

    // The documented default call: no query string at all.
    let bare = f.call_as(f.principal_a, "enumdef").await;
    assert_eq!(
        bare.status,
        StatusCode::OK,
        "the call the endpoint's own document advertises: {}",
        bare.body
    );
    assert_eq!(
        keys(&bare.body["data"][0]),
        vec!["id", "revenue_rank"],
        "the default's branch must be served, not merely accepted: {}",
        bare.body
    );

    // The same value sent explicitly, which is the same branch reached the
    // other way. Kept separate: the default is applied in `serving::params` and
    // an explicit value in the coercion path, and only one of them is what
    // publish had to anticipate.
    let sent = f.call_as(f.principal_a, "enumdef?mode=revenue").await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
    assert_eq!(
        keys(&sent.body["data"][0]),
        vec!["id", "revenue_rank"],
        "{}",
        sent.body
    );

    // A value from the OTHER side of the `{% if %}`, so a fix that simply
    // stopped checking anything is still visible: this one has to LOSE the
    // column, and it has to keep its 200.
    let off = f.call_as(f.principal_a, "enumdef?mode=a0").await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(keys(&off.body["data"][0]), vec!["id"], "{}", off.body);

    // And the frozen contract is the union, with `revenue_rank` optional —
    // without this the three assertions above are satisfied by a serving path
    // that ignores the frozen list.
    let declared: Vec<(&str, bool)> = sent.body["meta"]["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("no meta.columns in {}", sent.body))
        .iter()
        .map(|c| {
            (
                c["name"].as_str().expect("column name"),
                c["required"].as_bool().expect("column required"),
            )
        })
        .collect();
    assert_eq!(
        declared,
        vec![("id", true), ("revenue_rank", false)],
        "the enum's default branch never entered the union: {}",
        sent.body
    );
}

// ---------------------------------------------------------------------------
// M2 Task 7 — several statements, executed and shaped one by one
// ---------------------------------------------------------------------------
//
// These three live in this file because the middle one is a `schema_drift`
// case, which this file owns, and because the other two are what stop it
// passing vacuously: a refusal test proves nothing unless the same fixture,
// without the drift installed, serves. All three drive the REAL publish route
// and the REAL serving router, because the defect they close sat between the
// two — publish froze N statements while serving executed one.

/// Two statements, two frozen contracts, one call: each result is shaped
/// against ITS OWN columns and arrives under its own name.
///
/// # What stops it passing vacuously
///
/// `set_columns_per_sql` answers `{total}` for the statement containing
/// `count` and `{id, name}` for the other, for DESCRIBE and execute alike. So:
///
/// * a serving path that executes only the first statement produces `data` as
///   an ARRAY and the `data["total"]` assertion fails;
/// * a serving path that executes both but shapes both against
///   `statements[0]`'s frozen columns answers `503 schema_drift` for `total`,
///   because `total` was never declared by the `rows` statement;
/// * a fake that ignored the SQL would hand both statements one column list and
///   the two key assertions below could not disagree.
#[tokio::test]
async fn each_statement_is_served_under_its_own_name_and_its_own_contract() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // First arm wins, so the fallback goes last. Installed BEFORE publish: this
    // table is what publish describes and freezes.
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);

    f.publish_statements(
        "/two",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
    )
    .await;

    let res = f.call_as(f.principal_a, "two").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);

    // `data` is keyed by statement name once there is more than one — the shape
    // spec §4.4 fixes, and the reason a name is validated as an identifier at
    // publish.
    assert_eq!(
        keys(&res.body["data"]),
        vec!["rows", "total"],
        "each statement answers under its own name: {}",
        res.body
    );
    assert_eq!(
        keys(&res.body["data"]["rows"][0]),
        vec!["id", "name"],
        "the rows statement's own columns: {}",
        res.body
    );
    assert_eq!(
        keys(&res.body["data"]["total"][0]),
        vec!["total"],
        "the total statement's own column, not the rows statement's: {}",
        res.body
    );

    // Two executions, not one and not three: sequential, one `execute` each.
    assert_eq!(
        f.port.executions(),
        2,
        "one execution per statement: {}",
        res.body
    );
    // And each statement's count is reported under its own name.
    //
    // This assertion read `meta.row_count == 2` — the SUM — until M2 Task 8
    // replaced `meta` with the keyed shape. The sum was the best a single
    // integer could do; it is not the row count of anything a caller can name,
    // because the two statements share no transaction and no snapshot (§4.5).
    // `several_statements_report_their_contract_under_their_own_names` is where
    // the new shape is pinned in full; this keeps the reading the original
    // assertion was for — that a client cannot read one statement's arity as
    // the call's.
    assert_eq!(
        res.body["meta"]["statements"]["rows"]["row_count"], 1,
        "{}",
        res.body
    );
    assert_eq!(
        res.body["meta"]["statements"]["total"]["row_count"], 1,
        "{}",
        res.body
    );
}

/// **A drift in the SECOND statement refuses the whole call**, with
/// `schema_drift` — not a `200` carrying the first statement's rows.
///
/// The drift is installed AFTER publish, through the execution-only table, so
/// the contract this endpoint froze is the honest one and only the engine has
/// moved. That is the real sequence: a column is renamed, days after the
/// snapshot was taken.
///
/// The refusal CODE is asserted, not just the status: `503` covers both
/// `schema_drift` and `output_naming_collision`, and a test that reads only the
/// status cannot tell an operator which of the two fixes it is asking for.
#[tokio::test]
async fn a_drift_in_the_second_statement_refuses_the_whole_call() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);

    f.publish_statements(
        "/drift2",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
    )
    .await;

    // Only the SECOND statement's table moved. The first still returns exactly
    // what it froze, so a serving path that stops after the first sees nothing
    // wrong at all — which is the failure this test exists for.
    f.port.set_execution_columns_per_sql(&[
        ("count", &["grand_total"][..]),
        ("", &["id", "name"][..]),
    ]);

    let res = f.call_as(f.principal_a, "drift2").await;

    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a drift in the second statement must refuse the call: {}",
        res.body
    );
    assert_eq!(
        res.body["error"]["code"], "schema_drift",
        "the operator is told to re-publish, not to change `output.naming`: {}",
        res.body
    );
    // And the first statement's rows do NOT come out with the refusal: a
    // partial answer under a 503 is the worst of both.
    assert!(
        res.body.get("data").is_none(),
        "a refused call carries no data at all: {}",
        res.body
    );
}

/// **The row cap is charged per statement**, which is the difference between a
/// `total` statement costing one row and a `total` statement costing the `rows`
/// statement its budget.
///
/// Measured on the LIMITS EACH EXECUTION WAS GIVEN, not on the rows that came
/// back: this fake returns one row per statement whatever cap it is handed, so
/// the body is identical under a per-statement cap, a shared one, and a
/// decrementing one. `limits_seen()` is the only instrument that separates them.
#[tokio::test]
async fn the_row_cap_is_charged_per_statement() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // 7 and 11 rather than round numbers: both survive `execution_limits`'
    // clamp untouched, and neither can be produced by a default.
    f.publish_statements(
        "/cap",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({ "max_rows": 7, "timeout_seconds": 11 }),
    )
    .await;

    let res = f.call_as(f.principal_a, "cap").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);

    assert_eq!(
        f.port.limits_seen(),
        vec![
            (7, std::time::Duration::from_secs(11)),
            (7, std::time::Duration::from_secs(11)),
        ],
        "every statement gets the endpoint's whole cap, not a share of it and \
         not what the statement before it left over",
    );
}

// ---------------------------------------------------------------------------
// M2 Task 8 — the envelope, where it has to say something new (spec §4.4)
// ---------------------------------------------------------------------------

/// **`meta` keys the contract by statement name once there are several.**
///
/// This is the field Task 7 could not express and left as a concatenation:
/// `meta.columns` reported every statement's frozen columns in one flat list,
/// so a caller holding `["id","name","total"]` had no way to learn which
/// statement declared which — and no way to notice that the question was
/// unanswerable.
///
/// The two endpoint-wide fields are asserted ABSENT. A sum of row counts across
/// independent reads is not the row count of anything, and a flat union of
/// columns is a contract no statement has; leaving either beside the keyed map
/// would give a client two readings that can disagree.
#[tokio::test]
async fn several_statements_report_their_contract_under_their_own_names() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // First arm wins, so the fallback goes last. Installed BEFORE publish: this
    // is what publish describes and freezes.
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);

    f.publish_statements(
        "/meta-two",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
    )
    .await;

    let res = f.call_as(f.principal_a, "meta-two").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);

    assert_eq!(
        keys(&res.body["meta"]["statements"]),
        vec!["rows", "total"],
        "the contract is keyed by statement name, in the author's order: {}",
        res.body
    );
    assert_eq!(
        res.body["meta"]["statements"]["rows"]["row_count"], 1,
        "{}",
        res.body
    );
    assert_eq!(
        column_names(&res.body["meta"]["statements"]["rows"]["columns"]),
        vec!["id", "name"],
        "the rows statement's own frozen columns: {}",
        res.body
    );
    assert_eq!(
        column_names(&res.body["meta"]["statements"]["total"]["columns"]),
        vec!["total"],
        "the total statement's own frozen column, not the rows statement's and \
         not the concatenation of both: {}",
        res.body
    );

    assert!(
        res.body["meta"]["row_count"].is_null(),
        "a sum across independent reads is not the row count of anything: {}",
        res.body
    );
    assert!(
        res.body["meta"]["columns"].is_null(),
        "the unkeyed concatenation is gone — it is what this field replaces: {}",
        res.body
    );

    // The endpoint-wide half of `meta` is untouched by the split.
    assert_eq!(res.body["meta"]["version"], 1, "{}", res.body);
    assert_eq!(res.body["meta"]["cached"], false, "{}", res.body);
    assert!(res.body["meta"]["request_id"].is_string(), "{}", res.body);
    assert_eq!(res.body["status"], "ok", "{}", res.body);
}

/// **`bare` writes `data`'s VALUE**, which for several statements is the object
/// keyed by name — not "the rows".
///
/// Driven with several statements on purpose: with one, `bare` and "the rows
/// array" are the same body, so a single-statement test cannot tell an
/// implementation that writes `data` from one that writes the first statement's
/// rows.
#[tokio::test]
async fn bare_with_several_statements_writes_the_keyed_object_alone() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);

    f.publish_statements_with_output(
        "/bare-two",
        f.database_id,
        serde_json::json!([
            { "name": "rows",  "sql": "SELECT id, name FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
        ]),
        serde_json::json!({}),
        serde_json::json!({ "envelope": "bare" }),
    )
    .await;

    let res = f.call_as(f.principal_a, "bare-two").await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        keys(&res.body),
        vec!["rows", "total"],
        "the body IS `data`'s value — the keyed object, with no wrapper: {}",
        res.body
    );
    assert_eq!(keys(&res.body["total"][0]), vec!["total"], "{}", res.body);
}

/// A cache HIT reports the same `meta` a miss did.
///
/// The per-statement row counts are computed while the statements run, so a hit
/// — which runs none of them — has to carry them. The first version of this
/// field did not, and a warm several-statement endpoint would have answered
/// with an empty `statements` map under a `200`.
#[tokio::test]
async fn a_cache_hit_reports_the_same_per_statement_meta_as_the_miss() {
    let Some(f) = serving_common::fixture().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    f.port
        .set_columns_per_sql(&[("count", &["total"][..]), ("", &["id", "name"][..])]);

    let ep = f
        .publish_statements(
            "/cached-two",
            f.database_id,
            serde_json::json!([
                { "name": "rows",  "sql": "SELECT id, name FROM t" },
                { "name": "total", "sql": "SELECT count(*) AS total FROM t" },
            ]),
            serde_json::json!({}),
        )
        .await;
    // `publish_statements` publishes with caching OFF — every other assertion in
    // this file counts executions. `rows_affected` is asserted because a
    // `jsonb_set` down a path that does not exist still reports success, and
    // this test would then measure a cache that was never on.
    let touched = sqlx::query(
        "UPDATE dataapi.endpoint_versions \
            SET snapshot = jsonb_set(snapshot, '{cache}', \
                '{\"enabled\":true,\"ttl_seconds\":60,\"scope\":\"per_principal\"}'::jsonb) \
          WHERE endpoint_id = $1",
    )
    .bind(ep)
    .execute(&f.db.pool)
    .await
    .expect("enable caching on the frozen snapshot")
    .rows_affected();
    assert_eq!(touched, 1, "the frozen snapshot must actually have changed");

    let miss = f.call_as(f.principal_a, "cached-two").await;
    assert_eq!(miss.status, StatusCode::OK, "{}", miss.body);
    assert_eq!(miss.body["meta"]["cached"], false, "{}", miss.body);

    let hit = f.call_as(f.principal_a, "cached-two").await;
    assert_eq!(hit.status, StatusCode::OK, "{}", hit.body);
    assert_eq!(hit.body["meta"]["cached"], true, "{}", hit.body);
    assert!(
        hit.body["meta"]["statements"].is_object(),
        "asserted non-null before the comparison below: two absent fields \
         compare equal, so the equality alone would hold for an implementation \
         that reported no statements on either side: {}",
        hit.body
    );
    assert_eq!(
        hit.body["meta"]["statements"], miss.body["meta"]["statements"],
        "a hit describes the same statements the miss did — it is the same \
         body, and the only field that may differ is `cached`.\nmiss: {}\nhit:  {}",
        miss.body, hit.body
    );
    assert_eq!(hit.body["data"], miss.body["data"], "{}", hit.body);
}

/// Column NAMES from a `meta` column list, in order.
fn column_names(v: &serde_json::Value) -> Vec<&str> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected a column array, got {v}"))
        .iter()
        .map(|c| {
            c["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a column with no name: {c}"))
        })
        .collect()
}
