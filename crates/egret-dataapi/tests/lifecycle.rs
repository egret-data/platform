//! The publish lifecycle, driven from the caller's side against the REAL Cedar
//! PDP and a real Postgres — a mocked decision would pin the consumer, not the
//! server.
//!
//! The file exists for ONE property above all others:
//! [`a_published_version_is_unaffected_by_later_draft_edits`]. If a published
//! version follows its draft, versioning is decoration and P4 would change the
//! behaviour of every deployed client every time an author saved. That test is
//! mutation-verified against the read path in `EndpointStore::versions`.
//!
//! The engine is a fake [`FakePort`] rather than a live Trino. That is not a
//! shortcut: `describe_as`'s own behaviour is pinned by `egret-query`'s
//! `port_describe.rs` against a real coordinator, and what these tests need to
//! control is the FAILURE — a Ranger denial must stay a 403 and a bad-SQL error
//! must stay a 400 rather than both flattening into "your endpoint is broken",
//! and a failed describe must leave nothing behind at all. Neither is reachable
//! on demand from a live engine.
//!
//! DB-gated. Self-skips (passes) when DATABASE_URL is unset, like the sibling
//! suites — so check the DURATION, not just the `ok`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::{ColumnMeta, Error, UserId};
use egret_dataapi::{management_router, ClientStore, DataApiState, EndpointStore};
use egret_dataapi_api::EndpointSnapshot;
use egret_metastore::Db;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use uuid::Uuid;

