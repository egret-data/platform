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
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like the sibling
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
    /// Resolve by matching a SUBSTRING of the rendered statement, first match
    /// wins; the last entry is the fallback (give it `""`). Publish describes
    /// several renderings of one definition, so a fake that answers the same
    /// columns for all of them cannot tell a union apart from a single describe.
    PerSql(Vec<(&'static str, Vec<(&'static str, &'static str)>)>),
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
    /// Every statement the port was handed, oldest first. Publish describes one
    /// statement per branch now, so `last_sql` names an arbitrary one of them
    /// and a test about "was this branch described" has to look at all of them.
    fn all_sql(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, _, s)| s.clone())
            .collect()
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
            Describe::PerSql(table) => {
                let cols = table
                    .iter()
                    .find(|(needle, _)| sql.contains(needle))
                    .map(|(_, cols)| cols.clone())
                    .unwrap_or_else(|| panic!("PerSql has no arm matching {sql:?}"));
                Ok(cols
                    .into_iter()
                    .map(|(name, ty)| ColumnMeta {
                        name: name.into(),
                        ty: ty.into(),
                    })
                    .collect())
            }
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
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        // Best-effort sink with its receiver dropped — see `audit.rs` for the
        // suite that actually reads the events.
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // This file is about the publish lifecycle, not Activity — a fixed
        // retention is enough to satisfy the field.
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
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
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
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
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
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
               (workspace_id, name, path, method, database_id, statements, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', $5::text)), $6) RETURNING id",
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

    /// Replace the draft's statement list, straight into the table.
    ///
    /// Written rather than PATCHed for the same reason `create_endpoint` is: a
    /// publish test must not depend on the write route also being correct. The
    /// affected-row count is asserted because a forge that missed the row would
    /// otherwise leave the test measuring the ONE-statement endpoint it started
    /// from and passing for the wrong reason.
    async fn set_statements(&self, ep: Uuid, statements: serde_json::Value) {
        let touched = sqlx::query("UPDATE dataapi.endpoints SET statements = $2 WHERE id = $1")
            .bind(ep)
            .bind(&statements)
            .execute(&self.db.pool)
            .await
            .expect("store statements")
            .rows_affected();
        assert_eq!(touched, 1, "the statement list must actually be stored");
    }

    /// Writes `output` past the routes, the way a second writer of this table
    /// would — which is the only way an unrecognised envelope can reach a
    /// publish now that the create and patch routes refuse one.
    async fn set_output(&self, ep: Uuid, output: serde_json::Value) {
        let touched = sqlx::query("UPDATE dataapi.endpoints SET output = $2 WHERE id = $1")
            .bind(ep)
            .bind(&output)
            .execute(&self.db.pool)
            .await
            .expect("store output")
            .rows_affected();
        assert_eq!(touched, 1, "the output spec must actually be stored");
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

    /// A draft already in the trash, written in ONE statement rather than through
    /// the route — an authorization test must not depend on the very route it is
    /// proving a caller cannot reach.
    async fn create_trashed_endpoint(&self, name: &str, path: &str, sql: &str) -> Uuid {
        let id = self.create_endpoint(name, path, sql).await;
        sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await
            .expect("trash endpoint directly");
        id
    }

    /// Whether the row is in the trash. Read as `Option<Option<_>>` so "no such
    /// row" cannot be mistaken for "not trashed".
    async fn deleted_at(&self, ep: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
        let row: Option<Option<chrono::DateTime<chrono::Utc>>> =
            sqlx::query_scalar("SELECT deleted_at FROM dataapi.endpoints WHERE id = $1")
                .bind(ep)
                .fetch_optional(&self.db.pool)
                .await
                .expect("read deleted_at");
        row.expect("endpoint row still exists")
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
/// **A published version freezes a scripted statement's SCRIPT, not the text
/// publish generated from it.**
///
/// The largest design call in package S Task 6, and this is the assertion it
/// comes down to. Publish generates with REPRESENTATIVE parameter values —
/// stand-ins, not any caller's — so the text it produced is one branch of a
/// script whose whole purpose is to take a different one per request. A version
/// that froze that text would answer the representative branch to every caller,
/// with a `200` and no error anywhere: every parameter the script branches on
/// would be inert, silently. So the version carries what the author wrote, and
/// serving generates from it.
///
/// The `columns` assertion is the other half. What publish learnt about the
/// branches is not thrown away — it survives as the frozen union, which is what
/// every response is shaped against.
#[tokio::test]
async fn a_published_version_freezes_a_statements_script_and_not_its_output() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("gen", "gen", "SELECT 1 AS n").await;
    let script = "def build_sql(params):\n  return \"SELECT 1 AS n\"";
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "script": script }]),
    )
    .await;
    f.publish(ep).await;

    let snap = f.live_snapshot(ep).await;
    assert_eq!(
        snap.statements[0].source,
        egret_dataapi_api::StatementSource::Script(script.into()),
        "the version must carry the script the author wrote"
    );
    assert_eq!(
        snap.statements[0].source.sql(),
        None,
        "and no generated text beside it: two definitions with no rule for \
         which one runs is the shape this type exists to refuse"
    );
    assert_eq!(
        snap.statements[0]
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["n"],
        "what publish learnt from the generated text survives as the contract"
    );
}

/// **Package 5, through the real publish route.** A `string` parameter with no
/// `pattern` cannot be published onto an endpoint whose statement is a SCRIPT,
/// and the same endpoint publishes once the parameter is bounded.
///
/// The unit tests in `lifecycle.rs` assert the rule; this asserts that an author
/// meets it as a `400` carrying the sentence, and that nothing was written. The
/// script here emits `{{ region }}` and nothing else — it does not concatenate
/// anything — because the rule is deliberately conservative and a fixture that
/// interpolated would be refused by the sentinel probe instead, which would say
/// nothing about whether this rule exists.
///
/// The second half is what stops it passing for the wrong reason: exactly one
/// field changes, the `pattern`, and the endpoint publishes.
#[tokio::test]
async fn a_scripted_endpoint_cannot_publish_an_unbounded_string_parameter() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("n", "bigint")]));
    let ep = f.create_endpoint("bound", "bound", "SELECT 1 AS n").await;
    let script = "def build_sql(params):\n  return \"SELECT 1 AS n FROM t WHERE r = {{ region }}\"";
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "script": script }]),
    )
    .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "params": [{ "name": "region", "type": "string", "location": "query" }]
        }),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("region") && msg.contains("main"),
        "the author needs the parameter AND the statement that brought the rule: {body}"
    );
    assert!(
        msg.contains("pattern") && msg.contains("^[A-Za-z0-9_]+$"),
        "and an example of what to write, not only that something is missing: {body}"
    );
    assert!(f.versions(ep).await.is_empty(), "nothing may be published");

    // One field changes.
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "params": [{
                "name": "region",
                "type": "string",
                "location": "query",
                "pattern": "^[A-Za-z0-9_]*$",
            }]
        }),
    )
    .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the bounded parameter is publishable, or the rule would be a wall: {body}"
    );
}

#[tokio::test]
async fn a_published_version_is_unaffected_by_later_draft_edits() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("rev", "reports/revenue", "SELECT 1 AS n")
        .await;
    f.publish(ep).await;
    let v1 = f.live_snapshot(ep).await;
    assert_eq!(
        v1.statements[0].source.sql().expect("written SQL"),
        "SELECT 1 AS n"
    );

    // Edit the draft afterwards — including the SQL and the parameter list.
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "statements": [{ "name": "main", "sql": "SELECT 2 AS n WHERE r = {{r}}" }],
            "params": [{ "name": "r", "type": "string", "location": "query" }],
            "limits": { "max_rows": 1, "timeout_seconds": 1 },
        }),
    )
    .await;

    let still = f.live_snapshot(ep).await;
    assert_eq!(
        still.statements[0].source.sql().expect("written SQL"),
        "SELECT 1 AS n",
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
    f.patch_endpoint(
        ep,
        serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 2 AS n" }] }),
    )
    .await;
    f.publish(ep).await; // v2
    assert_eq!(
        f.live_snapshot(ep).await.statements[0]
            .source
            .sql()
            .expect("written SQL"),
        "SELECT 2 AS n"
    );

    f.rollback(ep, 1).await;
    assert_eq!(
        f.live_snapshot(ep).await.statements[0]
            .source
            .sql()
            .expect("written SQL"),
        "SELECT 1 AS n"
    );
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
    assert_eq!(
        f.live_snapshot(a).await.statements[0]
            .source
            .sql()
            .expect("written SQL"),
        "SELECT 1 AS n"
    );
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
        snap.statements[0]
            .columns
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
    f.patch_endpoint(
        ep,
        serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 2 AS n" }] }),
    )
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
        "UPDATE dataapi.endpoints SET statements = $2 WHERE id = $1",
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT 2 AS n" }]),
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(f.versions(ep).await.is_empty());
}

/// **`limits` is covered too, and it is the one field that is not there for the
/// columns' sake.**
///
/// The resolved `columns` do not depend on `limits` at all — but
/// `lifecycle::probe_transform` does: it projects a transform's measured
/// per-row output growth against `limits.max_rows` and refuses a publish that
/// cannot fit under the 1 MiB output cap. That probe runs BEFORE this
/// transaction, and the in-transaction re-validation deliberately does not
/// repeat it (two more Starlark evaluations inside a locked row is what Task 4
/// removed). So without `limits` in the identity check, an author could
/// validate a pass-through transform at a small `max_rows` and have a PATCH
/// land a million while the describes were in flight — freezing a snapshot
/// whose own numbers say every full-sized call must fail.
#[tokio::test]
async fn a_limits_edit_during_describe_is_a_conflict() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("toctou3", "toctou3", "SELECT 1 AS n")
        .await;
    f.port.patch_during_next_describe(
        &f.db,
        "UPDATE dataapi.endpoints SET limits = $2 WHERE id = $1",
        ep,
        serde_json::json!({ "max_rows": 1_000_000, "timeout_seconds": 30 }),
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a limits edit mid-publish must not be frozen behind a probe that never saw it: {body}"
    );
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

    f.patch_endpoint(
        ep,
        serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 2 AS n" }] }),
    )
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
    assert_eq!(
        f.live_snapshot(ep).await.statements[0]
            .source
            .sql()
            .expect("written SQL"),
        "SELECT 2 AS n"
    );
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
        f.live_snapshot(ep).await.statements[0]
            .source
            .sql()
            .expect("written SQL"),
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
    // ALL of them, not `last_sql`: publish now describes one statement per
    // branch, so which one is last is an implementation detail — and the claim
    // being made here is about what was described, not about what was described
    // most recently.
    let described = f.port.all_sql();
    assert!(
        described.iter().any(|s| s.contains("ORDER BY region")),
        "`| ident` emits the AUTHOR's allowed entry, raw: {described:?}"
    );
    assert!(
        described
            .iter()
            .any(|s| s.contains("region = ?") && s.contains("day >= ?")),
        "every conditional branch a value reaches must be described: {described:?}"
    );
    assert!(
        described.iter().any(|s| !s.contains("region = ?")),
        "and so must the branch a caller reaches by omitting the parameter, which \
         is the one a call with no query string takes: {described:?}"
    );
    assert_eq!(
        f.live_snapshot(ep).await.param_gate,
        egret_dataapi_api::PARAM_GATE,
        "a version that passed the gate must say so, or serving cannot tell it \
         from one published before the gate existed"
    );
}

