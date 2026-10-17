//! The one Data API path that is executed by a REAL engine.
//!
//! # Why this file exists
//!
//! Every other integration suite in this crate drives `serving_common::FakePort`, a
//! hand-written `QueryExecPort` that returns rows derived from the calling principal. That
//! makes the *serving* logic testable — it is what lets `serving_cache.rs` tell a cache hit
//! from a re-execution — but it means **nothing engine-side has ever been executed by a
//! test in this crate**. Every claim in it about how StarRocks or Trino behaves rests on
//! reading the connector, not on running it. An audit of this feature found that every
//! StarRocks-backed endpoint returned a bare 500 for a period, and the error classification
//! is still unfixed; that is precisely the class of defect a fake cannot see, because the
//! fake decides what an engine error looks like.
//!
//! So this file publishes an endpoint and serves it end to end with
//! `egret_query::EngineState` — the real port implementation the binary ships — in the
//! slot the fake usually occupies. Publish resolves the output columns by asking StarRocks
//! to plan the statement; serving executes it and returns the rows StarRocks returned.
//!
//! # Running it
//!
//! Two gates, both following the conventions already in this repo — `DATABASE_URL` for the
//! metastore (see `docs/en/testing/db-gated-tests.md`) and `EGRET_TEST_STARROCKS_URL` for
//! the engine, the same variable `egret-query`'s connector suites use:
//!
//! ```bash
//! just test-dataapi-starrocks
//! ```
//!
//! or by hand:
//!
//! ```bash
//! just up                                    # brings up StarRocks on :9030
//! ./deploy/seed-starrocks-dataapi.sh         # the fixture table and its three rows
//! export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//! export EGRET_TEST_STARROCKS_URL=mysql://localhost:9030
//! cargo test -p egret-dataapi --test serving_starrocks_live
//! ```
//!
//! # It self-skips without the engine, and that is deliberate
//!
//! `DATABASE_URL` is now fail-closed workspace-wide, but the ENGINE gate is a separate
//! axis: CI has no StarRocks to give this, so a hard failure there would only teach people
//! to ignore it. It follows the existing opt-in-behind-an-env-var convention
//! (`EGRET_TEST_TRINO_URL`, `EGRET_TEST_S3_ENDPOINT`, `EGRET_TEST_RANGER_E2E`).
//!
//! **A self-skipped run therefore proves nothing here.** Every assertion below is written
//! so that it could not pass against the fake: the row VALUES come from a table this file
//! does not write and the fake could not invent, and the column names come from a StarRocks
//! plan rather than from a canned `vec![col("n")]`.

mod serving_common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_dataapi::{
    management_router, serving_router, ActivityStore, ClientStore, DataApiState, EndpointStore,
};
use egret_metastore::Db;
use egret_query::execution::model::Limits;
use egret_query::{DatabaseRepo, EngineState, JobManager, QueryStore};
use egret_storage::{ObjectStore, StorageError};
use serving_common::{mint, FakeKeys};
use tower::ServiceExt;
use uuid::Uuid;

/// The schema and table `deploy/seed-starrocks-dataapi.sh` creates.
///
/// Named as constants and referenced from both the SQL and the assertions so a rename
/// cannot leave the test querying one table and asserting about another.
const SEED_DB: &str = "egret_dataapi_live";
const SEED_TABLE: &str = "regions";

/// The rows the seed script inserts: `('EU', 10), ('EU', 32), ('US', 7)`.
///
/// Asserted as the GROUPED result — `EU => 42`, `US => 7` — because a sum is something the
/// engine computed rather than something it echoed. `42` appears nowhere in the request, in
/// the endpoint definition, or in any fixture in this crate: it exists only if StarRocks
/// aggregated two rows this test never sent it.
const EXPECTED: [(&str, i64); 2] = [("EU", 42), ("US", 7)];

// ---------------------------------------------------------------------------
// The parts of `EngineState` this path does not use
// ---------------------------------------------------------------------------