/// What the fake engine should do on the next `describe_as`.
#[derive(Clone, Debug)]
enum Describe {
    /// Resolve to these columns.
    Columns(Vec<(&'static str, &'static str)>),
    /// The caller's policy denies a table the query names.
    Forbidden,
    /// The engine could not parse the SQL.
    BadSql(&'static str),
    /// The relation does not exist.
    Missing(&'static str),
}

/// A `QueryExecPort` under the test's control, which also RECORDS what it was
/// asked. The recording is what proves the probe rendering happened: an endpoint
/// with a `{{param}}` in it can only be described if the placeholder was
/// substituted first, and asserting on the SQL the port received is the only way
/// to see that from outside.
struct FakePort {
    mode: Mutex<Describe>,
    seen: Mutex<Vec<(Uuid, Uuid, Uuid, String)>>,
    /// The `param_count` of the last describe. Publish now describes the
    /// RENDERED statement, so the markers and their count are the whole reason
    /// `describe_as_with_params` exists — a fake that recorded only the SQL
    /// could not tell the two paths apart.
    last_params: Mutex<usize>,
    /// A statement to run DURING `describe_as` — that is, inside the exact
    /// window between publish's pre-lock read and its transaction. This is how
    /// the TOCTOU interleaving is produced deterministically, through the real
    /// route, instead of being simulated by calling the store directly.
    ///
    /// Binds `$1` = endpoint id, `$2` = a jsonb value. Fires once.
    mid_describe: Mutex<Option<(Db, &'static str, Uuid, serde_json::Value)>>,
}

impl FakePort {
    fn new() -> Self {
        Self {
            mode: Mutex::new(Describe::Columns(vec![("n", "integer")])),
            seen: Mutex::new(Vec::new()),
            last_params: Mutex::new(0),
            mid_describe: Mutex::new(None),
        }
    }
    fn set(&self, m: Describe) {
        *self.mode.lock().unwrap() = m;
    }
    /// Arrange for `stmt` to run in the middle of the next publish's describe.
    fn patch_during_next_describe(
        &self,
        db: &Db,
        stmt: &'static str,
        ep: Uuid,
        value: serde_json::Value,
    ) {
        *self.mid_describe.lock().unwrap() = Some((db.clone(), stmt, ep, value));
    }
    fn last_sql(&self) -> String {
        self.seen
            .lock()
            .unwrap()
            .last()
            .map(|(_, _, _, s)| s.clone())
            .expect("describe_as was never called")
    }
    fn call_count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    fn last_param_count(&self) -> usize {
        *self.last_params.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for FakePort {
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        self.seen
            .lock()
            .unwrap()
            .push((ws, database_id, principal.0, sql.to_string()));
        // The interleaving: publish has taken its pre-lock read and validated it,
        // and has not yet opened its transaction.
        let hook = self.mid_describe.lock().unwrap().take();
        if let Some((db, stmt, ep, value)) = hook {
            sqlx::query(stmt)
                .bind(ep)
                .bind(&value)
                .execute(&db.pool)
                .await
                .expect("mid-describe patch");
        }
        match self.mode.lock().unwrap().clone() {
            Describe::Columns(cols) => Ok(cols
                .into_iter()
                .map(|(name, ty)| ColumnMeta {
                    name: name.into(),
                    ty: ty.into(),
                })
                .collect()),
            Describe::Forbidden => Err(Error::Forbidden),
            Describe::BadSql(m) => Err(Error::InvalidInput(m.into())),
            Describe::Missing(m) => Err(Error::NotFound(m.into())),
        }
    }

    /// The path publish actually takes. It does NOT fall through to the trait's
    /// default: the default refuses a non-zero `param_count`, which is the right
    /// answer for a fake that has not thought about parameters and the wrong one
    /// for this suite, whose whole subject is parameterised publish.
    async fn describe_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        param_count: usize,
    ) -> Result<Vec<ColumnMeta>, Error> {
        *self.last_params.lock().unwrap() = param_count;
        self.describe_as(ws, database_id, principal, sql).await
    }

    /// Lifecycle is the AUTHORING path (draft → validate → publish); it only
    /// ever describes. A call here would mean publish started executing SQL.
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, Error> {
        unreachable!("the authoring lifecycle must never execute a query")
    }
}

struct Fixture {
    db: Db,
    ws: String,
    ws_id: Uuid,
    admin: Uuid,
    database_id: Uuid,
    port: Arc<FakePort>,
    /// A manager in `ws_id`, built once. `CedarPdp` snapshots policies at build
    /// time and a session caches its permission set, so this principal's grants
    /// must be in place before it is created — which they are, since the role is
    /// written inside `principal_with_in`.
    manager: Caller,
}

struct Caller {
    app: axum::Router,
    tok: String,
    /// The `users.id` behind the session. Needed to prove `describe_as` ran as
    /// THIS principal and not, say, as the endpoint's creator.
    user: Uuid,
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>, port: Arc<FakePort>) -> DataApiState {
    DataApiState {
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
        query: port,
        // The real Postgres key store. Nothing in this file authenticates a
        // machine token, so `active()` is never called and no key is generated.
        keys: Arc::new(egret_machine_idp::PgSigningKeyStore::new(db.clone())),
        issuer: "https://egret.test".into(),
        // Best-effort sink with its receiver dropped — see `audit.rs` for the
        // suite that actually reads the events.
        audit: egret_audit::AuditSink::new(1).0,
        cache: egret_dataapi::serving::ResponseCache::new(),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // This file is about the publish lifecycle, not Activity — a fixed
        // retention is enough to satisfy the field.
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
    }
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

async fn insert_workspace(db: &Db, owner: Uuid, tag: &str) -> (Uuid, String) {
    let slug = format!("{tag}-{}", Uuid::new_v4().simple());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Data API Lifecycle', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn insert_database(db: &Db, ws: Uuid, owner: Uuid, delegated: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, delegated, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, $3, $4) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(delegated)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

/// Both `:read` and `:manage`, as migration 0122 seeds `ws_admin`. Permissions
/// are flat, so a manage-only principal is not a role anyone holds; the
/// asymmetries that matter are pinned individually below.
const MANAGER: &[&str] = &["data_api_endpoint:read", "data_api_endpoint:manage"];

async fn fixture() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let admin = insert_user(&db, "lifecycle_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "lifecycle").await;
    let database_id = insert_database(&db, ws_id, admin, false).await;
    let port = Arc::new(FakePort::new());
    let manager = principal_with_in(&db, port.clone(), MANAGER, ws_id).await;
    Some(Fixture {
        db,
        ws,
        ws_id,
        admin,
        database_id,
        port,
        manager,
    })
}

/// A brand-new, non-admin user holding exactly `perms` in `ws`.
async fn principal_with_in(db: &Db, port: Arc<FakePort>, perms: &[&str], ws: Uuid) -> Caller {
    let user = insert_user(db, "lifecycle_caller").await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_lc_{}", Uuid::new_v4().simple()))
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

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp, port);
    let tok = st
        .sessions
        .create(egret_core::ids::UserId(user), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    // Nested at /api exactly as `egret-server` mounts it.
    let app = axum::Router::new()
        .nest("/api", management_router())
        .with_state(st);
    Caller { app, tok, user }
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

impl Fixture {
    async fn send(
        &self,
        c: &Caller,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("Cookie", format!("egret_session={}", c.tok));
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let res = c
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

    fn collection(&self) -> String {
        format!("/api/ws/{}/data-api/endpoints", self.ws)
    }

    /// A draft written straight to the table, so lifecycle tests do not depend on
    /// the create route also being correct.
    async fn create_endpoint(&self, name: &str, path: &str, sql: &str) -> Uuid {
        self.create_endpoint_on(self.database_id, name, path, sql)
            .await
    }

    async fn create_endpoint_on(
        &self,
        database_id: Uuid,
        name: &str,
        path: &str,
        sql: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, sql, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, $5, $6) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(name)
        .bind(format!("/{}", path.trim_start_matches('/')))
        .bind(database_id)
        .bind(sql)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft")
    }

    async fn patch_endpoint(&self, ep: Uuid, body: serde_json::Value) {
        let (status, b) = self
            .send(
                &self.manager,
                "PATCH",
                &format!("{}/{ep}", self.collection()),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "patch failed: {b}");
    }

    async fn publish_raw(&self, ep: Uuid) -> (StatusCode, serde_json::Value) {
        self.send(
            &self.manager,
            "POST",
            &format!("{}/{ep}/publish", self.collection()),
            Some(serde_json::json!({})),
        )
        .await
    }

    async fn publish(&self, ep: Uuid) -> serde_json::Value {
        let (status, body) = self.publish_raw(ep).await;
        assert_eq!(status, StatusCode::OK, "publish failed: {body}");
        body
    }

    async fn versions(&self, ep: Uuid) -> Vec<serde_json::Value> {
        let (status, body) = self
            .send(
                &self.manager,
                "GET",
                &format!("{}/{ep}/versions", self.collection()),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "versions failed: {body}");
        body.as_array().expect("array").clone()
    }

    /// The frozen definition currently being served. Reads it exactly the way P4
    /// will have to: through the version list, taking the row flagged `is_live`,
    /// and deserializing the snapshot column.
    async fn live_snapshot(&self, ep: Uuid) -> EndpointSnapshot {
        let versions = self.versions(ep).await;
        let live = versions
            .iter()
            .find(|v| v["is_live"] == serde_json::Value::Bool(true))
            .unwrap_or_else(|| panic!("no live version among {versions:?}"));
        serde_json::from_value(live["snapshot"].clone()).expect("deserialize snapshot")
    }

    async fn version_id_of(&self, ep: Uuid, version: i32) -> Uuid {
        let versions = self.versions(ep).await;
        let v = versions
            .iter()
            .find(|v| v["version"] == version)
            .unwrap_or_else(|| panic!("no version {version} among {versions:?}"));
        v["id"].as_str().expect("id").parse().expect("uuid")
    }

    async fn rollback_raw(
        &self,
        ep: Uuid,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.send(
            &self.manager,
            "POST",
            &format!("{}/{ep}/rollback", self.collection()),
            Some(body),
        )
        .await
    }

    async fn rollback(&self, ep: Uuid, version: i32) {
        let (status, body) = self
            .rollback_raw(ep, serde_json::json!({ "version": version }))
            .await;
        assert_eq!(status, StatusCode::OK, "rollback failed: {body}");
    }

    async fn state_of(&self, ep: Uuid) -> (String, Option<Uuid>) {
        sqlx::query_as("SELECT state, live_version_id FROM dataapi.endpoints WHERE id = $1")
            .bind(ep)
            .fetch_one(&self.db.pool)
            .await
            .expect("read state")
    }
}

// ---------- The invariant ----------

/// **The test this task exists for.** A published version must be frozen: a
/// later edit to the draft — which is the ONLY thing an author can touch — must
/// not reach it. Everything else here is bookkeeping; if this passes only
/// because the read path happens to agree with the draft today, versioning is
/// decoration and P4 will silently change what deployed clients receive.
///
/// Mutation-verified: pointing `EndpointStore::versions` at `e.sql` instead of
/// `v.snapshot` turns this red.
#[tokio::test]
async fn a_published_version_is_unaffected_by_later_draft_edits() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("rev", "reports/revenue", "SELECT 1 AS n")
        .await;
    f.publish(ep).await;
    let v1 = f.live_snapshot(ep).await;
    assert_eq!(v1.sql, "SELECT 1 AS n");

    // Edit the draft afterwards — including the SQL and the parameter list.
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "sql": "SELECT 2 AS n WHERE r = {{r}}",
            "params": [{ "name": "r", "type": "string", "location": "query" }],
            "limits": { "max_rows": 1, "timeout_seconds": 1 },
        }),
    )
    .await;

    let still = f.live_snapshot(ep).await;
    assert_eq!(
        still.sql, "SELECT 1 AS n",
        "the live version must not follow the draft — this is what makes versioning real"
    );
    // Not only the SQL: every field the snapshot froze must be equal to what was
    // published, or a partial read-through would pass the assertion above while
    // still leaking the draft.
    assert_eq!(
        still, v1,
        "no field of a published snapshot may follow the draft"
    );
}

#[tokio::test]
async fn rollback_restores_the_previous_snapshot() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("rev", "reports/revenue", "SELECT 1 AS n")
        .await;
    f.publish(ep).await; // v1
    f.patch_endpoint(ep, serde_json::json!({ "sql": "SELECT 2 AS n" }))
        .await;
    f.publish(ep).await; // v2
    assert_eq!(f.live_snapshot(ep).await.sql, "SELECT 2 AS n");

    f.rollback(ep, 1).await;
    assert_eq!(f.live_snapshot(ep).await.sql, "SELECT 1 AS n");
}

/// A version belongs to exactly one endpoint. Without the `v.endpoint_id = e.id`
/// predicate in the store's UPDATE, a bare version uuid — which carries no
/// ownership at all — would make one endpoint serve another's frozen SQL,
/// including across workspaces.
#[tokio::test]
async fn rollback_refuses_a_version_from_another_endpoint() {
    let f = skip_without_db!();
    let a = f.create_endpoint("a", "a", "SELECT 1 AS n").await;
    let b = f.create_endpoint("b", "b", "SELECT 2 AS n").await;
    f.publish(a).await;
    f.publish(b).await;
    let b_v1 = f.version_id_of(b, 1).await;

    let (status, body) = f
        .rollback_raw(a, serde_json::json!({ "version_id": b_v1 }))
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a version belongs to exactly one endpoint: {body}"
    );
    // And nothing moved.
    assert_eq!(f.live_snapshot(a).await.sql, "SELECT 1 AS n");
}

