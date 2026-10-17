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
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no `DATABASE_URL` FAILS, so check the
//! DURATION, not just the `ok`.

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::{ColumnMeta, Error, TabularResult, UserId};
use egret_dataapi::{
    management_router, serving_router, ActivityStore, ClientStore, DataApiState, EndpointStore,
};
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

/// `(SQL needle, column names)` arms, first match wins — the table
/// [`FakePort::set_columns_per_sql`] installs.
type PerSqlColumns = Vec<(String, Vec<String>)>;

/// Counts executions, so a test can tell a cache HIT from a re-execution that
/// happened to return the same rows — and records what the last one was ASKED to
/// run, so a test can assert on the rendered statement rather than only on the
/// rows it produced.
#[derive(Default)]
pub struct FakePort {
    pub executions: AtomicUsize,
    pub last: Mutex<Option<(String, Vec<Param>)>>,
    /// The `(max_rows, timeout)` the last execution was given.
    ///
    /// Recorded because a caller's row cap is otherwise invisible in the body: a
    /// fake that returns one row looks identical whether it was told 100 or
    /// 10 000, and "the preview cap is fixed rather than the endpoint's" is a
    /// statement about exactly this argument.
    pub last_limits: Mutex<Option<(usize, Duration)>>,
    /// Set by [`FakePort::fail_next_with`] / [`FakePort::fail_next_internal`];
    /// consumed by the next `execute_as`/`execute_as_with_params` call.
    next_error: Mutex<Option<Error>>,
    /// The column names the next executions return, when a test needs names a
    /// naming transform actually CHANGES.
    ///
    /// `None` — the default every existing assertion in these suites was written
    /// against — means the single column `n`. `n` is a name `apply_naming` maps to
    /// itself under every mode, so a test about `output.naming` being applied (or
    /// not) is vacuous with it: `camel("n") == "n"`.
    columns: Mutex<Option<Vec<String>>>,
    /// The column names to return per RENDERED statement — see
    /// [`FakePort::set_columns_per_sql`]. `None` (the default) leaves every
    /// existing assertion in these suites on the two paths above.
    per_sql: Mutex<Option<PerSqlColumns>>,
    /// The column names EXECUTION returns per rendered statement, when they must
    /// differ from the ones DESCRIBE reported — see
    /// [`FakePort::set_execution_columns_per_sql`].
    exec_per_sql: Mutex<Option<PerSqlColumns>>,
    /// Every execution's `(max_rows, timeout)`, in order.
    ///
    /// [`Self::last_limits`] keeps only the most recent, which cannot answer the
    /// question M2 asks: whether the SECOND statement of an endpoint was handed
    /// the same row cap as the first, or a budget the first had already spent.
    /// One call, several executions, so the answer is a LIST.
    limits_log: Mutex<Vec<(usize, Duration)>>,
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

    /// The `(max_rows, timeout)` of the most recent execution.
    pub fn last_limits(&self) -> (usize, Duration) {
        self.last_limits
            .lock()
            .unwrap()
            .expect("no execution has reached the port")
    }

    /// The `(max_rows, timeout)` of EVERY execution, in the order they ran.
    ///
    /// A per-statement row cap is invisible in the response body — a fake that
    /// returns one row per statement looks identical whether the second was told
    /// the endpoint's cap or the cap minus what the first returned — so this
    /// list is the only instrument that can tell the two apart.
    pub fn limits_seen(&self) -> Vec<(usize, Duration)> {
        self.limits_log.lock().unwrap().clone()
    }

    /// Make the NEXT `execute_as`/`execute_as_with_params` call fail, carrying
    /// `msg` on `egret_core::Error::InvalidInput`.
    ///
    /// **`InvalidInput`, not `Internal`, and the variant is the whole point.**
    /// `port_impl::engine_error` folds every QUERY-level engine failure —
    /// `EngineError::Engine`, which is what a Trino denial inside a 200
    /// (`connector/trino.rs:334`) and every StarRocks failure arrive as, plus
    /// `BadRequest` and `Limit` — into `Error::InvalidInput` carrying the
    /// engine's own words. `Internal` is what it produces for INFRASTRUCTURE
    /// and, crucially, for egret's own faults in `connect_as`. A fake that hands
    /// the serving path `Internal` here is modelling the wrong half of that
    /// table, and it is why `serving::router::engine_error` used to
    /// message-match egret's own failures. See [`FakePort::fail_next_internal`]
    /// for the other half, which now has its own test.
    ///
    /// One-shot and consumed BEFORE `executions`/`last` are touched: a failed
    /// call never rendered a statement worth recording, and leaving the flag
    /// set would fail every subsequent call in a test that only meant to
    /// trip one.
    ///
    /// This only proves the MAPPING (a message containing e.g. `"Access
    /// Denied"` becomes `data_access_denied`), not that the message is one a real
    /// engine sends — this fake returns whatever string it is given. See
    /// `is_permission_denied`'s own doc for what verifying THAT would take.
    pub fn fail_next_with(&self, msg: impl Into<String>) {
        *self.next_error.lock().unwrap() = Some(Error::InvalidInput(msg.into()));
    }

    /// Make the NEXT execution fail with `egret_core::Error::Internal` — the
    /// shape `port_impl::connect_as` produces for EGRET's own faults (resolving
    /// the principal's email, opening the database secret) as well as for an
    /// unreachable coordinator. Same one-shot semantics as
    /// [`FakePort::fail_next_with`].
    pub fn fail_next_internal(&self, msg: impl Into<String>) {
        let msg = msg.into();
        *self.next_error.lock().unwrap() = Some(Error::Internal(anyhow::anyhow!(msg)));
    }

    fn take_next_error(&self) -> Option<Error> {
        self.next_error.lock().unwrap().take()
    }

    /// Make every following execution return these column names instead of the
    /// default single `n`. Sticky, not one-shot: a test asserting on names wants
    /// them for the whole test, and the fixture is per-test anyway.
    pub fn set_columns(&self, names: &[&str]) {
        *self.columns.lock().unwrap() = Some(names.iter().map(|n| (*n).to_string()).collect());
    }