/// The Data API serving path never materialises a result set to object storage — it
/// returns rows in the response body — so the store is inert. It is `Err` rather than
/// `Ok(vec![])` on `get` so that a future change which DID start reading through it fails
/// loudly instead of silently seeing an empty object.
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

/// `JobManager`'s factory is unused: `describe_as`/`execute_as` build their connector
/// directly through `build_connector`. It returns an error rather than a stub connector so
/// that a path which started using it could not silently succeed against nothing.
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

/// The connection under test is NOT delegated, so `delegation_for` never asks the broker.
/// Refusing rather than minting keeps that a fact rather than an assumption: if the
/// delegation branch were ever taken, this fails instead of quietly issuing a token.
struct NoMinter;

#[async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Live {
    db: Db,
    serving: axum::Router,
    mgmt: axum::Router,
    session: String,
    ws_id: Uuid,
    ws_slug: String,
    admin: Uuid,
    principal: egret_core::UserId,
    database_id: Uuid,
}

/// `mysql://host:port` -> `(host, port)`. Port defaults to StarRocks' query port.
fn parse_engine_url() -> Option<(String, i32)> {
    let raw = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&raw).expect("EGRET_TEST_STARROCKS_URL must be a URL");
    Some((u.host_str()?.to_string(), u.port().unwrap_or(9030) as i32))
}

/// `Err` names the gate that was actually closed.
///
/// The two gates are different axes and a reader has to be able to tell them apart: the
/// ENGINE gate is a deliberate self-skip (CI has no StarRocks), while a missing
/// `DATABASE_URL` reaches here only because someone passed `EGRET_ALLOW_DB_SKIP`. Both
/// used to print "EGRET_TEST_STARROCKS_URL unset", which sent the reader to the wrong
/// remedy in the second case.
async fn live() -> Result<Live, String> {
    let Some((host, port)) = parse_engine_url() else {
        return Err("EGRET_TEST_STARROCKS_URL unset".to_string());
    };
    // Fail-closed on the metastore half — see `egret_test_db::url`. Reaching the `None`
    // arm therefore means the operator asked for a database-less run; only the ENGINE gate
    // above skips on its own.
    let Some(url) = egret_test_db::url().await else {
        return Err(format!(
            "DATABASE_URL unset, with {} set",
            egret_test_db::ALLOW_SKIP_VAR
        ));
    };
    let db = Db::connect(&url, 5).await.expect("connect");

    let admin = insert_user(&db, "sr_admin").await;
    let slug = format!("sr-{}", Uuid::new_v4().simple());
    let ws_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Data API live StarRocks', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let principal = egret_core::UserId(insert_user(&db, "sr_sa").await);

    // A REAL StarRocks registration. `delegated = false` and no secret: the dev container
    // runs `root` with no password, which is what `EGRET_TEST_STARROCKS_URL` points at.
    let database_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, options, delegated, created_by) \
         VALUES ($1, $2, 'starrocks', $3, $4, $5, false, $6) RETURNING id",
    )
    .bind(ws_id)
    .bind(format!("sr-{}", Uuid::new_v4().simple()))
    .bind(&host)
    .bind(port)
    .bind(serde_json::json!({ "user": "root" }))
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("insert starrocks database");

    let author = insert_user(&db, "sr_author").await;
    grant(
        &db,
        author,
        ws_id,
        &["data_api_endpoint:read", "data_api_endpoint:manage"],
    )
    .await;
    grant_ws_admin(&db, admin, ws_id).await;

    // The REAL PDP, built AFTER the grants — `CedarPdp` snapshots policies at build time.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );

    // ---- the whole point of the file: a REAL `QueryExecPort` ----
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
    let engine = EngineState {
        store: store.clone(),
        jobs: JobManager::new(
            store,
            databases.clone(),
            Arc::new(UnusedFactory),
            storage.clone(),
            "s3://warehouse".into(),
            "query-results".into(),
            limits.clone(),
            100,
        ),
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
        pdp: pdp.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
    };

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
        // Not a fake. This is `egret_query`'s own implementation, the one `egret-bin` wires.
        query: Arc::new(engine),
        keys: Arc::new(FakeKeys),
        issuer: "https://egret.test".into(),
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: Arc::new(egret_dataapi::serving::TokenBucket::new()),
        activity: ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
    };

    let session = st
        .sessions
        .create(egret_core::ids::UserId(author), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    Ok(Live {
        db,
        serving: axum::Router::new()
            .nest("/data-api/v1", serving_router())
            .with_state(st.clone()),
        mgmt: axum::Router::new()
            .nest("/api", management_router())
            .with_state(st),
        session,
        ws_id,
        ws_slug: slug,
        admin,
        principal,
        database_id,
    })
}