#[tokio::test]
async fn rollback_refuses_a_version_number_that_does_not_exist() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("a", "a", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let (status, _) = f
        .rollback_raw(ep, serde_json::json!({ "version": 7 }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rollback_needs_exactly_one_of_version_and_version_id() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("a", "a", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let v1 = f.version_id_of(ep, 1).await;
    for body in [
        serde_json::json!({}),
        serde_json::json!({ "version": 1, "version_id": v1 }),
    ] {
        let (status, b) = f.rollback_raw(ep, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {b}");
    }
}

// ---------- Publish ----------

#[tokio::test]
async fn publishing_freezes_the_described_columns_and_the_canonical_path() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![
        ("id", "uuid"),
        ("total", "decimal(38,2)"),
    ]));
    let ep = f
        .create_endpoint("rev", "reports/revenue", "SELECT id, total FROM t")
        .await;

    let published = f.publish(ep).await;
    assert_eq!(published["version"], 1);
    assert_eq!(published["state"], "live");

    let snap = f.live_snapshot(ep).await;
    assert_eq!(
        snap.columns
            .iter()
            .map(|c| (c.name.as_str(), c.ty.as_str()))
            .collect::<Vec<_>>(),
        vec![("id", "uuid"), ("total", "decimal(38,2)")],
        "the output schema `describe` resolved must be frozen into the snapshot"
    );
    // The stored, already-canonical path — never a raw user spelling.
    assert_eq!(snap.path, "/reports/revenue");
    assert_eq!(snap.method, "GET");
    assert_eq!(snap.database_id, f.database_id);

    let (state, live) = f.state_of(ep).await;
    assert_eq!(state, "live");
    assert_eq!(live, Some(f.version_id_of(ep, 1).await));
}

#[tokio::test]
async fn republishing_increments_the_version() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    f.patch_endpoint(ep, serde_json::json!({ "sql": "SELECT 2 AS n" }))
        .await;
    let second = f.publish(ep).await;
    assert_eq!(second["version"], 2);

    let versions = f.versions(ep).await;
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["version"], 2, "newest first");
    assert_eq!(versions[0]["is_live"], true);
    assert_eq!(versions[1]["is_live"], false);
}