/// The union, and the `required` flag that makes it honest.
///
/// Before this, publish described ONE rendering and froze its columns, so a
/// conditional endpoint returned `503 schema_drift` on every call that took a
/// different branch — including the call with no query string at all, which is
/// the one its own generated OpenAPI advertises as the default.
#[tokio::test]
async fn publishing_freezes_the_union_of_every_branch() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "cond",
            "reports/cond",
            "SELECT id {% if detail %}, name {% endif %}FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "detail", "type": "boolean", "location": "query" }
        ]}),
    )
    .await;
    f.publish(ep).await;

    let snap = f.live_snapshot(ep).await;
    assert_eq!(
        snap.statements[0]
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"],
        "both branches are described, and the union keeps first-seen order"
    );
    let by = |n: &str| {
        snap.statements[0]
            .columns
            .iter()
            .find(|c| c.name == n)
            .unwrap_or_else(|| panic!("no column {n}"))
            .clone()
    };
    assert!(by("id").required, "id is in every branch");
    assert!(
        !by("name").required,
        "name appears only under `detail`, so it is declared and optional"
    );
}

/// The branch a caller reaches by sending NOTHING is described too.
///
/// This is the one the probe assignments alone cannot reach: a string parameter
/// has no closed value space, so its representative value is always truthy and
/// `{% if region %}` always takes its block. Only the absent context renders the
/// other side — and absent is what most calls actually send.
#[tokio::test]
async fn the_branch_an_omitted_parameter_takes_is_described() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("region = ?", vec![("id", "uuid"), ("region", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "omit",
            "reports/omit",
            "SELECT id {% if region %}, region {% endif %}FROM t \
             {% if region %}WHERE region = {{ region }}{% endif %}",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "region", "type": "string", "location": "query" }
        ]}),
    )
    .await;
    f.publish(ep).await;

    let snap = f.live_snapshot(ep).await;
    assert!(
        !snap.statements[0]
            .columns
            .iter()
            .find(|c| c.name == "region")
            .expect("region is declared")
            .required,
        "a call with no query string omits `region`, takes the other branch and \
         returns one column, so `region` is not in every branch. This pins the \
         FLAG only: `shape_rows` still compares names and arity exactly, so that \
         call is a schema_drift 503 until Task 6 makes the check a subset one"
    );
}

/// Two types under one name cannot be unioned into one honest declaration, and
/// this is the same gap that served a `200` carrying an integer under a declared
/// `varchar`.
#[tokio::test]
async fn a_column_whose_type_differs_between_branches_refuses_to_publish() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("v", "varchar")]),
        ("", vec![("v", "integer")]),
    ]));
    let ep = f
        .create_endpoint(
            "conflict",
            "reports/conflict",
            "SELECT {% if detail %}name{% else %}id{% endif %} AS v FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "detail", "type": "boolean", "location": "query" }
        ]}),
    )
    .await;
    let (status, body) = f.publish_raw(ep).await;
    // 400, not 422: `ValidationError` becomes `Error::InvalidInput`, which
    // `egret_core::http` maps to BAD_REQUEST — the same status every other
    // publish rule refusal carries.
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("two types"),
        "the message must name the fix, not just the fault: {body}"
    );
    assert!(
        msg.contains("varchar") && msg.contains("integer"),
        "and both types, or the author cannot tell which branch to CAST: {body}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "a refused publish leaves no version behind"
    );
}

/// The publish half of the Trino parametric-datetime fix, and it exists because
/// nothing else in this crate pinned it.
///
/// **Measured, not assumed**: reverting `lifecycle.rs`'s fold from
/// `compare_column_types` back to `existing.ty != spec.ty` left all 434 tests in
/// this crate green.
/// [`a_column_whose_type_differs_between_branches_refuses_to_publish`] pins only
/// the REFUSAL side (`varchar` vs `integer`), so the tolerance was protected by
/// nothing at all and a later "simplification" back to `!=` would have
/// reintroduced the exact defect the fix exists for, silently.
///
/// The two spellings here are not invented. `describe_branches` computes
/// `bind_count` PER STATEMENT, and `describe_with_params(sql, 0, _)`
/// early-returns into `Connector::describe`, whose Trino implementation is the
/// trait default — `execute` over a `LIMIT 0` wrapper. So a branch that
/// interpolates NO parameter is typed through the client protocol, which
/// suppresses datetime precision without
/// `X-Trino-Client-Capabilities: PARAMETRIC_DATETIME`, and a branch that
/// interpolates one is typed through `DESCRIBE OUTPUT`, which states it. One
/// unchanging column, two spellings, and a `400 ColumnTypeConflict` on a
/// definition nobody changed. Measured live against Trino 483.
///
/// # Why BOTH orders are published
///
/// The union fold promotes the precision-bearing spelling, and an assertion
/// that the frozen contract carries `timestamp(0)` only measures that promotion
/// when the accumulator did NOT already hold it. The first version of this test
/// keyed its arms so that the parameterised branch — which
/// `branch_renderings` enumerates FIRST — supplied `timestamp(0)`, which meant
/// "keep the more specific" and "keep the first" were indistinguishable:
/// deleting the fold's effect entirely left this test green. So the same
/// definition is published twice with the two spellings swapped between the
/// branches, and both must freeze `timestamp(0)`. Whichever branch the
/// enumerator visits first, exactly one of the two is order-sensitive, and no
/// change to that order can hide the fold again.
///
/// The second endpoint is deliberately BACKWARDS with respect to Trino — there
/// the parameterised branch is the one that states the precision. It is a
/// synthetic that exists to fix the enumeration order, not a claim about any
/// engine.
///
/// The arms carry no `""` fallback on purpose: `PerSql` panics when no arm
/// matches, so the needles below are a measurement of what actually renders
/// (`… WHERE k = ?` and `… WHERE k IS NOT NULL`) rather than an assumption
/// about it. The first draft of this test keyed an arm on `"detail"`, which the
/// TEMPLATE contains and the RENDERED statement does not — that arm matched
/// neither branch, `""` matched both, and the fixture collapsed to a single
/// spelling.
#[tokio::test]
async fn a_datetime_column_two_branches_spell_differently_still_publishes() {
    let f = skip_without_db!();

    for (case, on_param_branch, on_bare_branch) in [
        ("param-branch-precise", "timestamp(0)", "timestamp"),
        ("bare-branch-precise", "timestamp", "timestamp(0)"),
    ] {
        f.port.set(Describe::PerSql(vec![
            ("= ?", vec![("v", on_param_branch)]),
            ("IS NOT NULL", vec![("v", on_bare_branch)]),
        ]));
        let ep = f
            .create_endpoint(
                case,
                &format!("reports/{case}"),
                "SELECT ts AS v FROM t WHERE {% if detail %}k = {{ detail }}{% else %}k IS NOT NULL{% endif %}",
            )
            .await;
        f.patch_endpoint(
            ep,
            serde_json::json!({ "params": [
                { "name": "detail", "type": "string", "location": "query" }
            ]}),
        )
        .await;

        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "[{case}] one column whose precision one branch's typing path \
             suppressed is not two types, and must not refuse the publish: {body}"
        );

        let snap = f.live_snapshot(ep).await;
        let v = snap.statements[0]
            .columns
            .iter()
            .find(|c| c.name == "v")
            .expect("v is declared");
        assert_eq!(
            v.ty, "timestamp(0)",
            "[{case}] the union must keep the precision-bearing spelling, not \
             whichever branch happened to be enumerated first"
        );
    }
}

/// The boundary of the tolerance above, on the SAME path. Two branches that
/// each STATE a precision, and state different ones, is a real conflict and
/// still refuses — the relaxation is only for a precision one side suppressed.
#[tokio::test]
async fn two_branches_stating_different_datetime_precisions_still_refuse() {
    let f = skip_without_db!();
    // Keyed the same way as the test above, and for the same reason.
    f.port.set(Describe::PerSql(vec![
        ("= ?", vec![("v", "timestamp(3)")]),
        ("IS NOT NULL", vec![("v", "timestamp(6)")]),
    ]));
    let ep = f
        .create_endpoint(
            "dtc",
            "reports/dtc",
            "SELECT ts AS v FROM t WHERE {% if detail %}k = {{ detail }}{% else %}k IS NOT NULL{% endif %}",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "detail", "type": "string", "location": "query" }
        ]}),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("timestamp(3)") && msg.contains("timestamp(6)"),
        "and both spellings, or the author cannot tell which branch to CAST: {body}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "a refused publish leaves no version behind"
    );
}