impl Live {
    /// Publish `sql` at `path` through the REAL publish route, so the snapshot under test
    /// is the one publish froze — including the output columns StarRocks reported.
    ///
    /// `params` moves with `sql` because publish refuses a declared parameter no
    /// placeholder references: a fixture that split them would fail at publish rather than
    /// at the assertion under test.
    async fn publish(&self, path: &str, sql: &str, params: serde_json::Value) -> Uuid {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, params, cache, rate_limit, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, \
                     jsonb_build_array(jsonb_build_object('name', 'main', 'sql', $5::text)), \
                     $6, $7, '{}'::jsonb, $8) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(self.database_id)
        .bind(sql)
        .bind(&params)
        .bind(serde_json::json!({ "enabled": false }))
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft");

        let res = self
            .mgmt
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/ws/{}/data-api/endpoints/{ep}/publish",
                        self.ws_slug
                    ))
                    // A human session cookie, not a bearer: the MANAGEMENT surface
                    // authenticates through `WsContext`/`SessionService`. Only the serving
                    // surface below takes a minted `data-api` token.
                    .header("Cookie", format!("egret_session={}", self.session))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .expect("publish request");
        let status = res.status();
        let body = body_json(res).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "publish must succeed against a live StarRocks — it is what resolves the \
             output columns: {body}"
        );
        serving_common::grant_invoke(&self.db, self.principal, ep).await;
        ep
    }

    /// A consumer call through the REAL serving router.
    async fn call(&self, path: &str) -> (StatusCode, serde_json::Value) {
        let token = mint(self.ws_id, self.principal, "sr-client");
        let res = self
            .serving
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/data-api/v1/{}/{path}", self.ws_slug))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("serving request");
        let status = res.status();
        (status, body_json(res).await)
    }
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
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
    .bind(format!("test_dataapi_sr_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert role");
    for p in perms {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&db.pool)
            .await
            .expect("grant permission to role");
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
    .expect("insert access grant");
}

async fn grant_ws_admin(db: &Db, user: Uuid, ws: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'workspace', $2 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_admin' \
         ON CONFLICT DO NOTHING",
    )
    .bind(user)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant ws_admin");
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