/// The atomicity requirement, stated as an observable: a failed describe must
/// leave NOTHING behind — no version row, no state change, no live pointer.
#[tokio::test]
async fn a_failed_describe_leaves_no_half_published_endpoint() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT nope FROM t").await;
    f.port.set(Describe::BadSql("line 1:8: column 'nope'"));

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    assert!(f.versions(ep).await.is_empty(), "no version may exist");
    let (state, live) = f.state_of(ep).await;
    assert_eq!(state, "draft", "state must not have moved");
    assert_eq!(live, None);
}

/// **An empty column list is a publish error, never a frozen empty schema.**
///
/// This is not a hypothetical shape of failure: on StarRocks the TEXT describe
/// path answers a statement containing a bind marker with zero columns and NO
/// error (measured, spec §3.4) — which is why publish describes through
/// `describe_as_with_params` at all. If that ever regressed, or if an engine
/// simply had nothing to say about a statement, freezing the answer would give
/// an endpoint that returns 200s carrying empty objects forever, with nothing in
/// the response or the log to say what went wrong.
#[tokio::test]
async fn a_describe_that_resolves_no_columns_is_refused() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("empty", "empty", "SELECT 1 AS n").await;
    f.port.set(Describe::Columns(vec![]));

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string().contains("no output columns"),
        "the reason must say what is empty: {body}"
    );
    assert!(f.versions(ep).await.is_empty(), "no version may exist");
}

/// A policy denial and a bad statement are different problems with different
/// fixes. Flattening both into one status sends an author whose SQL is perfect
/// to go and rewrite it.
#[tokio::test]
async fn describe_failures_keep_their_own_status() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;

    for (mode, expected) in [
        (Describe::Forbidden, StatusCode::FORBIDDEN),
        (Describe::Missing("tpch.tiny.nope"), StatusCode::NOT_FOUND),
        (
            Describe::BadSql("mismatched input"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        f.port.set(mode.clone());
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, expected, "{mode:?} → {body}");
    }
}

/// The whole validation surface, reached through the route. Each of these must
/// be a 400 and must not reach the engine at all — a multi-statement string in
/// particular must never be handed to a coordinator.
#[tokio::test]
async fn publish_refuses_an_invalid_definition_without_calling_the_engine() {
    let f = skip_without_db!();
    let before = f.port.call_count();
    for (sql, params, why) in [
        (
            "SELECT 1; DROP TABLE t",
            serde_json::json!([]),
            "two statements",
        ),
        (
            "INSERT INTO t VALUES (1)",
            serde_json::json!([]),
            "not a SELECT",
        ),
        (
            "SELECT * FROM t WHERE r = {{region}}",
            serde_json::json!([]),
            "undeclared placeholder",
        ),
        (
            "SELECT 1 AS n",
            serde_json::json!([{ "name": "region", "type": "string", "location": "query" }]),
            "unreferenced parameter",
        ),
        (
            "SELECT * FROM t WHERE s = {{s}}",
            serde_json::json!([{ "name": "s", "type": "enum", "location": "query" }]),
            "enum with no allowed list",
        ),
        // The serve-time privilege boundary: a placeholder inside a literal is
        // substituted as an already-quoted value, which closes the enclosing
        // literal and hands any Data API token holder a live predicate. It
        // carries no second statement, and it describes cleanly as `'x''y'`.
        (
            "SELECT * FROM t WHERE a = 'x{{p}}y'",
            serde_json::json!([{ "name": "p", "type": "string", "location": "query" }]),
            "placeholder inside a string literal",
        ),
        (
            "SELECT n FROM t /* {{p}} */",
            serde_json::json!([{ "name": "p", "type": "string", "location": "query" }]),
            "placeholder inside a block comment",
        ),
        // `#` is not a comment on either engine `Engine` can name. On StarRocks
        // 4.1.3, measured with `mysql -c`, it is accepted and IGNORED
        // (`SELECT 1 #x` returns a column named `x`); Trino has no `#` token.
        // The validator refuses this shape as a forward-looking rule — it must
        // hold before any MySQL-compatible front end where `#` IS a comment and
        // the placeholder would sit inside one.
        //
        // Deliberately no claim about what the ENGINE would do with this string:
        // earlier rounds of this task asserted first that it was a live
        // serve-time injection and then that describe would reject it, and both
        // were wrong. What is pinned below is the 400 and the absent version row.
        // See `lifecycle.rs`' scanner comment for the measurement of record.
        (
            "SELECT n FROM t # {{p}}",
            serde_json::json!([{ "name": "p", "type": "string", "location": "query" }]),
            "placeholder inside a hash region",
        ),
        // Two statements under the Trino reading of `\`, invisible to a scanner
        // that assumes the StarRocks one.
        (
            r"SELECT 'a\' ; DROP TABLE t -- '",
            serde_json::json!([]),
            "backslash smuggle, terminated",
        ),
        (
            "SELECT 1 --x\r; DROP TABLE t",
            serde_json::json!([]),
            "second statement after a bare CR",
        ),
    ] {
        let ep = f
            .create_endpoint(why, &format!("p/{}", Uuid::new_v4().simple()), sql)
            .await;
        f.patch_endpoint(ep, serde_json::json!({ "params": params }))
            .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(f.versions(ep).await.is_empty(), "{why} must not publish");
    }
    assert_eq!(
        f.port.call_count(),
        before,
        "an invalid definition must be refused before the engine is asked anything"
    );
}