/// **Three branches, and the fold's promotion is what makes this a refusal.**
///
/// A bare `timestamp` agrees with `timestamp(3)` AND with `timestamp(6)`, so an
/// accumulator that kept the FIRST spelling never compared the two stated
/// precisions against each other: the definition published, and the frozen
/// contract named neither precision. Measured on the code as it stood one
/// commit ago, not argued — it froze `timestamp`.
///
/// Promoting the precision-bearing spelling turns the accumulator into
/// `timestamp(3)` as soon as it meets one, so the branch carrying
/// `timestamp(6)` is compared against a STATED precision and correctly refused.
/// That is what makes the promotion a correctness fix rather than a cosmetic
/// one, and it is why the assertion below is on the MESSAGE: a refusal naming
/// the bare spelling would mean something else refused.
///
/// The fixture is arranged so that ONLY the promotion can produce this refusal
/// — see the comment on the arms. An earlier arrangement put `timestamp(6)` on
/// the first-enumerated statement, which made the accumulator precision-bearing
/// immediately and refused with or without the promotion; that version stayed
/// green under the mutation that deletes the fold, which is how it was caught.
#[tokio::test]
async fn three_branches_spanning_two_stated_precisions_refuse() {
    let f = skip_without_db!();
    // The BARE spelling goes on the statement enumerated FIRST, so the
    // accumulator starts precision-free and only the promotion can bring the two
    // stated precisions into contact. Which statement that is was MEASURED, not
    // assumed: a `PerSql` table with no matching arm panics on the first
    // statement it is handed, and it named
    // `SELECT ts AS v FROM t WHERE 1 = 1 AND k = ? AND j = ?`.
    //
    // Arms are ordered most-specific first, because `PerSql` matches on
    // `contains` and the both-parameter statement also contains `k = ?`.
    f.port.set(Describe::PerSql(vec![
        ("k = ? AND j = ?", vec![("v", "timestamp")]),
        ("k = ?", vec![("v", "timestamp(3)")]),
        ("j = ?", vec![("v", "timestamp(3)")]),
        ("1 = 1", vec![("v", "timestamp(6)")]),
    ]));
    let ep = f
        .create_endpoint(
            "dt3",
            "reports/dt3",
            "SELECT ts AS v FROM t WHERE 1 = 1\
             {% if a %} AND k = {{ a }}{% endif %}\
             {% if b %} AND j = {{ b }}{% endif %}",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "a", "type": "string", "location": "query" },
            { "name": "b", "type": "string", "location": "query" }
        ]}),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a bare `timestamp` agrees with both stated precisions, so keeping the \
         FIRST spelling published a contract naming neither: {body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("timestamp(3)") && msg.contains("timestamp(6)"),
        "the refusal must name the two STATED precisions that disagree, not the \
         bare spelling that agreed with both: {body}"
    );
    assert!(f.versions(ep).await.is_empty());
}

/// `output.shape: "arrays"` is positional — `shape_rows` zips the live and
/// frozen column lists by index — so a definition whose branches resolve to
/// different columns has no single order to freeze and must be refused.
#[tokio::test]
async fn an_arrays_endpoint_that_branches_refuses_to_publish() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "arr",
            "reports/arr",
            "SELECT id {% if detail %}, name {% endif %}FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "params": [
                { "name": "detail", "type": "boolean", "location": "query" }
            ],
            "output": { "shape": "arrays" }
        }),
    )
    .await;
    let (status, body) = f.publish_raw(ep).await;
    // 400, not 422: every `ValidationError` becomes `Error::InvalidInput`,
    // which `egret_core::http` maps to BAD_REQUEST with an `error` field — the
    // same shape every other publish rule refusal carries.
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("objects"),
        "the message must point at the fix: {body}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "a refused publish leaves no version behind"
    );
}

/// The rule must cost a non-branching `arrays` endpoint nothing: this is the
/// other half of the same property, from the same shape one parameter apart,
/// so a rule that refused every `arrays` endpoint outright would fail here.
#[tokio::test]
async fn an_arrays_endpoint_that_does_not_branch_still_publishes() {
    let f = skip_without_db!();
    f.port
        .set(Describe::Columns(vec![("id", "uuid"), ("name", "varchar")]));
    let ep = f
        .create_endpoint("arr2", "reports/arr2", "SELECT id, name FROM t")
        .await;
    f.patch_endpoint(ep, serde_json::json!({ "output": { "shape": "arrays" } }))
        .await;
    f.publish(ep).await; // must not refuse
}

/// `representative_value` picks `allowed[0]` (`lifecycle.rs`), so before this the
/// ORDER the author wrote `allowed` in decided the published contract.
#[tokio::test]
async fn every_allowed_enum_value_is_described_not_only_the_first() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "enum",
            "reports/enum",
            "SELECT id {% if mode | value == 'full' %}, name {% endif %}FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "mode", "type": "enum", "location": "query", "required": true,
              "allowed": ["summary", "full"] }
        ]}),
    )
    .await;
    f.publish(ep).await;
    let snap = f.live_snapshot(ep).await;
    assert!(
        snap.statements[0].columns.iter().any(|c| c.name == "name"),
        "`full` is not allowed[0], and it was never described before M1: {:?}",
        snap.statements[0].columns
    );
}

/// The cost argument, measured: describing every branch must not cost a describe
/// per probe on a definition that has no branches.
#[tokio::test]
async fn a_non_conditional_definition_describes_exactly_once() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint(
            "plain",
            "reports/plain",
            "SELECT id FROM t WHERE r = {{ region }}",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "region", "type": "string", "location": "query" }
        ]}),
    )
    .await;
    let before = f.port.call_count();
    f.publish(ep).await;
    assert_eq!(
        f.port.call_count() - before,
        1,
        "deduplication is the whole cost argument; `at least one` would pass with \
         it removed"
    );
}

/// The publish fan-out is bounded, and the bound is a REFUSAL.
///
/// Truncating the described set would drop whichever branch fell off the end,
/// and an undescribed branch is a column missing from the union — the exact
/// `503 schema_drift` this gate exists to remove. So the definition is refused
/// instead, and the author is told which knob reduces it.
///
/// Both sides are asserted from the same shape, one parameter apart, because a
/// test that only proved the refusal would pass against a limit of zero.
#[tokio::test]
async fn a_definition_with_more_branches_than_publish_will_describe_is_refused() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    // Measured: n independent optional booleans render to 57 distinct statements
    // at n = 10 and 68 at n = 11, against a limit of 64. Ten independent boolean
    // toggles on one endpoint is already past what anyone writes; eleven is the
    // first count that costs more than the budget.
    let build = |n: usize| {
        let mut sql = String::from("SELECT id FROM t WHERE 1=1");
        let mut params = Vec::new();
        for i in 0..n {
            sql.push_str(&format!(
                " {{% if p{i} %}} AND a{i} = {{{{ p{i} }}}}{{% endif %}}"
            ));
            params.push(serde_json::json!({
                "name": format!("p{i}"), "type": "boolean", "location": "query"
            }));
        }
        (sql, serde_json::Value::from(params))
    };

    let (sql, params) = build(10);
    let ok = f.create_endpoint("under", "reports/under", &sql).await;
    f.patch_endpoint(ok, serde_json::json!({ "params": params }))
        .await;
    let before = f.port.call_count();
    let (status, body) = f.publish_raw(ok).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ten optional booleans is under the limit and must still publish: {body}"
    );
    let described = f.port.call_count() - before;
    assert!(
        described > 10 && described <= 64,
        "and it must have cost real describes, all of them within the budget: \
         {described}"
    );

    let (sql, params) = build(11);
    let over = f.create_endpoint("over", "reports/over", &sql).await;
    f.patch_endpoint(over, serde_json::json!({ "params": params }))
        .await;
    let before = f.port.call_count();
    let (status, body) = f.publish_raw(over).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("68") && msg.contains("64"),
        "the author needs the measured count AND the limit, or they cannot tell \
         how far over they are: {body}"
    );
    assert!(
        msg.contains("`default`") && msg.contains("split"),
        "and what to do about it, not just the number: {body}"
    );
    assert_eq!(
        f.port.call_count(),
        before,
        "the refusal must come BEFORE the first describe — the point of the \
         limit is that the engine never sees the fan-out"
    );
    assert!(
        f.versions(over).await.is_empty(),
        "nothing may be published"
    );
}

/// Two same-named columns of different types in ONE statement is a different
/// fault from two branches disagreeing, and the fold catches both — so it must
/// not tell an author with no `{% if %}` in their definition to go and look at
/// their branches.
#[tokio::test]
async fn two_same_named_columns_in_one_statement_are_diagnosed_as_an_alias_problem() {
    let f = skip_without_db!();
    f.port
        .set(Describe::Columns(vec![("id", "bigint"), ("id", "varchar")]));
    let ep = f
        .create_endpoint("dup", "reports/dup", "SELECT a.id, b.id FROM a JOIN b")
        .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("two columns named") && msg.contains("alias"),
        "the repair is an alias: {body}"
    );
    assert!(
        !msg.contains("branch"),
        "this definition has no branch, and sending the author to look for one \
         is the misdiagnosis this arm exists to avoid: {body}"
    );
}

/// The duplicate-name refusal does not depend on the types being different.
///
/// `SELECT a.id, b.id FROM a JOIN b` with BOTH `bigint` published green before
/// this: the union deduplicates by name to one column, and `shape_rows` then
/// compares a live width of 2 against a frozen width of 1 and returns
/// `503 schema_drift` on EVERY call — not on some branch, on every one. Which is
/// the publish-green-then-always-fail class this task exists to remove, one
/// keystroke from the differing-type case above.
#[tokio::test]
async fn two_same_named_columns_of_the_same_type_are_refused_too() {
    let f = skip_without_db!();
    f.port
        .set(Describe::Columns(vec![("id", "bigint"), ("id", "bigint")]));
    let ep = f
        .create_endpoint(
            "dupsame",
            "reports/dupsame",
            "SELECT a.id, b.id FROM a JOIN b",
        )
        .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "identical types make the duplicate name MORE invisible, not less: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("two columns named"),
        "{body}"
    );
    assert!(f.versions(ep).await.is_empty(), "nothing may be published");
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
    f.patch_endpoint(
        ep,
        serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 2 AS n" }] }),
    )
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
    assert_eq!(
        f.live_snapshot(ep).await.statements[0]
            .source
            .sql()
            .expect("written SQL"),
        "SELECT 1 AS n"
    );
}

// ---------- Authorization and containment ----------