/// Publish an endpoint against a live StarRocks and serve it, asserting the ROWS.
///
/// Every step is the shipped one: the publish route resolves the output columns by asking
/// StarRocks to plan the statement, the serving router verifies the token, evaluates
/// `data_api_endpoint:invoke`, and executes through `egret_query::EngineState`.
///
/// # Why the assertion is on `42`
///
/// The seed inserts `('EU', 10), ('EU', 32), ('US', 7)` and the endpoint groups them. `42`
/// is therefore a value StarRocks computed: it is not in the request, not in the endpoint
/// definition, and not producible by `serving_common::FakePort`, whose rows are a function
/// of the calling principal's UUID. Swapping the real port back for the fake makes this
/// fail rather than pass differently — which is the property that makes it evidence.
#[tokio::test]
async fn a_published_endpoint_returns_rows_a_real_starrocks_computed() {
    let f = match live().await {
        Ok(f) => f,
        Err(why) => {
            eprintln!(
                "SKIPPED, asserted nothing — {why}. \
                 See this file's header, or run `just test-dataapi-starrocks`."
            );
            return;
        }
    };

    f.publish(
        "/regions",
        &format!(
            "SELECT region, sum(units) AS units FROM {SEED_DB}.{SEED_TABLE} \
             GROUP BY region ORDER BY region"
        ),
        serde_json::json!([]),
    )
    .await;

    let (status, body) = f.call("regions").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a published endpoint on a live StarRocks must serve — the audit found this \
         returning a bare 500: {body}"
    );

    let rows = body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("no data array in {body}"));
    assert_eq!(
        rows.len(),
        EXPECTED.len(),
        "two groups, one per region: {body}"
    );

    for (i, (region, units)) in EXPECTED.iter().enumerate() {
        // Default output shape is objects keyed by the column names StarRocks reported,
        // so this also pins that publish froze the names off a real plan.
        assert_eq!(
            rows[i]["region"],
            serde_json::json!(region),
            "row {i} region, in the ORDER BY's order: {body}"
        );
        assert_eq!(
            rows[i]["units"].as_i64(),
            Some(*units),
            "row {i} units — {units} is what StarRocks summed, not anything this test sent \
             it: {body}"
        );
    }
}

/// A DECLARED PARAMETER, bound by a real StarRocks.
///
/// The higher-risk half of the same path, and the one no fake can speak to. A bound value
/// travels: caller query string -> `coerce` -> `Param` -> the connector's placeholder
/// substitution -> StarRocks' own parser. `FakePort` sees the `Param` and stops there — it
/// folds the binds into a hash and returns that, which proves the serving path PASSED
/// something but says nothing about whether the engine accepted it, substituted it in the
/// right position, or compared it against the right column.
///
/// Asserted on the FILTER's effect rather than on a status code: `?region=EU` must come
/// back with EU's `42` and, critically, WITHOUT the `US` row that
/// `a_published_endpoint_returns_rows_a_real_starrocks_computed` proves is sitting in the
/// same table. A bind that was dropped, ignored, or substituted as a literal `{{region}}`
/// produces either both rows or an engine error — never this.
///
/// This is also the only automated coverage in the repo of parameter binding on StarRocks
/// specifically, where `\` is an escape character (unlike Trino) — the asymmetry behind the
/// backslash injection this repo has already had to patch at three call sites.
#[tokio::test]
async fn a_declared_parameter_is_bound_and_filters_on_a_real_starrocks() {
    let f = match live().await {
        Ok(f) => f,
        Err(why) => {
            eprintln!(
                "SKIPPED, asserted nothing — {why}. \
                 See this file's header, or run `just test-dataapi-starrocks`."
            );
            return;
        }
    };

    f.publish(
        "/by-region",
        &format!(
            "SELECT region, sum(units) AS units FROM {SEED_DB}.{SEED_TABLE} \
             WHERE region = {{{{region}}}} GROUP BY region ORDER BY region"
        ),
        serde_json::json!([
            { "name": "region", "type": "string", "location": "query", "required": true }
        ]),
    )
    .await;

    let (status, body) = f.call("by-region?region=EU").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "parameterised serve on live StarRocks: {body}"
    );

    let rows = body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("no data array in {body}"));
    assert_eq!(
        rows.len(),
        1,
        "the bind must FILTER — `US` is in the same table, so two rows means the value \
         never reached the WHERE clause: {body}"
    );
    assert_eq!(rows[0]["region"], serde_json::json!("EU"), "{body}");
    assert_eq!(
        rows[0]["units"].as_i64(),
        Some(42),
        "EU's two rows summed by StarRocks: {body}"
    );

    // The frozen contract carries StarRocks' OWN types, not a fake's `integer` for
    // everything — the other half of what publishing against a live engine buys.
    let columns = &body["meta"]["columns"];
    assert_eq!(columns[0]["ty"], serde_json::json!("varchar"), "{body}");
    assert_eq!(columns[1]["ty"], serde_json::json!("bigint"), "{body}");
}