/// `cache.scope = shared` on a DELEGATED connection is the one validation rule
/// that is a data-leak rather than a correctness problem: the engine filters
/// rows per principal, so a shared cache serves one caller's rows to another. It
/// must be refused at publish time, and the identical config on a non-delegated
/// connection must still publish — otherwise the test would pass against a rule
/// that simply banned shared caches.
#[tokio::test]
async fn a_shared_cache_is_refused_only_on_a_delegated_connection() {
    let f = skip_without_db!();
    let delegated_db = insert_database(&f.db, f.ws_id, f.admin, true).await;
    let shared =
        serde_json::json!({ "cache": { "enabled": true, "ttl_seconds": 60, "scope": "shared" } });

    let bad = f
        .create_endpoint_on(delegated_db, "d", "d", "SELECT 1 AS n")
        .await;
    f.patch_endpoint(bad, shared.clone()).await;
    let (status, body) = f.publish_raw(bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("delegated"));

    let ok = f.create_endpoint("s", "s", "SELECT 1 AS n").await;
    f.patch_endpoint(ok, shared).await;
    let (status, body) = f.publish_raw(ok).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the same config is fine when the connection is not delegated: {body}"
    );
}

/// **The TOCTOU window, driven through the real route.** The identity check
/// covers only `sql`/`database_id`/`params` — the three inputs `columns` depends
/// on — while the snapshot is built from the locked read for EVERY field. So an
/// edit landing during the describe could set `cache.scope = shared` on a
/// delegated connection and be frozen, publishing exactly the configuration
/// `check_cache_scope` exists to make unpublishable.
///
/// The edit is issued from inside `describe_as`, which is precisely the window.
/// What closes it is re-running the pure validator against the locked row.
#[tokio::test]
async fn an_edit_during_describe_cannot_publish_a_configuration_the_validator_forbids() {
    let f = skip_without_db!();
    let delegated_db = insert_database(&f.db, f.ws_id, f.admin, true).await;
    let ep = f
        .create_endpoint_on(delegated_db, "toctou", "toctou", "SELECT 1 AS n")
        .await;
    // Valid as it stands: per-principal cache on a delegated connection.
    f.patch_endpoint(
        ep,
        serde_json::json!({ "cache": { "enabled": true, "ttl_seconds": 60, "scope": "per_principal" } }),
    )
    .await;

    f.port.patch_during_next_describe(
        &f.db,
        "UPDATE dataapi.endpoints SET cache = $2 WHERE id = $1",
        ep,
        serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "shared" }),
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the locked row must be re-validated, not just compared: {body}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "and nothing may have been frozen"
    );
    assert_eq!(f.state_of(ep).await.0, "draft");
}

/// The same window, with an edit the identity check DOES cover. Different rule,
/// different status: stale `columns` are a retryable conflict, not a bad request.
#[tokio::test]
async fn an_sql_edit_during_describe_is_a_conflict() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("toctou2", "toctou2", "SELECT 1 AS n")
        .await;
    f.port.patch_during_next_describe(
        &f.db,
        "UPDATE dataapi.endpoints SET sql = $2 #>> '{}' WHERE id = $1",
        ep,
        serde_json::json!("SELECT 2 AS n"),
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(f.versions(ep).await.is_empty());
}