/// `:read` cannot author, and each lifecycle route is its own `require` call —
/// permissions here are FLAT, so nothing but the explicit check stands between a
/// reader and a publish.
///
/// `trash` and `restore` are in this list, and they are the two that needed a
/// fixture of their own. Driven against `ep` — which is LIVE — an unguarded trash
/// would still be refused, by the store's `AND state <> 'live'`, and an unguarded
/// restore by its `AND deleted_at IS NOT NULL`; the assertion would then hold for
/// a reason that has nothing to do with the permission. `trashable` and `trashed`
/// are each in exactly the state their route accepts, so deleting the `require`
/// turns those two calls into 200s that write.
#[tokio::test]
async fn read_permission_cannot_drive_the_lifecycle() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let trashable = f
        .create_endpoint("trashable", "trashable", "SELECT 1 AS n")
        .await;
    let trashed = f
        .create_trashed_endpoint("trashed", "trashed", "SELECT 1 AS n")
        .await;
    let reader =
        principal_with_in(&f.db, f.port.clone(), &["data_api_endpoint:read"], f.ws_id).await;

    for (target, route, body) in [
        (ep, "validate", None),
        (ep, "publish", Some(serde_json::json!({}))),
        (ep, "rollback", Some(serde_json::json!({ "version": 1 }))),
        (ep, "disable", None),
        (ep, "enable", None),
        (trashable, "trash", None),
        (trashed, "restore", None),
    ] {
        let (status, b) = f
            .send(
                &reader,
                "POST",
                &format!("{}/{target}/{route}", f.collection()),
                body,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{route}: {b}");
    }
    // …and neither row moved. A 403 that had already written would read exactly
    // the same on the status line above.
    assert!(
        f.deleted_at(trashable).await.is_none(),
        "a refused trash must not have written deleted_at"
    );
    assert!(
        f.deleted_at(trashed).await.is_some(),
        "a refused restore must not have cleared deleted_at"
    );
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
/// and must not be able to publish, roll back, disable, trash or restore it.
///
/// `trash` and `restore` get their own targets for the same reason as in
/// `read_permission_cannot_drive_the_lifecycle` above, and here it decides
/// whether the test binds at all: aimed at the LIVE `ep`, a store that had lost
/// its `workspace_id` predicate would still answer 404 — `trash` falls through to
/// a workspace-scoped `get` on the no-row path — so the assertion would pass
/// against the very defect it is named for. Against a draft in this workspace's
/// trash-able state, the unscoped write succeeds and returns 200.
#[tokio::test]
async fn the_lifecycle_of_another_workspaces_endpoint_is_not_found() {
    let f = skip_without_db!();
    let ep = f.create_endpoint("rev", "rev", "SELECT 1 AS n").await;
    f.publish(ep).await;
    let trashable = f
        .create_endpoint("trashable", "trashable", "SELECT 1 AS n")
        .await;
    let trashed = f
        .create_trashed_endpoint("trashed", "trashed", "SELECT 1 AS n")
        .await;

    let (other_ws, other_slug) = insert_workspace(&f.db, f.admin, "lifecycle-other").await;
    let outsider = principal_with_in(&f.db, f.port.clone(), MANAGER, other_ws).await;

    for (method, target, route, body) in [
        ("POST", ep, "validate", None),
        ("POST", ep, "publish", Some(serde_json::json!({}))),
        (
            "POST",
            ep,
            "rollback",
            Some(serde_json::json!({ "version": 1 })),
        ),
        ("POST", ep, "disable", None),
        ("POST", ep, "enable", None),
        ("GET", ep, "versions", None),
        ("POST", trashable, "trash", None),
        ("POST", trashed, "restore", None),
    ] {
        let (status, b) = f
            .send(
                &outsider,
                method,
                &format!("/api/ws/{other_slug}/data-api/endpoints/{target}/{route}"),
                body,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{route}: {b}");
    }
    // Untouched.
    let (state, _) = f.state_of(ep).await;
    assert_eq!(state, "live");
    assert_eq!(f.versions(ep).await.len(), 1);
    assert!(
        f.deleted_at(trashable).await.is_none(),
        "an outsider's trash must not have written deleted_at"
    );
    assert!(
        f.deleted_at(trashed).await.is_some(),
        "an outsider's restore must not have cleared deleted_at"
    );
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
    // The whole answer, not a field of it: `POST /validate` reports exactly what
    // publish would freeze, and since M2 that is one entry PER STATEMENT. A
    // single flattened `columns` list could not say which statement a column
    // belongs to, and two statements each returning a `total` would read as one
    // column declared twice.
    assert_eq!(
        body["statements"],
        serde_json::json!([{
            "name": "main",
            "sql": "SELECT id, n FROM t",
            "columns": [
                { "name": "id", "ty": "uuid", "required": true },
                { "name": "n", "ty": "bigint", "required": true }
            ]
        }])
    );
    assert!(f.versions(ep).await.is_empty(), "validate must not publish");
    assert_eq!(f.state_of(ep).await.0, "draft");
}

/// The same route over several statements, adjacent to the one-statement case
/// above so the two shapes are read together.
///
/// `POST /validate` and `POST /publish` share one function, so this is the
/// authoring surface of the same loop the publish tests drive: an author must
/// be able to see both contracts before committing to either.
#[tokio::test]
async fn validate_reports_one_column_set_per_statement() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("FROM ledger", vec![("total", "bigint")]),
        ("FROM sales", vec![("id", "uuid")]),
    ]));
    let ep = f.create_endpoint("multi", "multi", "SELECT 1 AS n").await;
    f.set_statements(
        ep,
        serde_json::json!([
            { "name": "rows", "sql": "SELECT id FROM sales" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM ledger" },
        ]),
    )
    .await;

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
        body["statements"],
        serde_json::json!([
            {
                "name": "rows",
                "sql": "SELECT id FROM sales",
                "columns": [{ "name": "id", "ty": "uuid", "required": true }]
            },
            {
                "name": "total",
                "sql": "SELECT count(*) AS total FROM ledger",
                "columns": [{ "name": "total", "ty": "bigint", "required": true }]
            }
        ])
    );
    assert!(f.versions(ep).await.is_empty(), "validate must not publish");
}

// ---------------------------------------------------------------------------
// Whole-branch review, Important 2 — `output.naming` over the UNION
// ---------------------------------------------------------------------------

/// **Two branches whose column names collapse onto one key under `naming`.**
///
/// Three of the four places that share the "what is a column" concept apply
/// `apply_naming`; the publish-side refusal did not. `describe_branches`
/// refuses duplicates by RAW name, while `shape_rows` and `data_schema` both
/// refuse on the naming-APPLIED key over the same frozen union.
///
/// Before M1 the frozen list was one branch's columns, so a collision needed
/// both spellings in one projection — which the raw-name check already caught.
/// Widening the frozen list to the union of every branch is what made a
/// CROSS-branch collision possible, and nothing at publish looked for it. The
/// result published green and then:
///
/// * every call, on either branch, answered `503 output_naming_collision`; and
/// * `openapi_gen` returned `NamingCollision`, which is a DOCUMENT-level error
///   — it fails the whole workspace's generated OpenAPI, taking every unrelated
///   endpoint's documentation down with it.
///
/// So the refusal belongs at publish, where the author is present and the blast
/// radius is one endpoint.
#[tokio::test]
async fn publish_refuses_a_union_whose_names_collide_under_output_naming() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("total_revenue", vec![("total_revenue", "bigint")]),
        ("totalRevenue", vec![("totalRevenue", "bigint")]),
    ]));
    let ep = f
        .create_endpoint(
            "collide",
            "reports/collide",
            "SELECT {% if d %}total_revenue{% else %}totalRevenue{% endif %} FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "params": [{ "name": "d", "type": "boolean", "location": "query" }],
            "output": { "shape": "objects", "naming": "camel" }
        }),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a union that collapses onto one key must be refused at publish, not \
         served as a 503 on every branch: {body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("totalRevenue") && msg.contains("total_revenue"),
        "the author must be told BOTH declared names, since either one is the \
         repair: {body}"
    );
    // A CONTIGUOUS span across a line continuation, not two `contains` on the
    // names. The first version of this message lost its `\` continuations and
    // shipped four runs of ten spaces — "…both become `totalRevenue` under
    // ␣␣␣␣␣␣␣␣␣␣`output.naming: camel`…" — which every name-only assertion
    // reads as passing. Asserting a span is what makes the rendered sentence,
    // rather than its vocabulary, the thing under test.
    assert!(
        msg.contains("both become `totalRevenue` under `output.naming: camel`"),
        "the message must read as a sentence, not as words with the line \
         continuations dropped: {msg}"
    );
    assert!(
        !msg.contains("  "),
        "no run of two spaces belongs in an author-facing message: {msg}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "and nothing may have been frozen"
    );
    assert_eq!(f.state_of(ep).await.0, "draft");
}

/// The same definition under `naming: as_is`, which is the control.
///
/// `total_revenue` and `totalRevenue` are two distinct keys there, so the
/// endpoint is perfectly serviceable and publish must NOT refuse it. Without
/// this, a refusal that ignored `naming` entirely — or one that refused any
/// two-branch union — would pass the test above.
#[tokio::test]
async fn publish_allows_the_same_union_when_naming_keeps_the_names_apart() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("total_revenue", vec![("total_revenue", "bigint")]),
        ("totalRevenue", vec![("totalRevenue", "bigint")]),
    ]));
    let ep = f
        .create_endpoint(
            "nocollide",
            "reports/nocollide",
            "SELECT {% if d %}total_revenue{% else %}totalRevenue{% endif %} FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({
            "params": [{ "name": "d", "type": "boolean", "location": "query" }],
            "output": { "shape": "objects", "naming": "as_is" }
        }),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let snap = f.live_snapshot(ep).await;
    let mut names: Vec<&str> = snap.statements[0]
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["totalRevenue", "total_revenue"]);
}

// ---------------------------------------------------------------------------
// Whole-branch review, Important 3 — `output` moved through the publish window
// ---------------------------------------------------------------------------

/// **`output` is patchable, and publish never noticed it had moved.**
///
/// `ValidationError::BranchingArraysShape` is raised by `validate()` against
/// `draft.output.shape`, BEFORE the transaction. `publish()` then re-reads the
/// row under lock and re-runs only `validate_definition`, which knows nothing
/// about `output`; its staleness guard compared `sql`, `database_id` and
/// `params` and not `output`. So an author who validates a branching definition
/// as `objects` and PATCHes `shape: arrays` during the describe round trips
/// froze an `arrays` snapshot over a branching union — and `shape_rows`, which
/// zips the live row against the frozen list by INDEX, then answered `503` for
/// every branch but the widest, permanently.
///
/// This is the same window, and the same shape of hole, that the guard's own
/// comment describes as having been closed for `cache.scope`.
#[tokio::test]
async fn an_output_edit_during_describe_is_a_conflict() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "toctou-out",
            "toctou-out",
            "SELECT id {% if detail %}, name {% endif %}FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [
            { "name": "detail", "type": "boolean", "location": "query" }
        ]}),
    )
    .await;

    f.port.patch_during_next_describe(
        &f.db,
        "UPDATE dataapi.endpoints SET output = $2 WHERE id = $1",
        ep,
        serde_json::json!({ "shape": "arrays", "naming": "as_is" }),
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an `output` that moved under the describe makes the resolved columns \
         stale in exactly the way `sql` does: {body}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "and no arrays snapshot may have been frozen over a branching union"
    );
    assert_eq!(f.state_of(ep).await.0, "draft");
}