    /// Answer with DIFFERENT columns per statement, matching a SUBSTRING of the
    /// rendered SQL; first arm wins, so the fallback goes last with `""`.
    ///
    /// Unlike [`FakePort::set_columns`], which only affects execution, this
    /// drives the DESCRIBE paths too — and that is the whole point. A branching
    /// definition is described once per branch at publish and executed on
    /// exactly one branch per call, so a fake that hands the same columns to
    /// both cannot tell a frozen union apart from a single describe, and an
    /// end-to-end test written against it would pass with the union never
    /// computed. Modelled on `lifecycle.rs`'s `Describe::PerSql`, which does the
    /// same job for the publish suite.
    ///
    /// Panics rather than falling back when no arm matches: a statement the
    /// table did not anticipate is a branch the test did not know publish would
    /// render, and answering it with a default would hide exactly that.
    pub fn set_columns_per_sql(&self, table: &[(&str, &[&str])]) {
        *self.per_sql.lock().unwrap() = Some(
            table
                .iter()
                .map(|(needle, names)| {
                    (
                        (*needle).to_string(),
                        names.iter().map(|n| (*n).to_string()).collect(),
                    )
                })
                .collect(),
        );
    }

    /// Answer an EXECUTION with different columns than the DESCRIBE that froze
    /// the contract — the only way to drive a drift into ONE statement of
    /// several.
    ///
    /// [`FakePort::set_columns_per_sql`] drives describe and execute alike, so a
    /// table installed before publish freezes exactly what it later serves and
    /// can never drift; [`FakePort::set_columns`] drifts EVERY statement at
    /// once, since it is keyed on nothing. Neither can express "the second
    /// statement's table was altered after publish", which is the case M2 has to
    /// refuse — and refusing it for the wrong statement, or not at all, are both
    /// silent.
    ///
    /// Installed AFTER publish, like `set_columns`. Same first-match-wins arms,
    /// same panic when none matches.
    pub fn set_execution_columns_per_sql(&self, table: &[(&str, &[&str])]) {
        *self.exec_per_sql.lock().unwrap() = Some(
            table
                .iter()
                .map(|(needle, names)| {
                    (
                        (*needle).to_string(),
                        names.iter().map(|n| (*n).to_string()).collect(),
                    )
                })
                .collect(),
        );
    }

    /// The execution-only per-statement answer, or `None` when no table was
    /// installed. Same shape as [`Self::per_sql_columns`], a different table.
    fn exec_per_sql_columns(&self, sql: &str) -> Option<Vec<ColumnMeta>> {
        let guard = self.exec_per_sql.lock().unwrap();
        let table = guard.as_ref()?;
        let names = table
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .unwrap_or_else(|| panic!("set_execution_columns_per_sql has no arm matching {sql:?}"))
            .1
            .clone();
        Some(names.iter().map(|n| col(n)).collect())
    }

    /// The per-statement answer, or `None` when no table was installed.
    ///
    /// Returns owned columns and drops the guard before it returns, so a caller
    /// may fall back to [`FakePort::out_columns`] — which takes a different lock
    /// — without nesting the two.
    fn per_sql_columns(&self, sql: &str) -> Option<Vec<ColumnMeta>> {
        let guard = self.per_sql.lock().unwrap();
        let table = guard.as_ref()?;
        let names = table
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .unwrap_or_else(|| panic!("set_columns_per_sql has no arm matching {sql:?}"))
            .1
            .clone();
        Some(names.iter().map(|n| col(n)).collect())
    }

    fn out_columns(&self) -> Vec<ColumnMeta> {
        match self.columns.lock().unwrap().as_ref() {
            Some(names) => names.iter().map(|n| col(n)).collect(),
            None => vec![col("n")],
        }
    }

    /// What an EXECUTION of `sql` returns: the per-statement table when one is
    /// installed, otherwise the suite's existing sticky/default columns.
    fn out_columns_for(&self, sql: &str) -> Vec<ColumnMeta> {
        if let Some(c) = self.exec_per_sql_columns(sql) {
            return c;
        }
        self.per_sql_columns(sql)
            .unwrap_or_else(|| self.out_columns())
    }

    /// What a DESCRIBE of `sql` returns. Without a per-statement table this is
    /// the single column `n` every other test in these suites was written
    /// against — deliberately NOT `out_columns`, so installing `set_columns`
    /// still changes only what execution returns.
    fn describe_columns(&self, sql: &str) -> Vec<ColumnMeta> {
        self.per_sql_columns(sql).unwrap_or_else(|| vec![col("n")])
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
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(self.describe_columns(sql))
    }