/// `query.databases.delegated` is mutable and lives OUTSIDE the snapshot, so a
/// legitimately published `scope = shared` version becomes a cross-principal
/// leak the moment its connection is flipped to delegated — with no
/// re-validation anywhere. Freezing the value publish validated against is what
/// gives P4 something to compare the live column with.
#[tokio::test]
async fn the_snapshot_freezes_the_delegation_mode_it_was_validated_against() {
    let f = skip_without_db!();
    let plain = f.create_endpoint("nd", "nd", "SELECT 1 AS n").await;
    f.publish(plain).await;
    assert!(
        !f.live_snapshot(plain).await.delegated,
        "a non-delegated connection must be recorded as such"
    );

    let delegated_db = insert_database(&f.db, f.ws_id, f.admin, true).await;
    let deleg = f
        .create_endpoint_on(delegated_db, "d", "d", "SELECT 1 AS n")
        .await;
    f.publish(deleg).await;
    assert!(f.live_snapshot(deleg).await.delegated);

    // The scenario the field exists for: a shared cache published while the
    // connection was NOT delegated, then the connection flips. The snapshot still
    // says `false`, which is how P4 can tell it must refuse the shared cache.
    let shared = f.create_endpoint("sh", "sh", "SELECT 1 AS n").await;
    f.patch_endpoint(
        shared,
        serde_json::json!({ "cache": { "enabled": true, "ttl_seconds": 60, "scope": "shared" } }),
    )
    .await;
    f.publish(shared).await;
    let snap = f.live_snapshot(shared).await;
    assert!(!snap.delegated);
    sqlx::query("UPDATE query.databases SET delegated = true WHERE id = $1")
        .bind(f.database_id)
        .execute(&f.db.pool)
        .await
        .expect("flip delegation");
    assert!(
        !f.live_snapshot(shared).await.delegated,
        "the frozen witness must not follow the live column — it is what P4 \
         compares AGAINST"
    );
    // Undo, so the shared fixture database is left as the other tests expect.
    sqlx::query("UPDATE query.databases SET delegated = false WHERE id = $1")
        .bind(f.database_id)
        .execute(&f.db.pool)
        .await
        .expect("restore delegation");
}

/// `rollback` deliberately does not re-enable a disabled endpoint. `publish` must
/// not either, or the two rules disagree and the quieter one wins by surprise.
#[tokio::test]
async fn publishing_a_disabled_endpoint_does_not_put_it_back_in_service() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    f.send(
        &f.manager,
        "POST",
        &format!("{}/{ep}/disable", f.collection()),
        None,
    )
    .await;

    f.patch_endpoint(ep, serde_json::json!({ "sql": "SELECT 2 AS n" }))
        .await;
    let published = f.publish(ep).await;
    assert_eq!(published["version"], 2, "the version is still minted");
    assert_eq!(
        published["state"], "disabled",
        "and the response says so, rather than claiming live"
    );

    let (state, live) = f.state_of(ep).await;
    assert_eq!(state, "disabled", "publishing must not re-enable");
    assert_eq!(live, Some(f.version_id_of(ep, 2).await), "but v2 is queued");
    // And the explicit route still works, serving the newly published version.
    f.send(
        &f.manager,
        "POST",
        &format!("{}/{ep}/enable", f.collection()),
        None,
    )
    .await;
    assert_eq!(f.state_of(ep).await.0, "live");
    assert_eq!(f.live_snapshot(ep).await.sql, "SELECT 2 AS n");
}

/// A parameterized endpoint has to be describable at all. `WHERE r = {{r}}` is
/// not SQL, so the placeholders are rendered to typed probe literals before the
/// engine sees them — and the frozen `sql` is still the TEMPLATE, not the probe.
#[tokio::test]
async fn a_parameterized_endpoint_is_described_through_a_rendered_probe() {
    let f = skip_without_db!();
    let sql = "SELECT n FROM t WHERE r = {{r}} AND n > {{n}}";
    let ep = f.create_endpoint("p", "p", sql).await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "r", "type": "string", "location": "query" },
            { "name": "n", "type": "number", "location": "query" }
        ]}),
    )
    .await;

    f.publish(ep).await;
    assert_eq!(
        f.port.last_sql(),
        "SELECT n FROM t WHERE r = ? AND n > ?",
        "the engine must be handed the statement that will be SERVED — rendered, \
         with bind markers — not a template and not a literal-substituted probe"
    );
    assert_eq!(
        f.port.last_param_count(),
        2,
        "and the marker count beside it, or the engine cannot type the statement"
    );
    assert_eq!(
        f.live_snapshot(ep).await.sql,
        sql,
        "the snapshot freezes the TEMPLATE — the render is only for describe"
    );
}