/// **An enum's ninth-and-later allowed values are branches too.**
///
/// `render::candidates` answers an enum with its `allowed` list and nothing
/// else, cut at `MAX_ENUM_CANDIDATES`. That cap was an eighth of the probe
/// budget while its only consumer was `check_dynamic`, where missing a
/// candidate costs sensitivity; it now decides a PUBLISHED CONTRACT, and an
/// allowed value never probed is a branch never described — so `?mode=a10` was
/// a permanent `503 schema_drift` on an endpoint that published green.
///
/// The parameter is `required` here on purpose: that removes the absent context
/// AND the resting context (no `default`), so the only thing that can put `a10`
/// in front of the engine is the candidate list itself. Measured at the old cap
/// of 8: one describe, and `extra` missing from the union.
#[tokio::test]
async fn publish_describes_an_enum_value_past_the_old_candidate_cap() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("extra", vec![("id", "uuid"), ("extra", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "enum9",
            "reports/enum9",
            "SELECT id {% if mode | value == 'a10' %}, extra {% endif %}FROM t",
        )
        .await;
    f.patch_endpoint(
        ep,
        serde_json::json!({ "params": [{
            "name": "mode", "type": "enum", "location": "query", "required": true,
            "allowed": ["a0","a1","a2","a3","a4","a5","a6","a7","a8","a9","a10","a11"]
        }]}),
    )
    .await;
    f.publish(ep).await;

    let snap = f.live_snapshot(ep).await;
    assert_eq!(
        snap.statements[0]
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "extra"],
        "the branch on `allowed[10]` was never described, so `?mode=a10` can \
         only ever be a 503"
    );
    assert!(
        !snap.statements[0]
            .columns
            .iter()
            .find(|c| c.name == "extra")
            .expect("extra")
            .required,
        "and it is optional — eleven of the twelve values do not return it"
    );
}

// ---------------------------------------------------------------------------
// M2 Task 5 — publish validates and describes EVERY statement
// ---------------------------------------------------------------------------

/// **The feature this task exists for.** Two statements, described
/// independently, each freezing its own column set under its own name.
///
/// The bridge this replaces refused the endpoint outright, because freezing
/// `statements[0]` would have published a contract short a whole result set —
/// 200s missing a half nobody reports as a bug. So the assertion is not merely
/// that it publishes: it is that BOTH column sets are in the snapshot, in the
/// author's order, with the names they were written under.
///
/// `Describe::PerSql` is keyed on the RELATION, not on a parameter name. A
/// fixture keyed on a placeholder cannot work here — the renderer consumes
/// `{{name}}` into a bind marker before the port ever sees the statement — and
/// that mistake silently collapsed a two-branch fixture to one in M1.
#[tokio::test]
async fn a_two_statement_endpoint_freezes_a_column_set_for_each() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("FROM ledger", vec![("total", "bigint")]),
        (
            "FROM sales",
            vec![("id", "uuid"), ("amount", "decimal(10,2)")],
        ),
    ]));
    let ep = f
        .create_endpoint("pair", "reports/pair", "SELECT 1 AS n")
        .await;
    f.set_statements(
        ep,
        serde_json::json!([
            { "name": "rows", "sql": "SELECT id, amount FROM sales" },
            { "name": "total", "sql": "SELECT count(*) AS total FROM ledger" },
        ]),
    )
    .await;

    let before = f.port.call_count();
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.port.call_count() - before,
        2,
        "neither statement branches, so each costs exactly one describe — and \
         BOTH must have been described, which is the whole point"
    );

    let snap = f.live_snapshot(ep).await;
    assert_eq!(
        snap.statements.len(),
        2,
        "both statements must be frozen: {snap:?}"
    );
    let names: Vec<&str> = snap.statements.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["rows", "total"],
        "in the author's order, and under the author's names"
    );
    assert_eq!(
        snap.statements[0].source.sql().expect("written SQL"),
        "SELECT id, amount FROM sales"
    );
    assert_eq!(
        snap.statements[1].source.sql().expect("written SQL"),
        "SELECT count(*) AS total FROM ledger"
    );
    let columns = |i: usize| -> Vec<&str> {
        snap.statements[i]
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect()
    };
    assert_eq!(
        columns(0),
        vec!["id", "amount"],
        "each statement freezes ITS OWN columns, not the endpoint's first"
    );
    assert_eq!(columns(1), vec!["total"]);
    assert_eq!(
        snap.statements[0].columns[1].ty, "decimal(10,2)",
        "the engine's own type string is frozen per statement"
    );
    assert!(
        snap.statements
            .iter()
            .all(|s| s.columns.iter().all(|c| c.required)),
        "neither statement branches, so every column is in every branch of it"
    );
}

/// The single-statement gate is untouchable, and it applies to EVERY statement.
///
/// A `;` in the second one is the interesting position: the first is perfectly
/// good SQL, so a loop that validated only until it found something publishable
/// — or only the first element, as the bridge did — would let this through and
/// freeze a definition carrying a statement separator into an immutable version.
///
/// The message is asserted, not the status: `400` answers a dozen definition
/// rules here, and reading which refusal fired is the only thing that says the
/// `;` scan is what stopped it.
#[tokio::test]
async fn the_single_statement_gate_still_fires_for_a_later_statement() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("gate", "reports/gate", "SELECT 1 AS n")
        .await;
    for (sql, needle, why) in [
        (
            "SELECT 1; DROP TABLE t",
            "only one statement may be published",
            "a separator in the second statement",
        ),
        (
            "SELECT 1 #x",
            "may not appear in a published definition",
            "a `#` in the second statement",
        ),
        (
            "INSERT INTO t VALUES (1)",
            "only SELECT",
            "a write in the second statement",
        ),
    ] {
        f.set_statements(
            ep,
            serde_json::json!([
                { "name": "rows", "sql": "SELECT 1 AS n" },
                { "name": "total", "sql": sql },
            ]),
        )
        .await;
        let before = f.port.call_count();
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(
            body["error"].as_str().unwrap_or_default().contains(needle),
            "{why}: expected the reason to mention {needle:?}, got {body}"
        );
        assert_eq!(
            f.port.call_count(),
            before,
            "{why}: a multi-statement string must never reach a coordinator"
        );
        assert!(f.versions(ep).await.is_empty(), "{why} must not publish");
    }
}

/// **The describe budget belongs to the ENDPOINT, not to each statement**
/// (spec §4.1.1). Ten statements must not license ten times the engine round
/// trips.
///
/// The expected total is MEASURED rather than written down: the same definition
/// is published alone first, its describes counted, and the pair is then
/// required to refuse at exactly twice that. A hard-coded number would still
/// pass against a per-statement limit if the arithmetic happened to line up;
/// deriving it from the single-statement cost cannot.
///
/// Both halves are under the limit on their own — the first one is PROVEN so,
/// by publishing — so the only thing that can refuse the pair is a budget that
/// sums across statements.
#[tokio::test]
async fn the_describe_budget_is_the_endpoints_total_not_one_per_statement() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    // Measured in M1: n independent optional booleans render to 57 distinct
    // statements at n = 10, against a limit of 64. Two of those is 114.
    let build = |relation: &str, n: usize| {
        let mut sql = format!("SELECT id FROM {relation} WHERE 1=1");
        let mut params = Vec::new();
        for i in 0..n {
            sql.push_str(&format!(
                " {{% if p{i} %}} AND a{i} = {{{{ p{i} }}}}{{% endif %}}"
            ));
            params.push(serde_json::json!({
                "name": format!("p{i}"), "type": "boolean", "location": "query"
            }));
        }
        (sql, serde_json::Value::from(params))
    };
    let (sql_a, params) = build("t", 10);
    let (sql_b, _) = build("u", 10);

    // What ONE of them costs, and proof that it is publishable on its own.
    let alone = f.create_endpoint("alone", "reports/alone", &sql_a).await;
    f.patch_endpoint(alone, serde_json::json!({ "params": params.clone() }))
        .await;
    let before = f.port.call_count();
    let (status, body) = f.publish_raw(alone).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ten optional booleans is under the limit for one statement: {body}"
    );
    let per_statement = f.port.call_count() - before;
    assert!(
        per_statement > 10 && per_statement <= 64,
        "and it must have cost real describes, within the budget: {per_statement}"
    );

    // The same shape twice on ONE endpoint. Neither half is over; the endpoint is.
    let pair = f.create_endpoint("wide", "reports/wide", &sql_a).await;
    f.patch_endpoint(pair, serde_json::json!({ "params": params }))
        .await;
    f.set_statements(
        pair,
        serde_json::json!([
            { "name": "rows", "sql": sql_a },
            { "name": "also", "sql": sql_b },
        ]),
    )
    .await;
    let before = f.port.call_count();
    let (status, body) = f.publish_raw(pair).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "two statements each just under the cap must refuse TOGETHER: {body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    let total = per_statement * 2;
    assert!(
        msg.contains(&total.to_string()),
        "the author needs the ENDPOINT's total ({total}), not one statement's \
         ({per_statement}): {msg}"
    );
    assert!(
        msg.contains("64"),
        "and the limit it is measured against: {msg}"
    );
    assert!(
        msg.contains("across its statements"),
        "and the message must read correctly now that `statement` and `branch` \
         are two different things: {msg}"
    );
    assert!(
        msg.contains("`default`") && msg.contains("split"),
        "and what to do about it, not just the number: {msg}"
    );
    assert!(
        !msg.contains("  "),
        "no run of two spaces belongs in an author-facing message: {msg}"
    );
    assert_eq!(
        f.port.call_count(),
        before,
        "the refusal must come BEFORE the first describe — the point of the \
         limit is that the engine never sees the fan-out, and summing after \
         describing the first statement would have cost it 57 round trips"
    );
    assert!(
        f.versions(pair).await.is_empty(),
        "nothing may be published"
    );
}

