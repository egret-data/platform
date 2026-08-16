//! Shared fixture for the two P4 Task 4 suites (`serving_cache.rs`,
//! `serving_ratelimit.rs`).
//!
//! Modelled on `serving_routing.rs`'s fixture, with one difference that is the
//! whole point of the cache suite: **this fake engine's rows depend on the
//! PRINCIPAL**, the way a Ranger-filtered engine's do. That is what makes "two
//! identities must not share a cached response" a statement about the cache key
//! rather than about a constant.
//!
//! Endpoints are published through the REAL publish route, so the snapshot under
//! test is the one publish freezes — a snapshot a test wrote itself could
//! disagree with the real one and then every assertion would be checking a
//! fixture against itself.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::{ColumnMeta, Error, TabularResult, UserId};
use egret_dataapi::{management_router, serving_router, ClientStore, DataApiState, EndpointStore};
use egret_machine_idp::{generate_rsa_key, sign_claims, IdpError, KeyMaterial, SigningKeyStore};
use egret_metastore::Db;
use egret_query_api::Param;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// An engine that filters per principal, like Ranger does.
// ---------------------------------------------------------------------------

/// The value this principal is allowed to see.
///
/// Derived from the user's UUID so it is stable across calls (a cache hit and a
/// re-execution are indistinguishable in the body — which is exactly why the
/// suite also counts executions) and different per identity.
pub fn marker(user: UserId) -> i64 {
    let b = user.0.as_bytes();
    i64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A signature of the values a call bound, folded into the rows the fake
/// returns.
///
/// This is what makes a parameter test non-vacuous. `marker(principal)` alone
/// would come back identically whether the serving path passed `binds` through,
/// passed an empty slice, or executed `snap.sql` unrendered — three very
/// different behaviours, one indistinguishable body. Folding the binds in means
/// the ROWS change when the bound values change, so an assertion that
/// `?region=EU` and `?region=US` differ is an assertion about the serving path
/// and not about a constant.
///
/// Zero for an empty list, on purpose: an endpoint with no parameters returns
/// exactly `marker(principal)`, which is what every pre-P4.5 assertion in these
/// suites was written against.
pub fn bind_signature(params: &[Param]) -> i64 {
    let mut h: i64 = 0;
    for p in params {
        for b in format!("{p:?}").into_bytes() {
            h = h.wrapping_mul(31).wrapping_add(i64::from(b));
        }
        h = h.wrapping_mul(31).wrapping_add(1);
    }
    h
}

/// Counts executions, so a test can tell a cache HIT from a re-execution that
/// happened to return the same rows — and records what the last one was ASKED to
/// run, so a test can assert on the rendered statement rather than only on the
/// rows it produced.
#[derive(Default)]
pub struct FakePort {
    pub executions: AtomicUsize,
    pub last: Mutex<Option<(String, Vec<Param>)>>,
}

impl FakePort {
    pub fn executions(&self) -> usize {
        self.executions.load(Ordering::SeqCst)
    }

    /// The `(sql, params)` of the most recent execution.
    pub fn last(&self) -> (String, Vec<Param>) {
        self.last
            .lock()
            .unwrap()
            .clone()
            .expect("no execution has reached the port")
    }
}

fn col(name: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        ty: "integer".into(),
    }
}

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for FakePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![col("n")])
    }

    /// Deliberately NOT the method the serving path calls, and it returns rows
    /// that ignore the binds because it has none.
    ///
    /// `serving/router.rs` calls `execute_as_with_params` for every request,
    /// parameterised or not. If it ever went back to calling this one, a
    /// parameterised endpoint's rows would lose their bind signature and every
    /// assertion in `serving_params.rs` that two values differ would fail. That
    /// is the point of keeping the two implementations distinguishable.
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        principal: UserId,
        sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some((sql.to_string(), Vec::new()));
        Ok(TabularResult {
            columns: vec![col("n")],
            rows: vec![vec![serde_json::json!(marker(principal))]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }

    /// Overridden rather than inherited, and that is a decision.
    ///
    /// `QueryExecPort::execute_as_with_params` is defaulted so the trait's nine
    /// hand-written fakes keep compiling, and the default REFUSES a non-empty
    /// `params` instead of quietly ignoring it — a fake that returns canned rows
    /// while dropping the values on the floor would make every parameterised
    /// assertion pass for no reason. So this fixture opts in explicitly, and
    /// what it returns depends on the values it was given.
    async fn execute_as_with_params(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        principal: UserId,
        sql: &str,
        params: &[Param],
        _limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some((sql.to_string(), params.to_vec()));
        Ok(TabularResult {
            columns: vec![col("n")],
            rows: vec![vec![serde_json::json!(
                marker(principal).wrapping_add(bind_signature(params))
            )]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }
}

// ---------------------------------------------------------------------------
// A rate-limit store on a clock the test drives.
// ---------------------------------------------------------------------------

/// Wraps the real [`TokenBucket`] and substitutes a clock the test controls for
/// the `now` the handler reads.
///
/// The handler calls `Instant::now()` itself, which is correct in production and
/// makes an integration test a race: `{per_key_qps: 1}` means a call that must
/// be refused turns into a 200 the moment more than a second passes between two
/// `oneshot`s — on a loaded CI box, or behind one slow query. Those are exactly
/// the assertions that get `#[ignore]`d after the third flake, and an ignored
/// rate-limit test is no rate limit at all.
///
/// Freezing the clock also buys the assertion a wall clock cannot make without
/// sleeping: advance it and the SAME client is admitted again, which is the
/// refill that separates this limiter from a lockout.
///
/// The bucket ITSELF is the real one — only the clock is substituted, so this
/// still measures the shipped implementation.
pub struct TestClock {
    inner: egret_dataapi::serving::TokenBucket,
    now: Mutex<Instant>,
}

impl TestClock {
    pub fn new() -> Self {
        Self {
            inner: egret_dataapi::serving::TokenBucket::new(),
            now: Mutex::new(Instant::now()),
        }
    }

    /// Move the clock forward. Every instant this store hands the bucket is
    /// derived from one starting point plus explicit advances, so no two of them
    /// can accidentally coincide.
    pub fn advance(&self, by: Duration) {
        let mut g = self.now.lock().unwrap();
        *g += by;
    }
}

impl egret_dataapi::serving::RateLimitStore for TestClock {
    fn check_at(
        &self,
        key: &str,
        rate: egret_dataapi::serving::Rate,
        _now: Instant,
    ) -> Result<(), Duration> {
        let now = *self.now.lock().unwrap();
        self.inner.check_at(key, rate, now)
    }
}

// ---------------------------------------------------------------------------
// The IdP key, and tokens minted against it.
// ---------------------------------------------------------------------------

fn active_key() -> &'static KeyMaterial {
    static KEY: OnceLock<KeyMaterial> = OnceLock::new();
    KEY.get_or_init(|| generate_rsa_key("egret-active").expect("generate key"))
}

pub struct FakeKeys;

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

/// P2's claim shape, with an EXPLICIT `sub`.
///
/// `sub` is the client id, and the rate limiter's per-client bucket is keyed on
/// it — so a fixture that minted a random `sub` per call (as `serving_routing`'s
/// does, correctly, since it has no limits to trip) would hand every request a
/// fresh bucket and make every rate-limit test pass for the wrong reason.
pub fn mint(ws: Uuid, user: UserId, client_id: &str) -> String {
    sign_claims(
        active_key(),
        &serde_json::json!({
            "iss": "https://egret.test",
            "sub": client_id,
            "aud": "data-api",
            "exp": chrono::Utc::now().timestamp() + 600,
            "ws": ws.to_string(),
            "egret_user_id": user.0.to_string(),
        }),
    )
    .expect("sign")
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

pub struct Fixture {
    pub db: Db,
    pub serving: axum::Router,
    pub mgmt: axum::Router,
    pub session: String,
    pub ws_slug: String,
    pub ws_id: Uuid,
    /// Two service accounts, whose Ranger-equivalent grants differ.
    pub principal_a: UserId,
    pub principal_b: UserId,
    pub admin: Uuid,
    /// `delegated = false` — the connection a `shared` cache may be published
    /// against.
    pub database_id: Uuid,
    /// `delegated = true` — where a `shared` cache is refused at publish time.
    pub delegated_database_id: Uuid,
    /// **`delegated = false`, `options.impersonate = true`.** The configuration
    /// that reads as shared-safe to anything consulting `delegated` alone, while
    /// the connector still runs every query as the caller
    /// (`connector/trino.rs:43`, `connector/starrocks.rs:129`). This is the
    /// connection the cross-principal leak lived on.
    pub impersonating_database_id: Uuid,
    pub port: Arc<FakePort>,
    /// The rate limiter's clock. Frozen unless a test advances it.
    pub clock: Arc<TestClock>,
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

async fn insert_database(db: &Db, ws: Uuid, owner: Uuid, delegated: bool) -> Uuid {
    insert_database_full(db, ws, owner, delegated, false).await
}

/// Both halves of the pair that decides whether the engine runs per principal.
///
/// `options` is written as raw JSON with the key spelled exactly as
/// `egret_query_api::ConnOptions::impersonate` serialises it — the same string the
/// production code reads through `lifecycle::impersonate_flag`. Written here
/// rather than through the databases route so a test can construct the
/// combination the route accepts but nothing validates.
async fn insert_database_full(
    db: &Db,
    ws: Uuid,
    owner: Uuid,
    delegated: bool,
    impersonate: bool,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases \
           (workspace_id, name, engine, host, port, delegated, options, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, $3, $4, $5) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(delegated)
    .bind(serde_json::json!({ "impersonate": impersonate }))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

async fn grant(db: &Db, user: Uuid, ws: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_t4_{}", Uuid::new_v4().simple()))
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
/// [`Fixture::publish_with_params`] issues it for BOTH principals, because these
/// two suites are about steps 5 and 6 and every call in them is meant to reach
/// the limiter and the cache. Step 4 itself is `serving_authz.rs`'s subject, and
/// it uses the real `POST …/endpoints/{id}/grants` route; this is SQL because
/// these principals are plain `users` rows with no `dataapi.service_accounts`
/// binding, which that route (correctly) refuses.
pub async fn grant_invoke(db: &Db, user: UserId, endpoint: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect) \
         SELECT 'user', $1, r.id, 'object', 'data_api_endpoint', $2, 'allow' \
           FROM roles r WHERE r.scope = 'workspace' AND r.name = 'data_api_invoker' \
         ON CONFLICT DO NOTHING",
    )
    .bind(user.0)
    .bind(endpoint)
    .execute(&db.pool)
    .await
    .expect("grant invoke");
}

pub async fn fixture() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let admin = insert_user(&db, "t4_admin").await;
    let slug = format!("t4-{}", Uuid::new_v4().simple());
    let ws_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Data API Task 4', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let principal_a = UserId(insert_user(&db, "t4_sa_a").await);
    let principal_b = UserId(insert_user(&db, "t4_sa_b").await);
    let database_id = insert_database(&db, ws_id, admin, false).await;
    let delegated_database_id = insert_database(&db, ws_id, admin, true).await;
    let impersonating_database_id = insert_database_full(&db, ws_id, admin, false, true).await;

    let author = insert_user(&db, "t4_author").await;
    grant(
        &db,
        author,
        ws_id,
        &["data_api_endpoint:read", "data_api_endpoint:manage"],
    )
    .await;

    // The REAL PDP, built after the grant (`CedarPdp` snapshots policies at
    // build time).
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let port = Arc::new(FakePort::default());
    let clock = Arc::new(TestClock::new());
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
        query: port.clone(),
        keys: Arc::new(FakeKeys),
        issuer: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        // ONE cache and ONE set of buckets for the whole fixture — built per
        // request they would be a cache that never hits and a limit nobody
        // reaches, and every test here would pass vacuously.
        cache: egret_dataapi::serving::ResponseCache::new(),
        rate_limits: clock.clone(),
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
        ws_slug: slug,
        ws_id,
        principal_a,
        principal_b,
        admin,
        database_id,
        delegated_database_id,
        impersonating_database_id,
        port,
        clock,
    })
}

pub struct Res {
    pub status: StatusCode,
    pub body: serde_json::Value,
    pub retry_after: Option<String>,
}

impl Res {
    /// The first cell of every row — enough to say WHOSE rows these are.
    pub fn ns(&self) -> Vec<i64> {
        self.body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("no data array in {}", self.body))
            .iter()
            .map(|row| match row {
                serde_json::Value::Object(m) => m.values().next().unwrap().as_i64().unwrap(),
                serde_json::Value::Array(a) => a[0].as_i64().unwrap(),
                other => panic!("unexpected row {other}"),
            })
            .collect()
    }

    pub fn cached(&self) -> bool {
        self.body["meta"]["cached"].as_bool().unwrap_or_else(|| {
            panic!("no meta.cached in {}", self.body);
        })
    }
}

impl Fixture {
    /// A call as `principal`, with a client id derived from it.
    pub async fn call_as(&self, principal: UserId, path: &str) -> Res {
        self.call_as_client(principal, &format!("client-{}", principal.0.simple()), path)
            .await
    }

    /// A call as `principal` presenting `client_id` — the pair the two keys are
    /// built from: the cache keys on the principal, the rate limiter on the
    /// client.
    pub async fn call_as_client(&self, principal: UserId, client_id: &str, path: &str) -> Res {
        let token = mint(self.ws_id, principal, client_id);
        let uri = format!("/data-api/v1/{}/{path}", self.ws_slug);
        let res = self
            .serving
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
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
        Res {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
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

    /// Insert a draft carrying `cache`/`rate_limit` and publish it through the
    /// REAL publish route, so the frozen snapshot is the one publish produces —
    /// including its `(delegated, impersonate)` witness pair, which is what the
    /// shared-cache check reads. Both halves come from publish, so a fixture
    /// cannot accidentally freeze a pair the real path would not.
    pub async fn publish(
        &self,
        path: &str,
        database_id: Uuid,
        cache: serde_json::Value,
        rate_limit: serde_json::Value,
    ) -> Uuid {
        self.publish_with_params(
            path,
            database_id,
            cache,
            rate_limit,
            "SELECT 1 AS n",
            serde_json::json!([]),
        )
        .await
    }

    /// The same, with declared `params` and the SQL that references them.
    ///
    /// P3's `check_params` refuses to publish a parameter no placeholder
    /// references, so the two arguments move together — a fixture that declared
    /// a parameter without using it would fail at publish, not at the assertion
    /// under test.
    pub async fn publish_with_params(
        &self,
        path: &str,
        database_id: Uuid,
        cache: serde_json::Value,
        rate_limit: serde_json::Value,
        sql: &str,
        params: serde_json::Value,
    ) -> Uuid {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, sql, params, cache, rate_limit, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(database_id)
        .bind(sql)
        .bind(&params)
        .bind(&cache)
        .bind(&rate_limit)
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
        // Both principals, so a cache or rate-limit test comparing two
        // identities is comparing what it thinks it is rather than one 200
        // against one 403.
        grant_invoke(&self.db, self.principal_a, ep).await;
        grant_invoke(&self.db, self.principal_b, ep).await;
        ep
    }

    /// Publish without asserting success, so a test can assert the REFUSAL.
    pub async fn try_publish(
        &self,
        path: &str,
        database_id: Uuid,
        cache: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, sql, cache, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, 'SELECT 1 AS n', $5, $6) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(database_id)
        .bind(&cache)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft");
        self.mgmt_send(
            "POST",
            &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", self.ws_slug),
            Some(serde_json::json!({})),
        )
        .await
    }

    /// Switch `options.impersonate` on a live connection AFTER a version was
    /// published against it — the other half of the pair, and the same
    /// no-re-validation window `set_database_delegated` opens.
    ///
    /// Writes the whole `options` object, as the databases route does, rather
    /// than patching a key: a test that only ever saw a jsonb_set path could
    /// pass against a production writer that replaces the object.
    pub async fn set_database_impersonate(&self, database_id: Uuid, impersonate: bool) {
        sqlx::query("UPDATE query.databases SET options = $1 WHERE id = $2")
            .bind(serde_json::json!({ "impersonate": impersonate }))
            .bind(database_id)
            .execute(&self.db.pool)
            .await
            .expect("flip impersonate");
    }

    /// Flip the live `delegated` column AFTER a version was published against
    /// it. Nothing re-validates when this happens — which is the entire reason
    /// the snapshot freezes a witness.
    pub async fn set_database_delegated(&self, database_id: Uuid, delegated: bool) {
        sqlx::query("UPDATE query.databases SET delegated = $1 WHERE id = $2")
            .bind(delegated)
            .bind(database_id)
            .execute(&self.db.pool)
            .await
            .expect("flip delegated");
    }
}