/// The publish rules of spec §5, each reaching publish through the route and
/// each refused with its own reason.
///
/// One case per rule, and the rule's own error text is asserted rather than just
/// the 400: three of these are 400s for entirely different reasons, and a test
/// that only counted statuses would pass with any one of the guards deleted.
#[tokio::test]
async fn publish_refuses_each_parameter_rule_with_its_own_reason() {
    let f = skip_without_db!();
    let before = f.port.call_count();
    for (sql, params, needle, why) in [
        (
            "SELECT 1 AS n WHERE r = {{r}}",
            serde_json::json!([{ "name": "r", "type": "string", "location": "body" }]),
            "location: body",
            "a body parameter is never supplied",
        ),
        (
            "SELECT 1 AS n WHERE d = {{d}}",
            serde_json::json!([
                { "name": "d", "type": "date", "location": "query", "default": "yesterday" }
            ]),
            "is not a valid `date`",
            "an author default that cannot coerce blames the caller forever",
        ),
        (
            "SELECT 1 AS n ORDER BY {{ s | ident }}",
            serde_json::json!([
                { "name": "s", "type": "enum", "location": "query",
                  "allowed": ["a", "b"] }
            ]),
            "optional with no `default`",
            "an unguarded optional enum is a 500 on every call that omits it",
        ),
        (
            "SELECT 1 AS n ORDER BY {{ s | ident }}",
            serde_json::json!([
                { "name": "s", "type": "string", "location": "query", "required": true }
            ]),
            "`ident` may only be applied to an enum parameter",
            "`ident` on a non-enum parameter",
        ),
        (
            "SELECT 1 AS n LIMIT {{n}}",
            serde_json::json!([
                { "name": "n", "type": "number", "location": "query", "required": true }
            ]),
            "`LIMIT`/`OFFSET`",
            "no engine binds a limit",
        ),
        (
            "SELECT {{v}} AS v FROM t",
            serde_json::json!([
                { "name": "v", "type": "string", "location": "query", "required": true }
            ]),
            "without a `CAST`",
            "an uncast projection parameter freezes the wrong column type",
        ),
        (
            "SELECT 1 AS n {% filter upper %}{{ r }}{% endfilter %}",
            serde_json::json!([
                { "name": "r", "type": "string", "location": "query", "required": true }
            ]),
            "may not appear in a published definition",
            "a capture construct desyncs the bind list",
        ),
        (
            "SELECT 1 AS n WHERE {% if r == 'EU' %}1=1{% endif %} AND x = {{r}}",
            serde_json::json!([
                { "name": "r", "type": "string", "location": "query", "required": true }
            ]),
            "compared directly against another value",
            "a bare parameter comparison is silently false",
        ),
        (
            "SELECT 1 AS n WHERE r = {{r",
            serde_json::json!([
                { "name": "r", "type": "string", "location": "query", "required": true }
            ]),
            "malformed parameter placeholder",
            "an unterminated placeholder",
        ),
    ] {
        let ep = f
            .create_endpoint(why, &format!("p/{}", Uuid::new_v4().simple()), sql)
            .await;
        f.patch_endpoint(ep, serde_json::json!({ "params": params }))
            .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(
            body.to_string().contains(needle),
            "{why}: expected the reason to mention {needle:?}, got {body}"
        );
        assert!(f.versions(ep).await.is_empty(), "{why} must not publish");
    }
    assert_eq!(
        f.port.call_count(),
        before,
        "every one of these is decidable without the engine, so none may reach it"
    );
}

/// The rules must not have banned the shapes they exist to make safe — a test
/// suite that only proves refusals passes just as well against a validator that
/// refuses everything.
///
/// Spec §4.3's canonical endpoint is the headline: it could not be published at
/// all before T7-3, because P3's scanner read `sort | ident` as a parameter NAME
/// and a loop variable `r` as an undeclared parameter, and because the old probe
/// handed `{% if %}` to the engine verbatim.
#[tokio::test]
async fn the_canonical_parameterized_endpoint_publishes() {
    let f = skip_without_db!();
    let sql = "SELECT region, SUM(amount) AS total \
               FROM sales \
               WHERE 1=1 \
               {% if region %} AND region = {{ region }} {% endif %} \
               {% if from %} AND day >= {{ from }} {% endif %} \
               {% if regions %} AND region IN ( \
               {% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %} \
               ) {% endif %} \
               GROUP BY region \
               ORDER BY {{ sort | ident }}";
    let ep = f.create_endpoint("canon", "canon", sql).await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "region",  "type": "string", "location": "query" },
            { "name": "from",    "type": "date",   "location": "query" },
            { "name": "regions", "type": "string", "location": "query" },
            // `sort` reaches an identifier position through `| ident`, which is
            // only legal on an enum with an `allowed` list — and it is required,
            // because `ORDER BY` has no `{% if %}` around it.
            { "name": "sort", "type": "enum", "location": "query", "required": true,
              "allowed": ["region", "total"] }
        ]}),
    )
    .await;

    f.publish(ep).await;
    let described = f.port.last_sql();
    assert!(
        described.contains("ORDER BY region"),
        "`| ident` emits the AUTHOR's allowed entry, raw: {described}"
    );
    assert!(
        described.contains("region = ?") && described.contains("day >= ?"),
        "every conditional branch a value reaches must be described: {described}"
    );
    assert_eq!(
        f.live_snapshot(ep).await.param_gate,
        egret_dataapi_api::PARAM_GATE,
        "a version that passed the gate must say so, or serving cannot tell it \
         from one published before the gate existed"
    );
}

/// `describe_as` must run as the CALLER, so a delegated connection resolves
/// under their policy and an author cannot publish a contract over a table they
/// cannot themselves read.
#[tokio::test]
async fn describe_runs_as_the_calling_principal_in_the_calling_workspace() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let (ws, database, principal, _) = f.port.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(ws, f.ws_id, "and in the caller's workspace, not the path's");
    assert_eq!(database, f.database_id);
    assert_eq!(
        principal, f.manager.user,
        "describe must run as the caller, not as the endpoint's owner"
    );
    assert_ne!(
        principal, f.admin,
        "specifically: not as the row's created_by"
    );
}

// ---------- Disable / enable ----------