/// Every rule M1 built over ONE definition's branches must still fire when the
/// definition that breaks it is the SECOND statement.
///
/// This is the shape of hole the brief for this task names outright: converting
/// some of these to per-statement and not others leaves a rule that silently
/// stops applying to everything after `statements[0]`. Each case here puts a
/// clean, non-branching statement FIRST, so a loop that stopped at the first
/// publishable statement would pass every one of them.
///
/// The message is read in each case, not just the status.
#[tokio::test]
async fn every_branch_rule_still_fires_for_a_later_statement() {
    let f = skip_without_db!();

    // 1. `output.shape: arrays` over a statement whose branches differ.
    {
        f.port.set(Describe::PerSql(vec![
            ("FROM ledger", vec![("total", "bigint")]),
            (", name", vec![("id", "uuid"), ("name", "varchar")]),
            ("", vec![("id", "uuid")]),
        ]));
        let ep = f
            .create_endpoint("arr", "reports/arr", "SELECT 1 AS n")
            .await;
        f.patch_endpoint(
            ep,
            serde_json::json!({
                "params": [{ "name": "detail", "type": "boolean", "location": "query" }],
                "output": { "shape": "arrays" }
            }),
        )
        .await;
        f.set_statements(
            ep,
            serde_json::json!([
                { "name": "total", "sql": "SELECT count(*) AS total FROM ledger" },
                { "name": "rows", "sql": "SELECT id {% if detail %}, name {% endif %}FROM t" },
            ]),
        )
        .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "arrays: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("`output.shape: arrays` cannot express"),
            "arrays: the positional-row refusal must be the one that fired: {body}"
        );
        assert!(f.versions(ep).await.is_empty(), "arrays must not publish");
    }

    // 2. Two columns of one name inside a single branch of the second statement.
    {
        f.port.set(Describe::PerSql(vec![
            ("JOIN b", vec![("id", "bigint"), ("id", "bigint")]),
            ("", vec![("total", "bigint")]),
        ]));
        let ep = f
            .create_endpoint("dup", "reports/dup", "SELECT 1 AS n")
            .await;
        f.set_statements(
            ep,
            serde_json::json!([
                { "name": "total", "sql": "SELECT count(*) AS total FROM ledger" },
                { "name": "rows", "sql": "SELECT a.id, b.id FROM a JOIN b ON 1=1" },
            ]),
        )
        .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "duplicate: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("returns two columns named `id`"),
            "duplicate: the alias diagnosis must be the one that fired: {body}"
        );
        assert!(
            f.versions(ep).await.is_empty(),
            "duplicate must not publish"
        );
    }

    // 3. `output.naming` collapsing two of the second statement's branch names
    //    onto one key.
    {
        f.port.set(Describe::PerSql(vec![
            ("total_revenue", vec![("total_revenue", "bigint")]),
            ("totalRevenue", vec![("totalRevenue", "bigint")]),
            ("", vec![("id", "uuid")]),
        ]));
        let ep = f
            .create_endpoint("col", "reports/col", "SELECT 1 AS n")
            .await;
        f.patch_endpoint(
            ep,
            serde_json::json!({
                "params": [{ "name": "d", "type": "boolean", "location": "query" }],
                "output": { "shape": "objects", "naming": "camel" }
            }),
        )
        .await;
        f.set_statements(
            ep,
            serde_json::json!([
                { "name": "rows", "sql": "SELECT id FROM t" },
                { "name": "rev",
                  "sql": "SELECT {% if d %}total_revenue{% else %}totalRevenue{% endif %} FROM r" },
            ]),
        )
        .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "naming: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("both become `totalRevenue` under `output.naming: camel`"),
            "naming: the collision fold must run over the SECOND statement's \
             union too: {body}"
        );
        assert!(f.versions(ep).await.is_empty(), "naming must not publish");
    }

    // 4. One name, two types, across the second statement's branches.
    {
        f.port.set(Describe::PerSql(vec![
            ("name AS v", vec![("v", "varchar")]),
            ("n AS v", vec![("v", "bigint")]),
            ("", vec![("id", "uuid")]),
        ]));
        let ep = f.create_endpoint("ty", "reports/ty", "SELECT 1 AS n").await;
        f.patch_endpoint(
            ep,
            serde_json::json!({
                "params": [{ "name": "d", "type": "boolean", "location": "query" }]
            }),
        )
        .await;
        f.set_statements(
            ep,
            serde_json::json!([
                { "name": "rows", "sql": "SELECT id FROM t" },
                { "name": "v",
                  "sql": "SELECT {% if d %}name{% else %}n{% endif %} AS v FROM r" },
            ]),
        )
        .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "type: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("is `varchar` in one branch of this definition and `bigint`"),
            "type: the union's type comparison must run per statement: {body}"
        );
        assert!(f.versions(ep).await.is_empty(), "type must not publish");
    }
}

/// **A parameter is declared once for the endpoint and may be read by only one
/// of its statements** (spec §4.2).
///
/// `ValidationError::UnreferencedParameter` is the one rule in the parameter
/// scan that is about the CONTRACT rather than about the statement, and it is
/// therefore the one rule that cannot be applied per statement: a `sort` that
/// only the rows query reads is not an unused query parameter, it is a query
/// parameter used by one of two statements. Running the scan unchanged over
/// each statement refuses this endpoint outright, which is a loud refusal of a
/// shape §4.2 states is legal.
///
/// Both directions, because a union computed as "whatever `statements[0]`
/// reads" would pass the first half and fail the second.
#[tokio::test]
async fn a_parameter_only_one_statement_reads_still_publishes() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("FROM ledger", vec![("total", "bigint")]),
        ("", vec![("id", "uuid")]),
    ]));
    let params = serde_json::json!([
        { "name": "r", "type": "string", "location": "query", "required": true }
    ]);
    let rows = "SELECT id FROM t WHERE region = {{ r }}";
    let total = "SELECT count(*) AS total FROM ledger";

    for (first, second, why) in [
        (rows, total, "the FIRST statement reads it"),
        (total, rows, "the SECOND statement reads it"),
    ] {
        let ep = f
            .create_endpoint(
                "shared",
                &format!("reports/{}", Uuid::new_v4().simple()),
                "SELECT 1 AS n",
            )
            .await;
        f.patch_endpoint(ep, serde_json::json!({ "params": params }))
            .await;
        f.set_statements(
            ep,
            serde_json::json!([
                { "name": "a", "sql": first },
                { "name": "b", "sql": second },
            ]),
        )
        .await;
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{why}: a parameter one statement reads is declared, not unused: {body}"
        );
    }

    // The control: a parameter NO statement reads is still refused, so the rule
    // became a union rather than disappearing.
    let ep = f
        .create_endpoint("unused", "reports/unused", "SELECT 1 AS n")
        .await;
    f.patch_endpoint(ep, serde_json::json!({ "params": params }))
        .await;
    f.set_statements(
        ep,
        serde_json::json!([
            { "name": "a", "sql": "SELECT id FROM t" },
            { "name": "b", "sql": total },
        ]),
    )
    .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("declared but never referenced"),
        "an endpoint may still not offer a query parameter that changes \
         nothing in any of its statements: {body}"
    );
}

/// The envelope rule, against the STORED row rather than the request body.
///
/// Same argument as the statement names one statement over: the create and patch
/// routes refuse an unrecognised `output.envelope`, nothing in the schema does,
/// and publish is where the value stops being editable. A snapshot frozen with
/// `"Bare"` serves the WRAPPED shape — the opposite of what its author asked for
/// and what the API echoed back to them — for the life of that version, with one
/// `warn!` in a log nobody is reading.
#[tokio::test]
async fn publish_refuses_an_output_envelope_the_write_routes_would_have_refused() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("env", "reports/env", "SELECT 1 AS n")
        .await;
    f.set_output(ep, serde_json::json!({ "envelope": "Bare" }))
        .await;

    let before = f.port.call_count();
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("is not one of `wrapped` or `bare`"),
        "the message must name the rule: {body}"
    );
    assert_eq!(
        f.port.call_count(),
        before,
        "the envelope is a scan, so it must refuse before the engine"
    );
    assert!(f.versions(ep).await.is_empty(), "nothing may be published");

    // The documented value publishes, so the rule above is a rule and not a
    // publish that can no longer happen.
    f.set_output(ep, serde_json::json!({ "envelope": "bare" }))
        .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// **A described column list is frozen onto the statement it was described FOR,
/// by name.**
///
/// `publish` takes `described` from its caller and the statement text from the
/// locked row, and it used to pair them by INDEX. The identity check guards that
/// today, so this is not a live bug — it is the difference between a guarantee
/// held in another function and one held by the data. What a crossing produces
/// is the worst kind of wrong: `total`'s one-column contract frozen onto `rows`'
/// SQL, an immutable version that answers `503 schema_drift` forever on an
/// endpoint whose definition is perfectly correct.
///
/// `described` is handed over REVERSED, which is exactly what an index pairing
/// cannot tell from the right order.
#[tokio::test]
async fn publish_freezes_each_statements_own_columns() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("paired", "reports/paired", "SELECT 1 AS n")
        .await;
    f.set_statements(
        ep,
        serde_json::json!([
            { "name": "rows", "sql": "SELECT id FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS n FROM t" },
        ]),
    )
    .await;

    let store = EndpointStore::new(f.db.clone());
    let draft = store.get(f.ws_id, ep).await.expect("draft");
    let column = |name: &str| egret_dataapi_api::ColumnSpec {
        name: name.into(),
        ty: "integer".into(),
        required: true,
    };
    // Reversed, and each entry carries the name it was described for.
    let described = vec![
        egret_dataapi_api::StatementSnapshot {
            name: "total".into(),
            source: egret_dataapi_api::StatementSource::Sql("SELECT count(*) AS n FROM t".into()),
            columns: vec![column("n")],
        },
        egret_dataapi_api::StatementSnapshot {
            name: "rows".into(),
            source: egret_dataapi_api::StatementSource::Sql("SELECT id FROM t".into()),
            columns: vec![column("id")],
        },
    ];
    store
        .publish(
            f.ws_id,
            ep,
            egret_dataapi::PublishRequest {
                by: egret_core::UserId(f.admin),
                note: "",
                draft: &draft,
                described,
                // No live version yet, so there is nothing to acknowledge.
                acknowledge_breaking: None,
            },
        )
        .await
        .expect("publish");

    let snap = f.live_snapshot(ep).await;
    let frozen: Vec<(String, Vec<String>)> = snap
        .statements
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                s.columns.iter().map(|c| c.name.clone()).collect(),
            )
        })
        .collect();
    assert_eq!(
        frozen,
        vec![
            ("rows".to_string(), vec!["id".to_string()]),
            ("total".to_string(), vec!["n".to_string()]),
        ],
        "each statement must carry the columns described for IT, in the draft's order"
    );
    // And the SQL still comes from the locked row, not from `described`.
    assert_eq!(
        snap.statements[0].source.sql().expect("written SQL"),
        "SELECT id FROM t"
    );
}