    /// Publish describes the RENDERED statement, which carries `?` markers, so
    /// it goes through this method rather than `describe_as`. Overridden rather
    /// than inherited for the same reason as `execute_as_with_params` below: the
    /// trait's default refuses a non-zero `param_count`, and every fixture in
    /// this suite publishes parameterised endpoints.
    async fn describe_as_with_params(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        sql: &str,
        _param_count: usize,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(self.describe_columns(sql))
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
        limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        if let Some(e) = self.take_next_error() {
            return Err(e);
        }
        self.executions.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some((sql.to_string(), Vec::new()));
        *self.last_limits.lock().unwrap() = Some((limits.max_rows, limits.timeout));
        self.limits_log
            .lock()
            .unwrap()
            .push((limits.max_rows, limits.timeout));
        let columns = self.out_columns_for(sql);
        Ok(TabularResult {
            // One cell per column, so a test that asked for extra columns still
            // gets a row the serving path's arity check accepts.
            rows: vec![vec![serde_json::json!(marker(principal)); columns.len()]],
            columns,
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
        limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        if let Some(e) = self.take_next_error() {
            return Err(e);
        }
        self.executions.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some((sql.to_string(), params.to_vec()));
        *self.last_limits.lock().unwrap() = Some((limits.max_rows, limits.timeout));
        self.limits_log
            .lock()
            .unwrap()
            .push((limits.max_rows, limits.timeout));
        let columns = self.out_columns_for(sql);
        Ok(TabularResult {
            rows: vec![vec![
                serde_json::json!(
                    marker(principal).wrapping_add(bind_signature(params))
                );
                columns.len()
            ]],
            columns,
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

#[async_trait::async_trait]
impl egret_dataapi::serving::RateLimitStore for TestClock {
    /// Substitutes the clock and NOTHING else: the arithmetic is
    /// `TokenBucket::take`'s, and the refusal is mapped through the same
    /// `Refusal::Exhausted` the shipped in-process store answers. A `Duration`
    /// invented here would let these suites assert a wire mapping the product
    /// does not have.
    async fn check_at(
        &self,
        key: &str,
        rate: egret_dataapi::serving::Rate,
        _now: Instant,
    ) -> Result<(), egret_dataapi::serving::Refusal> {
        let now = *self.now.lock().unwrap();
        self.inner
            .take(key, rate, now)
            .map_err(egret_dataapi::serving::Refusal::Exhausted)
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

/// A management caller holding a specific permission set — see
/// [`Fixture::caller_with`]. Carries its OWN router because it carries its own
/// PDP: `CedarPdp::build` snapshots policies, so a principal granted after the
/// fixture was built is invisible to the fixture's own PDP.
pub struct Caller {
    /// The `users.id` the session resolves to.
    pub user: Uuid,
    app: axum::Router,
    session: String,
}

impl Caller {
    /// A GET through this caller's own router, carrying its session cookie.
    pub async fn get(&self, path: &str) -> (StatusCode, serde_json::Value) {
        let res = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .header("Cookie", format!("egret_session={}", self.session))
                    .body(Body::empty())
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

    /// A POST through this caller's own router, carrying its session cookie.
    ///
    /// The mirror of [`Caller::get`], and it exists for the same reason: a gate
    /// on a POST route can only be measured by a principal holding exactly the
    /// permission set under test, and [`Fixture::mgmt_post`]'s session belongs to
    /// `author`, who holds `:read` AND `:manage`.
    pub async fn post(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method("POST")
            .uri(path)
            .header("Cookie", format!("egret_session={}", self.session));
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let res = self
            .app
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
}

pub struct Fixture {
    pub db: Db,
    pub serving: axum::Router,
    pub mgmt: axum::Router,
    /// The same state the two routers above were built from — cloned before
    /// `.with_state` consumed it. Lets a test reach a `pub(crate)` expression
    /// like [`egret_dataapi::can_invoke_for_test`] directly, without going
    /// through HTTP for a question that isn't about routing.
    pub state: DataApiState,
    pub session: String,
    /// The `users.id` behind [`Fixture::session`] — the human every management
    /// call in these suites is made AS. Exposed because a Try-it trial records
    /// the person who ran it in `access_log.actor_user_id`, and asserting that
    /// column needs the id the session resolves to rather than a second user
    /// that merely also exists.
    pub author: Uuid,
    pub ws_slug: String,
    pub ws_id: Uuid,
    /// Two service accounts, whose Ranger-equivalent grants differ.
    pub principal_a: UserId,
    pub principal_b: UserId,
    pub admin: Uuid,
    /// The endpoint the fixture publishes, at `/probe`. `f.admin` is a
    /// `ws_admin` member of `ws_id` and holds no object-scoped grant naming
    /// this endpoint — see `grant_ws_admin` below.
    pub endpoint_id: Uuid,
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

/// Make `user` a `ws_admin` of `ws` — a WORKSPACE-scoped `access_grants` row on
/// the real, migration-seeded role, written directly rather than through
/// `workspace_members` for the same reason [`grant_invoke`] writes
/// `access_grants` directly: `AccessGrantStore::grants_for` reads this table,
/// not the legacy one the `sync_workspace_grant` trigger mirrors from.
///
/// This is what lets `f.admin` reach `data_api_endpoint:invoke` on every
/// endpoint in `ws` through the Cedar workspace->object cascade (migration
/// 0124) while owning zero `scope_type = 'object'` rows — the distinction
/// `can_invoke`'s doc comment exists to explain.
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

/// Migrations are applied ONCE per test process, not once per test.
///
/// `db.migrate()` is idempotent, but a no-op run still reads and checksums every
/// migration in `migrations/` — measured at **1.24s** against an already-migrated
/// database. `fixture()` is called by every test in the binary, so this binary's
/// 27 tests were paying ~33s of a 91s run to re-verify migrations that could not
/// have changed since the process started.
pub async fn fixture() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");

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
        &[
            "data_api_endpoint:read",
            "data_api_endpoint:manage",
            // Needed for `Fixture::create_service_account`, which goes through
            // the REAL `POST …/service-accounts` route — gated on `:manage`,
            // never `:read` (issuing/creating mutates the authorization graph).
            "service_account:manage",
        ],
    )
    .await;
    // `admin` gets NO object-scoped grant on anything — it reaches invoke (once
    // an endpoint is published below) purely through this workspace-scoped
    // `ws_admin` membership and the Cedar cascade migration 0124 put on it.
    grant_ws_admin(&db, admin, ws_id).await;

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
        // Deliberately the SAME string as `issuer` — this is the state a
        // deployment with `data_api.public_url` UNSET boots into, so every
        // assertion in these suites that names `https://egret.test` is
        // pinning today's output. A test that wants the two to differ
        // rebuilds the routers through `Fixture::with_consumer_base_url`;
        // moving this line instead would silently retarget every suite.
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        // ONE cache and ONE set of buckets for the whole fixture — built per
        // request they would be a cache that never hits and a limit nobody
        // reaches, and every test here would pass vacuously.
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: clock.clone(),
        // A fixed, sensible retention rather than a config read — nothing in
        // this fixture boots `egret-config`.
        activity: ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
    };

    let session = st
        .sessions
        .create(egret_core::ids::UserId(author), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    // Captured before `.with_state(st)` below consumes it.
    let state = st.clone();

    let mut f = Fixture {
        db,
        serving: axum::Router::new()
            .nest("/data-api/v1", serving_router())
            .with_state(state.clone()),
        mgmt: axum::Router::new()
            .nest("/api", management_router())
            .with_state(st),
        state,
        session,
        author,
        ws_slug: slug,
        ws_id,
        principal_a,
        principal_b,
        admin,
        // Set for real just below, once the fixture can publish through its own
        // `mgmt` router.
        endpoint_id: Uuid::nil(),
        database_id,
        delegated_database_id,
        impersonating_database_id,
        port,
        clock,
    };
    f.endpoint_id = f
        .publish(
            "/probe",
            database_id,
            serde_json::json!({ "enabled": false }),
            serde_json::json!({}),
        )
        .await;

    Some(f)
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
    /// Rebuild both routers with a DIFFERENT consumer-facing base URL, leaving
    /// `issuer` where it is.
    ///
    /// The two must be able to disagree for any assertion about them to mean
    /// anything: with the fixture's default (the two equal, i.e. the key unset)
    /// a change that ignored `consumer_base_url` entirely would still produce
    /// `https://egret.test` everywhere and pass. It rebuilds rather than mutates
    /// because `.with_state` has already consumed the state the live routers
    /// hold — assigning to `self.state` alone would change nothing a request
    /// can see, and the test would pass for the wrong reason.
    pub fn with_consumer_base_url(mut self, url: &str) -> Self {
        self.state.consumer_base_url = url.to_string();
        self.serving = axum::Router::new()
            .nest("/data-api/v1", serving_router())
            .with_state(self.state.clone());
        self.mgmt = axum::Router::new()
            .nest("/api", management_router())
            .with_state(self.state.clone());
        self
    }

    /// A call as `principal`, with a client id derived from it.
    pub async fn call_as(&self, principal: UserId, path: &str) -> Res {
        self.call_as_client(principal, &format!("client-{}", principal.0.simple()), path)
            .await
    }

    /// A call as `principal` presenting `client_id` — the pair the two keys are
    /// built from: the cache keys on the principal, the rate limiter on the
    /// client.
    pub async fn call_as_client(&self, principal: UserId, client_id: &str, path: &str) -> Res {
        self.call_as_client_with_headers(principal, client_id, path, &[])
            .await
    }

    /// The same consumer call, plus arbitrary request headers.
    ///
    /// Exists for the headers a caller could INVENT: `actor_user_id` is a fact
    /// only because a `TrialActor` extension cannot be set from outside the
    /// process, and the way that stops being true is somebody reading the actor
    /// from a header. A test that can send one is the only thing that notices.
    pub async fn call_as_client_with_headers(
        &self,
        principal: UserId,
        client_id: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Res {
        let token = mint(self.ws_id, principal, client_id);
        let uri = format!("/data-api/v1/{}/{path}", self.ws_slug);
        let mut builder = Request::builder()
            .method("GET")
            .uri(&uri)
            .header("authorization", format!("Bearer {token}"));
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let res = self
            .serving
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
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

    /// A consumer call whose `Authorization` header is supplied VERBATIM, so a
    /// test can send one that will not verify.
    ///
    /// [`Fixture::call_as_client_with_headers`] always mints a valid token, so
    /// nothing built on it can measure what happens BEFORE authentication
    /// succeeds — and "this refusal is reachable without a credential" is a
    /// statement only an unauthenticated request can settle.
    pub async fn call_with_raw_authorization(
        &self,
        path: &str,
        authorization: &str,
        headers: &[(&str, &str)],
    ) -> Res {
        let uri = format!("/data-api/v1/{}/{path}", self.ws_slug);
        let mut builder = Request::builder()
            .method("GET")
            .uri(&uri)
            .header("authorization", authorization);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let res = self
            .serving
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
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

    /// Package 7: the same consumer call, carrying `X-Egret-Version`.
    ///
    /// `version` is a `&str` and not an `i32` on purpose — half of what the pin
    /// has to refuse is a value no integer type can express (`+1`, `1 `, `abc`,
    /// the empty string), and a typed parameter would have made those tests
    /// impossible to write without going around this helper.
    pub async fn call_pinned(&self, principal: UserId, path: &str, version: &str) -> Res {
        self.call_as_client_with_headers(
            principal,
            &format!("client-{}", principal.0.simple()),
            path,
            &[("x-egret-version", version)],
        )
        .await
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

    /// A GET through the management router, with the session cookie
    /// [`Fixture::mgmt_send`] already attaches.
    pub async fn mgmt_get(&self, path: &str) -> (StatusCode, serde_json::Value) {
        self.mgmt_send("GET", path, None).await
    }

    /// The same GET, reporting `Cache-Control` instead of the body.
    ///
    /// A separate helper rather than a wider return type on [`Fixture::mgmt_get`]
    /// because exactly one management response has a header worth asserting on —
    /// the generated OpenAPI document, which is per service account and must not
    /// be stored (see `routes::get_openapi`). Widening every caller's tuple to
    /// carry a `None` would be noise.
    pub async fn mgmt_get_cache_control(&self, path: &str) -> (StatusCode, Option<String>) {
        let res = self
            .mgmt
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .header("Cookie", format!("egret_session={}", self.session))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cache_control = res
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        (res.status(), cache_control)
    }

    /// A POST through the management router, with the session cookie
    /// [`Fixture::mgmt_send`] already attaches. Needed to republish an
    /// EXISTING endpoint id a second time (producing version 2) rather than
    /// creating a fresh one, which [`Fixture::publish`] always does.
    pub async fn mgmt_post(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        self.mgmt_send("POST", path, body).await
    }

    /// Insert a row directly into `dataapi.access_log`, bypassing the real
    /// serving path, so a test can shape a population — three successes and
    /// seven failures, say — without driving ten real requests through it.
    ///
    /// Attributed to `f.endpoint_id` (the `/probe` endpoint `fixture()`
    /// publishes) at version 1 — the version `ActivityStore::summary`'s
    /// `cacheable_calls` JOIN reads `dataapi.endpoint_versions` through.
    /// `actor_user_id` is left NULL: this is a real call, not a trial — see
    /// [`Fixture::log_row_as_trial`] for that.
    ///
    /// A thin specialisation of [`Fixture::log_row_for`], fixed to the
    /// fixture's own endpoint at its only version — see that function for a
    /// row against an EXPLICIT endpoint/version, which this can't produce.
    pub async fn log_row(
        &self,
        status_code: i32,
        duration_ms: i32,
        cache_hit: bool,
        error_code: Option<&str>,
    ) {
        self.log_row_for(
            Some(self.endpoint_id),
            Some(1),
            status_code,
            duration_ms,
            cache_hit,
            error_code,
        )
        .await
    }

    /// The general form: an explicit `endpoint_id`/`version` rather than
    /// always the fixture's own `/probe` at its only version.
    ///
    /// Exists for two shapes [`Fixture::log_row`] cannot produce: a row that
    /// never resolved ANY endpoint (`endpoint_id: None` — the unmatched-404
    /// shape `access_log.rs` describes, which carries a NULL `endpoint_id`
    /// exactly like the grand total does — see `ActivityStore::summary`'s
    /// `grouping()` doc comment for why telling the two apart is the point of
    /// that query), and a row against one of SEVERAL versions of one endpoint
    /// (needed to prove `cacheable_calls` joins the version the row recorded
    /// rather than the endpoint's current config).
    pub async fn log_row_for(
        &self,
        endpoint_id: Option<Uuid>,
        version: Option<i32>,
        status_code: i32,
        duration_ms: i32,
        cache_hit: bool,
        error_code: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO dataapi.access_log \
               (workspace_id, endpoint_id, version, status_code, duration_ms, \
                cache_hit, error_code, request_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(self.ws_id)
        .bind(endpoint_id)
        .bind(version)
        .bind(status_code)
        .bind(duration_ms)
        .bind(cache_hit)
        .bind(error_code)
        .bind(Uuid::new_v4().to_string())
        .execute(&self.db.pool)
        .await
        .expect("insert access_log row");
    }

    /// The same, but as a Try-it trial run by `actor` — excluded from every
    /// statistic `ActivityStore::summary` computes (spec §4.2), and shown only
    /// in the call list.
    pub async fn log_row_as_trial(&self, status_code: i32, duration_ms: i32, actor: Uuid) {
        sqlx::query(
            "INSERT INTO dataapi.access_log \
               (workspace_id, endpoint_id, version, status_code, duration_ms, \
                actor_user_id, request_id) \
             VALUES ($1, $2, 1, $3, $4, $5, $6)",
        )
        .bind(self.ws_id)
        .bind(self.endpoint_id)
        .bind(status_code)
        .bind(duration_ms)
        .bind(actor)
        .bind(Uuid::new_v4().to_string())
        .execute(&self.db.pool)
        .await
        .expect("insert access_log row");
    }

    /// A second workspace the fixture's session user holds NO membership in —
    /// deliberately, so a row planted in it is a row the caller cannot see
    /// through any grant. That is what makes
    /// `a_request_id_lookup_cannot_read_another_workspace` a test of the
    /// `workspace_id = $1` predicate rather than of a grant that happens to be
    /// missing.
    pub async fn make_workspace(&self, slug: &str) -> Uuid {
        let owner = insert_user(&self.db, "t5_other_owner").await;
        sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Data API Task 5 Other', $2, 'active') RETURNING id",
        )
        .bind(format!("{slug}-{}", Uuid::new_v4().simple()))
        .bind(owner)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert workspace")
    }

    /// [`Fixture::log_row_for`], but for an EXPLICIT workspace rather than the
    /// fixture's own `ws_id`. Needed only to plant a row in the workspace
    /// [`Fixture::make_workspace`] returns — every other test in this crate
    /// wants a row in the fixture's own workspace and uses `log_row`/
    /// `log_row_for` instead.
    pub async fn log_row_in(
        &self,
        ws: Uuid,
        status_code: i32,
        duration_ms: i32,
        request_id: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO dataapi.access_log \
               (workspace_id, status_code, duration_ms, request_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(ws)
        .bind(status_code)
        .bind(duration_ms)
        .bind(
            request_id
                .map(str::to_string)
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
        )
        .execute(&self.db.pool)
        .await
        .expect("insert access_log row");
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
               (workspace_id, name, path, method, database_id, statements, params, cache, rate_limit, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', $5::text)), $6, $7, $8, $9) RETURNING id",
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

    /// Publish an endpoint carrying an arbitrary STATEMENT LIST, and its own
    /// `limits`, through the real publish route.
    ///
    /// [`Fixture::publish_with_params`] builds the one-element list inline and
    /// takes no `limits`, so it can express neither of the two things M2 Task 7
    /// is about: an endpoint with several statements, and a row cap a test can
    /// recognise. `statements` is the jsonb the draft column stores —
    /// `[{"name": …, "sql": …}, …]` — so a test states the list exactly as an
    /// author would, and publish validates it exactly as it would theirs.
    ///
    /// Caching is off: every assertion here counts EXECUTIONS, and a warm entry
    /// would answer with none.
    pub async fn publish_statements(
        &self,
        path: &str,
        database_id: Uuid,
        statements: serde_json::Value,
        limits: serde_json::Value,
    ) -> Uuid {
        self.publish_statements_with_output(
            path,
            database_id,
            statements,
            limits,
            serde_json::json!({}),
        )
        .await
    }

    /// The same, with an explicit `output` spec.
    ///
    /// Separate from `publish_statements` rather than a fifth argument on it
    /// because `output.envelope` (M2 Task 8) is the only reason a several-
    /// statement fixture needs one, and every existing caller wants the default.
    pub async fn publish_statements_with_output(
        &self,
        path: &str,
        database_id: Uuid,
        statements: serde_json::Value,
        limits: serde_json::Value,
        output: serde_json::Value,
    ) -> Uuid {
        self.publish_statements_with_output_and_cache(
            path,
            database_id,
            statements,
            limits,
            output,
            serde_json::json!({ "enabled": false }),
        )
        .await
    }

    /// The same, with the CACHE config spelled out too.
    ///
    /// Exists for one assertion the two above cannot make: that a response a
    /// serving-side check REFUSED is not stored. Caching has to be on for that
    /// question to mean anything, and every existing caller of the two wrappers
    /// above wants it off because they count executions.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_statements_with_output_and_cache(
        &self,
        path: &str,
        database_id: Uuid,
        statements: serde_json::Value,
        limits: serde_json::Value,
        output: serde_json::Value,
        cache: serde_json::Value,
    ) -> Uuid {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, cache, limits, output, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(database_id)
        .bind(&statements)
        .bind(&cache)
        .bind(&limits)
        .bind(&output)
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
        grant_invoke(&self.db, self.principal_a, ep).await;
        grant_invoke(&self.db, self.principal_b, ep).await;
        ep
    }

    /// Publish an arbitrary statement list WITH declared parameters.
    ///
    /// The four wrappers above each take one of `params`, `limits`, `output`
    /// and `cache` and not the others; package S needs `statements` (to say
    /// `script` instead of `sql`) together with `params` (a scripted statement
    /// that reads none is not the thing under test) and `cache` (whether a HIT
    /// generates is one of the questions). So this one takes the three and
    /// leaves `limits` and `output` at their defaults.
    ///
    /// `statements` is the jsonb the draft column stores, stated exactly as an
    /// author would: `[{"name": "main", "script": "def build_sql(params): …"}]`.
    pub async fn publish_statements_with_params(
        &self,
        path: &str,
        database_id: Uuid,
        statements: serde_json::Value,
        params: serde_json::Value,
        cache: serde_json::Value,
    ) -> Uuid {
        let (ep, status, body) = self
            .try_publish_statements_with_params(path, database_id, statements, params, cache)
            .await;
        assert_eq!(status, StatusCode::OK, "publish failed: {body}");
        grant_invoke(&self.db, self.principal_a, ep).await;
        grant_invoke(&self.db, self.principal_b, ep).await;
        ep
    }

    /// The same, without asserting the publish succeeded — for the tests that
    /// are about what publish REFUSES. Returns the draft's id as well, so a
    /// refusal can be inspected against the row that produced it.
    pub async fn try_publish_statements_with_params(
        &self,
        path: &str,
        database_id: Uuid,
        statements: serde_json::Value,
        params: serde_json::Value,
        cache: serde_json::Value,
    ) -> (Uuid, StatusCode, serde_json::Value) {
        let ep: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, params, cache, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(format!("ep-{}", Uuid::new_v4().simple()))
        .bind(path)
        .bind(database_id)
        .bind(&statements)
        .bind(&params)
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
        (ep, status, body)
    }

    /// Rewrite the live version's frozen snapshot, in place.
    ///
    /// **A published version is immutable and this is the one thing that
    /// deliberately violates that**, because it is the only instrument that can
    /// answer "did serving run the script for THIS request?": generation has no
    /// observable side effect, so the way to measure that a cache hit skipped it
    /// is to make the script unrunnable between two identical calls and watch
    /// the second one still answer 200. A timing comparison could not tell a
    /// skipped evaluation from a fast one.
    ///
    /// `mutate` is handed the snapshot as JSON and returns the replacement.
    pub async fn rewrite_live_snapshot(
        &self,
        endpoint: Uuid,
        mutate: impl FnOnce(serde_json::Value) -> serde_json::Value,
    ) {
        let (version_id, snapshot): (Uuid, serde_json::Value) = sqlx::query_as(
            "SELECT v.id, v.snapshot FROM dataapi.endpoint_versions v \
               JOIN dataapi.endpoints e ON e.live_version_id = v.id \
              WHERE e.id = $1",
        )
        .bind(endpoint)
        .fetch_one(&self.db.pool)
        .await
        .expect("read the live snapshot");
        sqlx::query("UPDATE dataapi.endpoint_versions SET snapshot = $1 WHERE id = $2")
            .bind(mutate(snapshot))
            .bind(version_id)
            .execute(&self.db.pool)
            .await
            .expect("rewrite the live snapshot");
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
               (workspace_id, name, path, method, database_id, statements, cache, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1 AS n')), $5, $6) RETURNING id",
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

    /// Insert a draft through the REAL `POST …/endpoints` route and leave it
    /// unpublished (`state = 'draft'`) — used to prove a draft is excluded from
    /// the generated OpenAPI document, the same way it is excluded from serving.
    pub async fn create_draft_endpoint(&self, name: &str, path: &str) -> Uuid {
        let (status, body) = self
            .mgmt_post(
                &format!("/api/ws/{}/data-api/endpoints", self.ws_slug),
                Some(serde_json::json!({
                    "name": name,
                    "path": path,
                    "database_id": self.database_id,
                    "statements": [{ "name": "main", "sql": "SELECT 1 AS n" }],
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "create draft failed: {body}");
        body["id"].as_str().expect("id").parse().expect("uuid")
    }

    /// Create a real service account through the REAL `POST
    /// …/service-accounts` route, never a direct insert — so its `users.id`
    /// is exactly the kind of row `scoped_account` accepts, and a test that
    /// hands this id back to a `:read`-gated route is exercising the same
    /// containment check production traffic goes through. Returns the
    /// account's id (a `users.id`).
    pub async fn create_service_account(&self, name: &str) -> Uuid {
        let (status, body) = self
            .mgmt_post(
                &format!("/api/ws/{}/data-api/service-accounts", self.ws_slug),
                Some(serde_json::json!({ "name": name })),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "create service account failed: {body}"
        );
        body["id"].as_str().expect("id").parse().expect("uuid")
    }

    /// Make `user` a `ws_admin` of the fixture's own workspace — a public
    /// wrapper around the module-level `grant_ws_admin` used to seed `f.admin`,
    /// so a test can put a SECOND principal (e.g. one
    /// [`Fixture::create_service_account`] just minted) on the same cascade
    /// path without duplicating the `access_grants` insert.
    pub async fn grant_ws_admin(&self, user: Uuid) {
        grant_ws_admin(&self.db, user, self.ws_id).await
    }

    /// A management router + session for a BRAND-NEW human holding exactly
    /// `perms` in the fixture's workspace — the only way to measure a gate,
    /// since [`Fixture::mgmt_get`]'s own session belongs to `author`, who holds
    /// `data_api_endpoint:read` AND `:manage` and would therefore pass every
    /// gate this module has.
    ///
    /// **A FRESH `CedarPdp` is built after the role rows exist**, because
    /// `CedarPdp::build` snapshots policies. Reusing `self.state`'s PDP — built
    /// in `fixture()` — would answer from a snapshot taken before this role and
    /// silently deny everything, which reads exactly like a gate that works.
    /// Everything else is cloned from the fixture's state so the routes under
    /// test are the ones every other test in the suite drives.
    pub async fn caller_with(&self, perms: &[&str]) -> Caller {
        let user = insert_user(&self.db, "t4_gated").await;
        grant(&self.db, user, self.ws_id, perms).await;
        let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
            egret_auth::pdp::CedarPdp::build(self.db.clone(), &egret_events::EventBus::new())
                .await
                .expect("build CedarPdp"),
        );
        let mut st = self.state.clone();
        st.pdp = pdp;
        let session = st
            .sessions
            .create(egret_core::ids::UserId(user), "test", "127.0.0.1")
            .await
            .expect("create session")
            .token;
        let app = axum::Router::new()
            .nest("/api", management_router())
            .with_state(st);
        Caller { user, app, session }
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

    /// A real `query.databases` row that EXISTS, in a real workspace that is not
    /// `self.ws_id`.
    ///
    /// The point is that the row is present in the table: a random `Uuid` is
    /// refused by an implementation with no workspace scoping at all, because the
    /// row does not exist anywhere, so it cannot tell a scoped read from an
    /// unscoped one. This is the shape that separates them — and it is the IDOR
    /// shape for any id that arrives in a request BODY.
    pub async fn database_in_another_workspace(&self) -> Uuid {
        let other: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Other workspace', $2, 'active') RETURNING id",
        )
        .bind(format!("other-{}", Uuid::new_v4().simple()))
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert other workspace");
        insert_database(&self.db, other, self.admin, false).await
    }

    /// Flip the live `delegated` column AFTER a version was published against
    /// it. Nothing re-validates when this happens — which is the entire reason
    /// the snapshot freezes a witness.
    /// Mark a live connection `governed` (or clear it) AFTER a version was
    /// published against it — Package 1's drift case, and the reason the
    /// governance rule is evaluated live rather than against a frozen witness.
    ///
    /// Note what does NOT move when this runs: `delegated` and
    /// `options.impersonate` are untouched, so a witness-pair comparison of the
    /// kind `shared_cache_allowed` performs still AGREES. That is the whole
    /// argument for the departure, and a test that flipped `delegated` instead
    /// would not distinguish the two designs.
    pub async fn set_database_governed(&self, database_id: Uuid, governed: bool) {
        let done = sqlx::query("UPDATE query.databases SET governed = $1 WHERE id = $2")
            .bind(governed)
            .bind(database_id)
            .execute(&self.db.pool)
            .await
            .expect("flip governed");
        // A forge that matched no row would leave every assertion below
        // measuring the un-flipped connection, i.e. passing for the wrong
        // reason — the exact shape this repo has shipped twelve times.
        assert_eq!(done.rows_affected(), 1, "no database row was flipped");
    }

    pub async fn set_database_delegated(&self, database_id: Uuid, delegated: bool) {
        sqlx::query("UPDATE query.databases SET delegated = $1 WHERE id = $2")
            .bind(delegated)
            .bind(database_id)
            .execute(&self.db.pool)
            .await
            .expect("flip delegated");
    }

    /// Replace an already-published endpoint's DRAFT statement list, straight
    /// into the table — mirrors `lifecycle.rs`'s helper of the same name.
    /// Written rather than PATCHed so a cache test does not also depend on the
    /// write route being correct. The affected-row count is asserted because a
    /// forge that missed the row would leave [`Fixture::republish`] refreezing
    /// the OLD statement list and the test would measure nothing.
    pub async fn set_statements(&self, ep: Uuid, statements: serde_json::Value) {
        let touched = sqlx::query("UPDATE dataapi.endpoints SET statements = $2 WHERE id = $1")
            .bind(ep)
            .bind(&statements)
            .execute(&self.db.pool)
            .await
            .expect("store statements")
            .rows_affected();
        assert_eq!(touched, 1, "the statement list must actually be stored");
    }

    /// Publish an EXISTING endpoint id a second time, through the real publish
    /// route (via [`Fixture::mgmt_post`]), and return the version number it
    /// reports. [`Fixture::publish`]/[`Fixture::publish_with_params`] always
    /// insert a fresh endpoint, which cannot produce a version-2 snapshot for a
    /// test that needs to prove a republish invalidates a warm cache entry.
    pub async fn republish(&self, ep: Uuid) -> i32 {
        let (status, body) = self
            .mgmt_post(
                &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", self.ws_slug),
                Some(serde_json::json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "republish failed: {body}");
        body["version"]
            .as_i64()
            .unwrap_or_else(|| panic!("no version in publish response: {body}")) as i32
    }

    /// [`Fixture::republish`] for a draft whose changes package 6 classifies as
    /// BREAKING: fetch the acknowledgement from `POST …/validate` and send it
    /// back on `POST …/publish`.
    ///
    /// [`Fixture::republish`] posts `{}` and therefore 409s on any breaking
    /// change, which is the whole point of that gate — so a test that needs two
    /// versions differing in something breaking (a tightened `rate_limit`, a
    /// removed column) cannot use it. This is the same two-step an author
    /// performs in the console, driven through the same two routes.
    ///
    /// It **asserts a token was issued**, so it cannot be used by accident on a
    /// non-breaking change: publish refuses a token sent when nothing breaks, and
    /// a helper that silently degraded to `{}` would hide the fact that the
    /// change under test turned out not to be breaking after all — which is
    /// exactly the vacuity a test using this is trying to avoid.
    pub async fn republish_acknowledging(&self, ep: Uuid) -> i32 {
        let (status, v) = self
            .mgmt_post(
                &format!("/api/ws/{}/data-api/endpoints/{ep}/validate", self.ws_slug),
                Some(serde_json::json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "validate failed: {v}");
        let token = v["acknowledge_breaking"].as_str().unwrap_or_else(|| {
            panic!(
                "no acknowledge_breaking in the validate response, i.e. this change is NOT \
                 breaking — use `republish` instead, and re-read what the test is asserting: {v}"
            )
        });
        let (status, body) = self
            .mgmt_post(
                &format!("/api/ws/{}/data-api/endpoints/{ep}/publish", self.ws_slug),
                Some(serde_json::json!({ "acknowledge_breaking": token })),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "acknowledged republish failed: {body}"
        );
        body["version"]
            .as_i64()
            .unwrap_or_else(|| panic!("no version in publish response: {body}")) as i32
    }

    /// Take back the `data_api_endpoint:invoke` grant [`grant_invoke`] issued.
    ///
    /// SQL for the same reason `grant_invoke` is SQL: these principals are plain
    /// `users` rows with no `dataapi.service_accounts` binding, which the real
    /// `DELETE …/grants/{sa}` route (correctly) refuses. The route itself is
    /// exercised by `serving_authz.rs` against real service accounts.
    ///
    /// **Asserts it removed a row.** A revoke that matched nothing looks exactly
    /// like a revoke that worked, right up until the assertion that the call is
    /// now refused — at which point the test reports "the pin resurrected a
    /// revoked grant" for a grant that was never revoked.
    pub async fn revoke_invoke(&self, user: UserId, endpoint: Uuid) {
        let removed = sqlx::query(
            "DELETE FROM access_grants g USING roles r \
             WHERE g.role_id = r.id AND r.scope = 'workspace' \
               AND r.name = 'data_api_invoker' \
               AND g.principal_type = 'user' AND g.principal_id = $1 \
               AND g.scope_type = 'object' AND g.object_type = 'data_api_endpoint' \
               AND g.scope_id = $2",
        )
        .bind(user.0)
        .bind(endpoint)
        .execute(&self.db.pool)
        .await
        .expect("revoke invoke")
        .rows_affected();
        assert_eq!(
            removed, 1,
            "the invoke grant must actually have been removed; a revoke that \
             matched nothing makes every refusal assertion after it meaningless"
        );
    }

    /// Overwrite the DRAFT's `rate_limit`, so a republish freezes a different one.
    pub async fn set_rate_limit(&self, ep: Uuid, rate_limit: serde_json::Value) {
        let touched = sqlx::query("UPDATE dataapi.endpoints SET rate_limit = $2 WHERE id = $1")
            .bind(ep)
            .bind(&rate_limit)
            .execute(&self.db.pool)
            .await
            .expect("store rate_limit")
            .rows_affected();
        assert_eq!(touched, 1, "the rate limit must actually be stored");
    }

    /// Package 8: `GET /data-api/v1/{ws}` as `principal`, with a valid bearer.
    ///
    /// The URL is built from `self.ws_slug` and NOT from a caller-supplied
    /// string, so the ordinary case cannot accidentally be a cross-workspace
    /// one; [`Fixture::contract_raw`] is what a test uses when the mismatch is
    /// the subject.
    pub async fn contract_as(&self, principal: UserId) -> ContractRes {
        let token = mint(
            self.ws_id,
            principal,
            &format!("client-{}", principal.0.simple()),
        );
        self.contract_raw(&self.ws_slug, &format!("Bearer {token}"))
            .await
    }

    /// The same fetch with the slug and the `Authorization` header supplied
    /// VERBATIM.
    ///
    /// Both halves have to be forgeable for the refusals to be measurable: a
    /// token minted for another workspace, an archived workspace's slug, a
    /// header that will not verify, or no header at all. [`Fixture::contract_as`]
    /// can express none of those, because it always mints a valid token for this
    /// workspace.
    pub async fn contract_raw(&self, ws_slug: &str, authorization: &str) -> ContractRes {
        let mut builder = Request::builder()
            .method("GET")
            .uri(format!("/data-api/v1/{ws_slug}"));
        if !authorization.is_empty() {
            builder = builder.header("authorization", authorization);
        }
        let res = self
            .serving
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        contract_res(res).await
    }
}

/// Read a contract response off the wire. Shared by [`Fixture::contract_raw`]
/// and by a test that drives the route through a router of its own — a test
/// substituting the rate-limit store has to build the router itself, and it must
/// still read the answer exactly the way the ordinary path does or it is
/// asserting on a different thing.
pub async fn contract_res(res: axum::response::Response) -> ContractRes {
    let status = res.status();
    let header = |name: &str| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let cache_control = header("cache-control");
    let retry_after = header("retry-after");
    let bytes = axum::body::to_bytes(res.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    ContractRes {
        status,
        cache_control,
        retry_after,
        body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    }
}

/// One fetch of the bearer-reachable contract.
///
/// Separate from [`Res`] because the two assertions this route needs are not
/// the ones a served call needs: `Cache-Control` (which `Res` does not carry,
/// and whose absence is a real defect here — see `serving::contract`) and the
/// document itself rather than a `data` array.
pub struct ContractRes {
    pub status: StatusCode,
    pub cache_control: Option<String>,
    /// `Retry-After`, in seconds. Carried because this route is rate limited —
    /// see `serving::contract` — and a 429 without it tells a consumer to guess.
    pub retry_after: Option<String>,
    pub body: serde_json::Value,
}

impl ContractRes {
    /// The `paths` keys, sorted — "which endpoints does this document describe".
    ///
    /// Sorted so an assertion is about the SET and not about `serde_json`'s
    /// insertion order, and panicking rather than defaulting to empty: a
    /// document with no `paths` object at all is a failure, and a helper that
    /// quietly returned `[]` for it would make "this account sees nothing" pass
    /// for the wrong reason.
    pub fn paths(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.body["paths"]
            .as_object()
            .unwrap_or_else(|| panic!("no paths object in {}", self.body))
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    /// The wire `error.code`, for a refusal.
    pub fn code(&self) -> &str {
        self.body["error"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("no error.code in {}", self.body))
    }
}