/// Disabling keeps `live_version_id`, which is the whole reason re-enabling can
/// be free. Without the `enable` route this promise would be unfalsifiable.
#[tokio::test]
async fn disable_keeps_the_live_version_so_enable_needs_no_republish() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let v1 = f.version_id_of(ep, 1).await;

    let (status, body) = f
        .send(
            &f.manager,
            "POST",
            &format!("{}/{ep}/disable", f.collection()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (state, live) = f.state_of(ep).await;
    assert_eq!(state, "disabled");
    assert_eq!(live, Some(v1), "the live pointer must survive a disable");

    let calls = f.port.call_count();
    let (status, body) = f
        .send(
            &f.manager,
            "POST",
            &format!("{}/{ep}/enable", f.collection()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (state, live) = f.state_of(ep).await;
    assert_eq!(state, "live");
    assert_eq!(live, Some(v1), "the same version serves again");
    assert_eq!(
        f.port.call_count(),
        calls,
        "re-enabling must not re-describe — that is what 'no re-publish' means"
    );
    assert_eq!(f.versions(ep).await.len(), 1, "and must not mint a version");
}

#[tokio::test]
async fn enabling_an_unpublished_endpoint_is_a_conflict_not_a_live_endpoint_with_nothing_to_serve()
{
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    let (status, body) = f
        .send(
            &f.manager,
            "POST",
            &format!("{}/{ep}/enable", f.collection()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(f.state_of(ep).await.0, "draft");
}

/// Rolling back is a statement about WHICH version is served, not about whether
/// the endpoint serves at all.
#[tokio::test]
async fn rollback_does_not_silently_re_enable_a_disabled_endpoint() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    f.patch_endpoint(ep, serde_json::json!({ "sql": "SELECT 2 AS n" }))
        .await;
    f.publish(ep).await;
    f.send(
        &f.manager,
        "POST",
        &format!("{}/{ep}/disable", f.collection()),
        None,
    )
    .await;

    f.rollback(ep, 1).await;
    assert_eq!(f.state_of(ep).await.0, "disabled");
    assert_eq!(f.live_snapshot(ep).await.sql, "SELECT 1 AS n");
}

// ---------- Authorization and containment ----------

/// `:read` cannot author, and each lifecycle route is its own `require` call —
/// permissions here are FLAT, so nothing but the explicit check stands between a
/// reader and a publish.
#[tokio::test]
async fn read_permission_cannot_drive_the_lifecycle() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let reader =
        principal_with_in(&f.db, f.port.clone(), &["data_api_endpoint:read"], f.ws_id).await;

    for (route, body) in [
        ("validate", None),
        ("publish", Some(serde_json::json!({}))),
        ("rollback", Some(serde_json::json!({ "version": 1 }))),
        ("disable", None),
        ("enable", None),
    ] {
        let (status, b) = f
            .send(
                &reader,
                "POST",
                &format!("{}/{ep}/{route}", f.collection()),
                body,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{route}: {b}");
    }
    // The read side still works, or the loop above would pass against a router
    // that denied a reader everything.
    let (status, b) = f
        .send(
            &reader,
            "GET",
            &format!("{}/{ep}/versions", f.collection()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b.as_array().expect("array").len(), 1);
}

#[tokio::test]
async fn listing_versions_requires_read_not_manage() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let manage_only = principal_with_in(
        &f.db,
        f.port.clone(),
        &["data_api_endpoint:manage"],
        f.ws_id,
    )
    .await;
    let (status, b) = f
        .send(
            &manage_only,
            "GET",
            &format!("{}/{ep}/versions", f.collection()),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "permissions are flat — :manage must not imply :read. {b}"
    );
}

/// Cross-workspace is 404, never 403: a legitimate manager in ANOTHER workspace
/// must not be able to tell "no such endpoint" from "exists, but not yours" —
/// and must not be able to publish, roll back or disable it.
#[tokio::test]
async fn the_lifecycle_of_another_workspaces_endpoint_is_not_found() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;

    let (other_ws, other_slug) = insert_workspace(&f.db, f.admin, "lifecycle-other").await;
    let outsider = principal_with_in(&f.db, f.port.clone(), MANAGER, other_ws).await;
    let base = format!("/api/ws/{other_slug}/data-api/endpoints/{ep}");

    for (method, route, body) in [
        ("POST", "validate", None),
        ("POST", "publish", Some(serde_json::json!({}))),
        (
            "POST",
            "rollback",
            Some(serde_json::json!({ "version": 1 })),
        ),
        ("POST", "disable", None),
        ("POST", "enable", None),
        ("GET", "versions", None),
    ] {
        let (status, b) = f
            .send(&outsider, method, &format!("{base}/{route}"), body)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{route}: {b}");
    }
    // Untouched.
    let (state, _) = f.state_of(ep).await;
    assert_eq!(state, "live");
    assert_eq!(f.versions(ep).await.len(), 1);
}

/// An unpublished endpoint has an EMPTY version list; an endpoint that is not
/// this workspace's is a 404. Confusing the two would make the list route an
/// existence oracle.
#[tokio::test]
async fn an_unpublished_endpoint_has_an_empty_version_list_not_a_404() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    let (status, body) = f
        .send(
            &f.manager,
            "GET",
            &format!("{}/{ep}/versions", f.collection()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().expect("array").len(), 0);

    let (status, _) = f
        .send(
            &f.manager,
            "GET",
            &format!("{}/{}/versions", f.collection(), Uuid::new_v4()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------- Validate ----------

#[tokio::test]
async fn validate_reports_the_columns_without_publishing_anything() {
    let f = skip_without_db!();
    f.port
        .set(Describe::Columns(vec![("id", "uuid"), ("n", "bigint")]));
    let ep = f.create_endpoint("rev", "rev", "SELECT id, n FROM t").await;

    let (status, body) = f
        .send(
            &f.manager,
            "POST",
            &format!("{}/{ep}/validate", f.collection()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["columns"],
        serde_json::json!([
            { "name": "id", "ty": "uuid" },
            { "name": "n", "ty": "bigint" }
        ])
    );
    assert!(f.versions(ep).await.is_empty(), "validate must not publish");
    assert_eq!(f.state_of(ep).await.0, "draft");
}