/// **Publish re-checks the statement NAMES, and it is not a duplicate of the
/// 400 the write routes already give.**
///
/// The create and patch routes run `validate_statements` over the request body
/// (Task 3), so a list forged like this one cannot arrive through them. It can
/// arrive anyway: nothing constrains the names in the database — the CHECK
/// added with migration 0129 covers emptiness only — and this crate is not the
/// only writer of the table. Publish is where a name stops being editable, so
/// it is the last place the check is worth anything: a duplicate frozen into an
/// immutable version keys two result sets onto one JSON key (§4.4), and one of
/// them can never be served.
#[tokio::test]
async fn publish_refuses_a_statement_list_the_write_routes_would_have_refused() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("names", "reports/names", "SELECT 1 AS n")
        .await;
    for (statements, needle, why) in [
        (
            serde_json::json!([
                { "name": "rows", "sql": "SELECT 1 AS n" },
                { "name": "rows", "sql": "SELECT 2 AS n" },
            ]),
            "used by more than one statement",
            "two statements sharing a name",
        ),
        (
            serde_json::json!([
                { "name": "rows", "sql": "SELECT 1 AS n" },
                { "name": "Total", "sql": "SELECT 2 AS n" },
            ]),
            "is not a valid identifier",
            "a name that is not an identifier",
        ),
        // The LENGTH, which the identifier rule does not bound: every character
        // here is one that rule accepts, and the name becomes a JSON key in
        // every response of an IMMUTABLE version.
        (
            serde_json::json!([{
                "name": "r".repeat(egret_dataapi_api::MAX_STATEMENT_NAME_LEN + 1),
                "sql": "SELECT 1 AS n",
            }]),
            "and the limit is 64",
            "a name past the length limit",
        ),
        // The COUNT, which is the one of the four that is about cost rather
        // than about the envelope's keys. `call_count` is asserted unchanged
        // below for every case, and for this one that is the whole point: the
        // list is one past `MAX_STATEMENTS`, and publish must refuse it without
        // rendering a branch of any of them, let alone describing one.
        (
            serde_json::Value::Array(
                (0..=egret_dataapi_api::MAX_STATEMENTS)
                    .map(|i| serde_json::json!({ "name": format!("s{i}"), "sql": "SELECT 1 AS n" }))
                    .collect(),
            ),
            "and the limit is 16",
            "more statements than the limit",
        ),
    ] {
        f.set_statements(ep, statements).await;
        let before = f.port.call_count();
        let (status, body) = f.publish_raw(ep).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(
            body["error"].as_str().unwrap_or_default().contains(needle),
            "{why}: expected {needle:?}, got {body}"
        );
        assert_eq!(
            f.port.call_count(),
            before,
            "{why}: a name is a scan, so it must refuse before the engine"
        );
        assert!(f.versions(ep).await.is_empty(), "{why} must not publish");
    }
}

// ---------- Publish-time contract diff and acknowledgement (package 6) ----------
//
// Every fixture below publishes a FIRST version and then publishes over it. That
// is not incidental setup: a publish with no live version exercises the
// first-publish path, where there is nothing to break and no acknowledgement can
// ever be required, so a suite built on one would pass with the whole gate
// deleted. The only test here that has no live version is the one whose subject
// IS that path, and it says so in its name.

impl Fixture {
    /// `POST …/publish` with a body of the caller's choosing, so the
    /// acknowledgement field can be present, absent or wrong.
    async fn publish_with(
        &self,
        ep: Uuid,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.send(
            &self.manager,
            "POST",
            &format!("{}/{ep}/publish", self.collection()),
            Some(body),
        )
        .await
    }

    async fn validate_ep(&self, ep: Uuid) -> serde_json::Value {
        let (status, body) = self
            .send(
                &self.manager,
                "POST",
                &format!("{}/{ep}/validate", self.collection()),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "validate failed: {body}");
        body
    }

    /// A live v1 returning `id` and `n`, and then a draft that returns only
    /// `id` — the removed column, ready to publish.
    async fn live_then_drops_a_column(&self, tag: &str) -> Uuid {
        self.port
            .set(Describe::Columns(vec![("id", "uuid"), ("n", "bigint")]));
        let ep = self.create_endpoint(tag, tag, "SELECT id, n FROM t").await;
        self.publish(ep).await;
        self.port.set(Describe::Columns(vec![("id", "uuid")]));
        self.set_statements(
            ep,
            serde_json::json!([{ "name": "main", "sql": "SELECT id FROM t" }]),
        )
        .await;
        ep
    }
}

/// **The property this package exists for.** Before it, this publish returned
/// 200 and every deployed consumer of `n` found out at its next call.
#[tokio::test]
async fn publishing_a_removed_column_over_a_live_version_is_refused() {
    let f = skip_without_db!();
    let ep = f.live_then_drops_a_column("pkg6drop").await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("column `n`"),
        "the refusal must name the change the author has to act on: {msg}"
    );
    // A publish-time refusal costs nothing: the transaction rolls back having
    // written no version and moved no pointer.
    assert_eq!(
        f.versions(ep).await.len(),
        1,
        "a refused publish must leave the history alone"
    );
    assert_eq!(f.live_snapshot(ep).await.statements[0].columns.len(), 2);
}

/// The acknowledgement `POST /validate` issues is the one `POST /publish`
/// accepts, and the break is RECORDED against the version and its author.
#[tokio::test]
async fn the_acknowledgement_from_validate_publishes_the_break_and_records_it() {
    let f = skip_without_db!();
    let ep = f.live_then_drops_a_column("pkg6ack").await;

    let v = f.validate_ep(ep).await;
    assert_eq!(
        v["against_live"]["breaking"][0]["kind"], "column_removed",
        "{v}"
    );
    let token = v["acknowledge_breaking"]
        .as_str()
        .expect("validate must issue an acknowledgement when something breaks")
        .to_string();

    let (status, body) = f
        .publish_with(
            ep,
            serde_json::json!({ "note": "drop n", "acknowledge_breaking": token }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["breaking_changes"][0]["kind"], "column_removed");

    let versions = f.versions(ep).await;
    let v2 = versions
        .iter()
        .find(|v| v["version"] == 2)
        .expect("v2 exists");
    assert_eq!(v2["breaking_changes"][0]["kind"], "column_removed");
    assert!(
        v2["breaking_changes"][0]["subject"]
            .as_str()
            .unwrap_or_default()
            .contains("`n`"),
        "{v2}"
    );
    // Who accepted it is the row's own `created_by`, which is what makes this an
    // audit trail rather than a note.
    assert_eq!(
        v2["created_by"],
        serde_json::json!(f.manager.user.to_string())
    );
}

/// An acknowledgement is a digest of the changes it acknowledges, so one
/// obtained for a break is not a `force: true` that covers the next one.
#[tokio::test]
async fn an_acknowledgement_does_not_cover_a_break_it_was_not_issued_for() {
    let f = skip_without_db!();
    let ep = f.live_then_drops_a_column("pkg6stale").await;

    let token = f.validate_ep(ep).await["acknowledge_breaking"]
        .as_str()
        .expect("a token")
        .to_string();

    // The author keeps editing after taking the token: now the row limit falls
    // too, so the set of breaking changes is not the set that was acknowledged.
    f.patch_endpoint(ep, serde_json::json!({ "limits": { "max_rows": 10 } }))
        .await;

    let (status, body) = f
        .publish_with(ep, serde_json::json!({ "acknowledge_breaking": token }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("does not match"),
        "{body}"
    );
    assert_eq!(f.versions(ep).await.len(), 1);
}

/// The headline case: the edit an author reads as "I added something" and a
/// consumer receives as a different response body.
#[tokio::test]
async fn adding_a_second_statement_over_a_live_version_is_refused() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6two", "pkg6two", "SELECT id FROM t")
        .await;
    f.publish(ep).await;

    f.port.set(Describe::PerSql(vec![
        ("count(*)", vec![("n", "bigint")]),
        ("", vec![("id", "uuid")]),
    ]));
    f.set_statements(
        ep,
        serde_json::json!([
            { "name": "main", "sql": "SELECT id FROM t" },
            { "name": "total", "sql": "SELECT count(*) AS n FROM t" }
        ]),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("keyed by statement name"),
        "the refusal must say what the consumer's body becomes: {msg}"
    );
    assert_eq!(f.versions(ep).await.len(), 1);
}

/// The other half of the rule, and the one that keeps the warning worth
/// reading: editing the SQL changes what an endpoint returns and breaks no
/// contract, so it publishes with nothing to acknowledge.
#[tokio::test]
async fn editing_only_the_sql_publishes_over_a_live_version_with_no_acknowledgement() {
    let f = skip_without_db!();
    f.port
        .set(Describe::Columns(vec![("id", "uuid"), ("n", "bigint")]));
    let ep = f
        .create_endpoint("pkg6sql", "pkg6sql", "SELECT id, n FROM t")
        .await;
    f.publish(ep).await;

    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT id, n FROM t WHERE n > 0" }]),
    )
    .await;

    let v = f.validate_ep(ep).await;
    assert_eq!(v["against_live"]["breaking"], serde_json::json!([]), "{v}");
    assert!(v["acknowledge_breaking"].is_null(), "{v}");
    // Reported, though — "we compared and found six harmless things" is a
    // different message from "we found nothing".
    assert_eq!(
        v["against_live"]["safe"][0]["kind"],
        "statement_definition_changed"
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Compared, and broke nothing. Distinct from the `null` a first publish
    // records, which means nothing was compared at all.
    assert_eq!(body["breaking_changes"], serde_json::json!([]));
}

/// The first publish is the one case with nothing to compare, and the record
/// says so rather than claiming a comparison found nothing.
#[tokio::test]
async fn a_first_publish_compares_nothing_and_records_that_it_did_not() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6first", "pkg6first", "SELECT id FROM t")
        .await;

    let v = f.validate_ep(ep).await;
    assert!(
        v["against_live"].is_null(),
        "there is no live version to compare against: {v}"
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["breaking_changes"].is_null(),
        "a first publish compared nothing: {body}"
    );
    let versions = f.versions(ep).await;
    assert!(versions[0]["breaking_changes"].is_null(), "{versions:?}");
}

/// **The record a `rollback` used to make false.**
///
/// v1 returns `id, n`. A rollback to v0 — `id` alone — is not refused, and
/// should not be: it is usually someone ending an outage. But it moves every
/// consumer of v1 off a contract they were coding against, and it writes no row
/// in `endpoint_versions`. Publishing the id-only draft afterwards then compares
/// against v0, finds nothing, and stores `[]`, which migration `0131` DEFINES as
/// "compared and broke nothing".
///
/// Two things make that true again, and neither is a gate:
///
/// * the version row names the baseline it is a claim about, so `[]` beside a
///   baseline that is not the newest version is visibly a rolled-back one;
/// * the rollback itself classifies what it moved consumers off and returns it,
///   which is the record that did not exist anywhere before.
#[tokio::test]
async fn a_publish_after_a_rollback_names_the_baseline_its_empty_diff_is_about() {
    let f = skip_without_db!();
    // v1: `id` alone.
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6rb", "pkg6rb", "SELECT id FROM t")
        .await;
    f.publish(ep).await;

    // v2: `id, n`, the contract consumers then code against. Adding a column is
    // safe, so this needs no acknowledgement.
    f.port
        .set(Describe::Columns(vec![("id", "uuid"), ("n", "bigint")]));
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT id, n FROM t" }]),
    )
    .await;
    f.publish(ep).await;

    // The rollback to v1 removes `n` from every response, and says so.
    let (status, rolled) = f
        .rollback_raw(ep, serde_json::json!({ "version": 1 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{rolled}");
    assert_eq!(
        rolled["rollback_breaks"][0]["kind"], "column_removed",
        "a rollback must say what it moves consumers off, even though it \
         refuses nothing: {rolled}"
    );
    assert!(
        rolled["rollback_breaks"][0]["subject"]
            .as_str()
            .unwrap_or_default()
            .contains("`n`"),
        "{rolled}"
    );
    // It is not an acknowledgement and must never be recorded as one.
    assert!(rolled["breaking_changes"].is_null(), "{rolled}");

    // Now publish the id-only draft. Against v1 — the live version — it breaks
    // nothing, and that is the truth. The row has to say WHICH truth.
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT id FROM t" }]),
    )
    .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["breaking_changes"], serde_json::json!([]), "{body}");

    let versions = f.versions(ep).await;
    let v3 = versions
        .iter()
        .find(|v| v["version"] == 3)
        .expect("v3 exists");
    assert_eq!(
        v3["compared_against_version"],
        serde_json::json!(1),
        "the empty diff is a claim about v1, not about v2 — and a reader who \
         cannot see that reads it as `nobody was broken here`: {v3}"
    );
    // The version it does NOT speak for is still in the history, one number
    // higher than the baseline, which is what makes the rollback visible.
    let v2 = versions
        .iter()
        .find(|v| v["version"] == 2)
        .expect("v2 exists");
    assert_eq!(v2["compared_against_version"], serde_json::json!(1), "{v2}");
}

/// The ordinary case, so the field above is not just "whatever the last publish
/// happened to point at": with no rollback the baseline IS the version before.
#[tokio::test]
async fn a_publish_over_a_live_version_names_that_version_as_its_baseline() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6base", "pkg6base", "SELECT id FROM t")
        .await;
    f.publish(ep).await;
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT id FROM t WHERE x" }]),
    )
    .await;
    f.publish(ep).await;

    let versions = f.versions(ep).await;
    let v2 = versions.iter().find(|v| v["version"] == 2).expect("v2");
    assert_eq!(v2["compared_against_version"], serde_json::json!(1), "{v2}");
    // And a FIRST publish compared against nothing, so it names nothing —
    // exactly where `breaking_changes` is null too. The two absences are the
    // same absence and must not disagree.
    let v1 = versions.iter().find(|v| v["version"] == 1).expect("v1");
    assert!(v1["compared_against_version"].is_null(), "{v1}");
    assert!(v1["breaking_changes"].is_null(), "{v1}");
}

/// A mid-list insertion under `shape: arrays`, end to end.
///
/// `SELECT id, n` becoming `SELECT id, region, n` reorders nothing — the
/// survivors are still `id` then `n` — so a check that compares the survivors as
/// a sequence sees an empty diff and publishes with a 200. `serving::router`
/// requires the engine's positional order to equal the frozen list exactly and
/// then clones the rows verbatim, so every consumer reading `data[i][1]` as the
/// bigint `n` starts reading the varchar `region`.
#[tokio::test]
async fn publishing_a_mid_list_column_insertion_under_arrays_is_refused() {
    let f = skip_without_db!();
    f.port
        .set(Describe::Columns(vec![("id", "uuid"), ("n", "bigint")]));
    let ep = f
        .create_endpoint("pkg6ins", "pkg6ins", "SELECT id, n FROM t")
        .await;
    f.patch_endpoint(ep, serde_json::json!({ "output": { "shape": "arrays" } }))
        .await;
    f.publish(ep).await;

    f.port.set(Describe::Columns(vec![
        ("id", "uuid"),
        ("region", "varchar"),
        ("n", "bigint"),
    ]));
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT id, region, n FROM t" }]),
    )
    .await;

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an insertion moves every index after it: {body}"
    );
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("index 1 to 2"),
        "the refusal must name the index a consumer already reads: {msg}"
    );
    assert_eq!(
        f.versions(ep).await.len(),
        1,
        "a refused publish leaves the history alone"
    );
}

/// Sending an acknowledgement that corresponds to nothing is refused rather
/// than ignored. Ignoring it is how a client learns to send one always, which
/// is the `force: true` the token shape exists to make unrepresentable.
#[tokio::test]
async fn an_acknowledgement_sent_when_nothing_breaks_is_refused() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6noop", "pkg6noop", "SELECT id FROM t")
        .await;
    f.publish(ep).await;

    let (status, body) = f
        .publish_with(
            ep,
            serde_json::json!({ "acknowledge_breaking": "0".repeat(64) }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("breaks nothing"),
        "{body}"
    );
}

/// The same refusal on the FIRST publish, where the comparison does not happen
/// at all.
///
/// This is the arm that was missed first: `publish` skipped the check entirely
/// when there was no live version, so a token sent there was silently ignored —
/// and a client whose token is silently ignored is a client that learns to send
/// one on every call, which is exactly the `force: true` the digest exists to
/// prevent.
#[tokio::test]
async fn an_acknowledgement_sent_on_a_first_publish_is_refused_too() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6firstack", "pkg6firstack", "SELECT id FROM t")
        .await;
    assert!(
        f.versions(ep).await.is_empty(),
        "this test is about the no-live-version path"
    );

    let (status, body) = f
        .publish_with(
            ep,
            serde_json::json!({ "acknowledge_breaking": "0".repeat(64) }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("breaks nothing"),
        "{body}"
    );
    assert!(
        f.versions(ep).await.is_empty(),
        "a refused publish must write nothing"
    );
}

/// A live snapshot that cannot be read stops the publish; it does not skip the
/// comparison.
///
/// Package 6 gave `publish` a dependency it did not have: it must now be able to
/// DESERIALIZE the live version. The tempting handling is to shrug and publish
/// anyway — and that would mean a corrupt row silently disables the
/// breaking-change gate for that endpoint, forever, with a 200 each time. So the
/// direction is closed, and it is the same answer `EndpointStore::versions` has
/// given for an unreadable snapshot since P3.
///
/// Only a direct write can produce one: `publish` is the sole writer of this
/// column. That is why the fixture below goes past the routes.
#[tokio::test]
async fn publish_refuses_when_the_live_snapshot_cannot_be_read() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("pkg6corrupt", "pkg6corrupt", "SELECT id FROM t")
        .await;
    f.publish(ep).await;

    let touched = sqlx::query(
        "UPDATE dataapi.endpoint_versions v SET snapshot = '{}'::jsonb \
           FROM dataapi.endpoints e WHERE e.live_version_id = v.id AND e.id = $1",
    )
    .bind(ep)
    .execute(&f.db.pool)
    .await
    .expect("corrupt the live snapshot")
    .rows_affected();
    assert_eq!(
        touched, 1,
        "the live snapshot must actually have been replaced"
    );

    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unreadable live version must stop the publish, not skip the diff: {body}"
    );
    // Counted in SQL, not through the versions route: that route deserializes
    // the same snapshot and answers the same 500, so it cannot be the witness
    // for whether a row was written.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoint_versions WHERE endpoint_id = $1")
            .bind(ep)
            .fetch_one(&f.db.pool)
            .await
            .expect("count versions");
    assert_eq!(rows, 1, "a refused publish must write nothing");
}

/// The diff is against the version consumers are RECEIVING, not against the
/// newest row — the two differ the moment anyone rolls back, and `publish`
/// used to read `MAX(version)` and nothing else.
#[tokio::test]
async fn the_diff_is_against_the_live_version_not_the_newest_one() {
    let f = skip_without_db!();
    // v1 returns `id` and `n`.
    f.port
        .set(Describe::Columns(vec![("id", "uuid"), ("n", "bigint")]));
    let ep = f
        .create_endpoint("pkg6roll", "pkg6roll", "SELECT id, n FROM t")
        .await;
    f.publish(ep).await;

    // v2 drops `n`, acknowledged.
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    f.set_statements(
        ep,
        serde_json::json!([{ "name": "main", "sql": "SELECT id FROM t" }]),
    )
    .await;
    let token = f.validate_ep(ep).await["acknowledge_breaking"]
        .as_str()
        .expect("a token")
        .to_string();
    let (status, body) = f
        .publish_with(ep, serde_json::json!({ "acknowledge_breaking": token }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Roll back to v1, which returns both columns again. `MAX(version)` is now
    // v2 and the LIVE contract is v1.
    let (status, body) = f
        .send(
            &f.manager,
            "POST",
            &format!("{}/{ep}/rollback", f.collection()),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The draft still drops `n`. Against v2 — the newest row — that is no
    // change at all; against v1, which is what consumers receive, it is the
    // removal. The refusal is what says which one was compared.
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the diff must be against the LIVE version, not the newest: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("column `n`"),
        "{body}"
    );
}
