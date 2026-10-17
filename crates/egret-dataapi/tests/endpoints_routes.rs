//! Draft CRUD for Data API endpoints, driven from the caller's side against the
//! REAL Cedar PDP — a mocked decision would pin the consumer, not the server.
//!
//! Four properties this file exists to hold:
//!
//! 1. `data_api_endpoint:read` cannot author. egret's permissions are FLAT, so
//!    `:manage` implies no `:read` and `:read` implies no `:manage`; nothing but
//!    an explicit `require` stands between a reader and a write.
//! 2. An endpoint in another workspace is 404, never 403 — absence and denial
//!    must be indistinguishable from outside.
//! 3. `database_id` and `folder_id` arrive in the REQUEST BODY, so a draft could
//!    otherwise name a connection or a folder belonging to some other workspace.
//!    Publish (Task 4) resolves the schema through `describe_as`, which is
//!    workspace-scoped, so a foreign `database_id` would fail there — but by
//!    then the reference has been accepted, stored, and shown back to the author
//!    as valid. Nothing downstream would ever refuse a foreign `folder_id` at
//!    all. Both are refused here, with the same 404, and `folder_id` is checked
//!    for `entity_type` too — `analytics.folders` is polymorphic, so a local
//!    CHART folder is a valid uuid that would swallow the endpoint out of its
//!    own list.
//! 4. A stored `path` is CANONICAL. Every alias of one URL — dot segments,
//!    interior `//`, percent-encoding, a query string or fragment — is a 400,
//!    because each would otherwise be a separate row behind the same route, and
//!    a percent-encoded one could never be matched at all: P4 serves
//!    `{*path}` and axum percent-DECODES a wildcard capture.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like
//! `routes_authz.rs` — so check the duration, not just the `ok`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_dataapi::{management_router, ClientStore, DataApiState, EndpointStore};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    db: Db,
    /// The workspace under test, by SLUG — `WsContext` resolves `{workspace}`
    /// as a slug, so this is what goes in a URL.
    ws: String,
    ws_id: Uuid,
    /// A human admin, used as `created_by`.
    admin: Uuid,
    /// A `query.databases` row owned by `ws_id`. `endpoints.database_id` has a
    /// real FK (RESTRICT), so every draft needs a row to point at.
    database_id: Uuid,
}

/// A principal: a router + session token whose PDP already sees its grants.
/// Both are rebuilt per principal because `CedarPdp` snapshots policies at build
/// time and a session caches its permission set.
struct Caller {
    app: axum::Router,
    tok: String,
}

/// A `QueryExecPort` that must never be consulted. Draft CRUD talks to the
/// engine at no point — validation and `describe` belong to the publish
/// lifecycle — so a create or a PATCH that reached a coordinator would be a bug
/// this makes loud instead of slow.
struct UnreachablePort;
#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for UnreachablePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
    ) -> Result<Vec<egret_core::ColumnMeta>, egret_core::Error> {
        unreachable!("draft CRUD must never reach the query engine")
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, egret_core::Error> {
        unreachable!("draft CRUD must never reach the query engine")
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> DataApiState {
    DataApiState {
        db: db.clone(),
        clients: ClientStore::new(db.clone()),
        endpoints: EndpointStore::new(db.clone()),
        query: Arc::new(UnreachablePort),
        users: egret_auth::users::UserRepo::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
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
        // This file is about draft CRUD, not Activity — a fixed retention is
        // enough to satisfy the field.
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
         VALUES ($1, 'Data API Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn insert_database(db: &Db, ws: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

async fn fixture() -> Option<Fixture> {
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    let admin = insert_user(&db, "dataapi_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "dataapi").await;
    let database_id = insert_database(&db, ws_id, admin).await;
    Some(Fixture {
        db,
        ws,
        ws_id,
        admin,
        database_id,
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

impl Fixture {
    /// The realistic operator. `ws_admin` is seeded with BOTH `:read` and
    /// `:manage` (migration 0122) because permissions are flat and `:manage`
    /// implies nothing — a manage-only principal is not a role anyone holds. The
    /// asymmetry that matters (read alone cannot author) is pinned separately.
    const MANAGER: &'static [&'static str] =
        &["data_api_endpoint:read", "data_api_endpoint:manage"];

    /// A draft written straight to the table. Tests about WHO MAY READ an
    /// endpoint should not depend on the create route also being correct.
    async fn create_endpoint(&self, name: &str, path: &str, sql: &str) -> Uuid {
        self.create_endpoint_in(self.ws_id, self.database_id, name, path, sql)
            .await
    }

    async fn create_endpoint_in(
        &self,
        ws: Uuid,
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
        .bind(ws)
        .bind(name)
        .bind(path)
        .bind(database_id)
        .bind(sql)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft")
    }

    async fn create_workspace(&self, tag: &str) -> (Uuid, String) {
        insert_workspace(&self.db, self.admin, tag).await
    }

    /// A real `analytics.folders` row. `entity_type` is a parameter because the
    /// wrong type is one of the two ways a folder id can be illegitimate here,
    /// and the guard has to refuse both.
    async fn create_folder_in(&self, ws: Uuid, entity_type: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO analytics.folders (workspace_id, entity_type, name, created_by) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(ws)
        .bind(entity_type)
        .bind(format!("f-{}", Uuid::new_v4().simple()))
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert folder")
    }

    async fn create_database_in(&self, ws: Uuid) -> Uuid {
        insert_database(&self.db, ws, self.admin).await
    }

    /// A brand-new, non-admin user holding exactly `perms` in `self.ws`.
    async fn principal_with(&self, perms: &[&str]) -> Caller {
        self.principal_with_in(perms, self.ws_id).await
    }

    /// The same, in an arbitrary workspace — the cross-workspace tests need a
    /// principal who is a legitimate MANAGER somewhere else.
    async fn principal_with_in(&self, perms: &[&str], ws: Uuid) -> Caller {
        let user = insert_user(&self.db, "dataapi_caller").await;
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name, description) \
             VALUES ('workspace', $1, 'test') RETURNING id",
        )
        .bind(format!("test_dataapi_ep_{}", Uuid::new_v4().simple()))
        .fetch_one(&self.db.pool)
        .await
        .expect("insert role");
        for p in perms {
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(role)
                .bind(p)
                .execute(&self.db.pool)
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
        .execute(&self.db.pool)
        .await
        .expect("grant role");

        let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
            egret_auth::pdp::CedarPdp::build(self.db.clone(), &egret_events::EventBus::new())
                .await
                .expect("build CedarPdp"),
        );
        let st = build_state(self.db.clone(), pdp);
        let tok = st
            .sessions
            .create(egret_core::ids::UserId(user), "test", "127.0.0.1")
            .await
            .expect("create session")
            .token;
        // Nested at /api exactly as `egret-server` mounts it, so the paths these
        // tests drive are the paths production serves.
        let app = axum::Router::new()
            .nest("/api", management_router())
            .with_state(st);
        Caller { app, tok }
    }

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

    async fn post_as(
        &self,
        c: &Caller,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.send(c, "POST", path, Some(body)).await
    }

    async fn patch_as(
        &self,
        c: &Caller,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.send(c, "PATCH", path, Some(body)).await
    }

    async fn get_as(&self, c: &Caller, path: &str) -> (StatusCode, serde_json::Value) {
        self.send(c, "GET", path, None).await
    }

    fn collection(&self) -> String {
        format!("/api/ws/{}/data-api/endpoints", self.ws)
    }
}

// ---------- Authorization ----------

#[tokio::test]
async fn read_permission_cannot_create_an_endpoint() {
    let f = skip_without_db!();
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;

    let (status, body) = f
        .post_as(
            &reader,
            &f.collection(),
            serde_json::json!({
                "name": "rev", "path": "reports/revenue",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "read must not imply create");

    // And nothing was written — a 403 returned AFTER the insert would look the
    // same from outside.
    let made: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dataapi.endpoints WHERE workspace_id = $1 AND name = 'rev'",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("count endpoints");
    assert_eq!(made, 0, "body: {body}");
}

/// The read side must still work for a reader — otherwise the test above would
/// pass just as well against a router that denied a reader everything.
#[tokio::test]
async fn read_permission_can_still_list_and_get() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;

    let (status, body) = f.get_as(&reader, &f.collection()).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body.as_array().expect("array").len(), 1);

    let (status, body) = f.get_as(&reader, &format!("{}/{id}", f.collection())).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["id"].as_str().expect("id"), id.to_string());
}

/// Permissions are FLAT: `:manage` grants nothing on the read routes. Pinned
/// because it is the trap P2 hit — a test that grants only `:manage` and expects
/// a list to work gets a 403 and looks like a broken route.
#[tokio::test]
async fn manage_permission_alone_cannot_list() {
    let f = skip_without_db!();
    f.create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let manager_only = f.principal_with(&["data_api_endpoint:manage"]).await;

    let (status, body) = f.get_as(&manager_only, &f.collection()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "permissions are flat — :manage must not imply :read. body: {body}"
    );
}

#[tokio::test]
async fn listing_endpoints_requires_read() {
    let f = skip_without_db!();
    f.create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let nobody = f.principal_with(&[]).await;

    let (status, body) = f.get_as(&nobody, &f.collection()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn getting_an_endpoint_requires_read() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let nobody = f.principal_with(&[]).await;

    let (status, body) = f.get_as(&nobody, &format!("{}/{id}", f.collection())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn updating_an_endpoint_requires_manage() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;

    let (status, body) = f
        .patch_as(
            &reader,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 2" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");

    let sql: String =
        sqlx::query_scalar("SELECT statements->0->>'sql' FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("sql");
    assert_eq!(sql, "SELECT 1", "the refused edit must not have edited");
}

/// **A trial gates on `:manage`, not `:read`** — it runs a query as a service
/// account, which is an act on somebody else's behalf rather than a look at the
/// draft. `:read` is the permission a reviewer or a consumer-team member holds.
///
/// The second half is what makes the first non-vacuous: the SAME request from a
/// manager gets past the gate and fails on the ids instead (404), so the 403
/// above is the permission check and not the endpoint id or the body.
#[tokio::test]
async fn running_a_trial_requires_manage() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;
    let body = serde_json::json!({ "service_account_id": Uuid::new_v4() });

    let (status, out) = f
        .post_as(
            &reader,
            &format!("{}/{id}/try", f.collection()),
            body.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {out}");

    let manager = f.principal_with(Fixture::MANAGER).await;
    let (status, out) = f
        .post_as(&manager, &format!("{}/{id}/try", f.collection()), body)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a manager reaches the containment check on the account: {out}"
    );
}

#[tokio::test]
async fn deleting_an_endpoint_requires_manage() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;

    let (status, body) = f
        .send(&reader, "DELETE", &format!("{}/{id}", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoints WHERE id = $1")
        .bind(id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count");
    assert_eq!(left, 1, "the refused delete must not have deleted anything");
}

// ---------- Workspace containment ----------

#[tokio::test]
async fn an_endpoint_cannot_be_read_across_workspaces() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let (other_id, other_ws) = f.create_workspace("other").await;
    let mgr = f.principal_with_in(Fixture::MANAGER, other_id).await;

    let (status, body) = f
        .get_as(&mgr, &format!("/api/ws/{other_ws}/data-api/endpoints/{ep}"))
        .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "not 403 — absence and denial must look the same. body: {body}"
    );
}

#[tokio::test]
async fn an_endpoint_cannot_be_updated_across_workspaces() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let (other_id, other_ws) = f.create_workspace("other").await;
    let mgr = f.principal_with_in(Fixture::MANAGER, other_id).await;

    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("/api/ws/{other_ws}/data-api/endpoints/{ep}"),
            serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 'pwned'" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    let sql: String =
        sqlx::query_scalar("SELECT statements->0->>'sql' FROM dataapi.endpoints WHERE id = $1")
            .bind(ep)
            .fetch_one(&f.db.pool)
            .await
            .expect("sql");
    assert_eq!(sql, "SELECT 1", "the outsider must not have edited it");
}

#[tokio::test]
async fn an_endpoint_cannot_be_deleted_across_workspaces() {
    let f = skip_without_db!();
    let ep = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let (other_id, other_ws) = f.create_workspace("other").await;
    let mgr = f.principal_with_in(Fixture::MANAGER, other_id).await;

    let (status, body) = f
        .send(
            &mgr,
            "DELETE",
            &format!("/api/ws/{other_ws}/data-api/endpoints/{ep}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoints WHERE id = $1")
        .bind(ep)
        .fetch_one(&f.db.pool)
        .await
        .expect("count");
    assert_eq!(left, 1, "the outsider must not have deleted it");
}

/// The list is scoped to the caller's workspace, not merely filtered by the path
/// segment: an endpoint belonging to another workspace must not appear.
#[tokio::test]
async fn the_list_shows_only_this_workspaces_endpoints() {
    let f = skip_without_db!();
    f.create_endpoint("mine", "/reports/revenue", "SELECT 1")
        .await;
    let (other_id, _) = f.create_workspace("other").await;
    let other_db = f.create_database_in(other_id).await;
    f.create_endpoint_in(other_id, other_db, "theirs", "/reports/theirs", "SELECT 2")
        .await;

    let reader = f.principal_with(&["data_api_endpoint:read"]).await;
    let (status, body) = f.get_as(&reader, &f.collection()).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let names: Vec<&str> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|e| e["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["mine"], "body: {body}");
}

// ---------- database_id containment ----------
//
// `database_id` comes from the REQUEST BODY. Nothing else in the draft path
// resolves it, so without an explicit check a draft can name a connection owned
// by another workspace. `describe_as` (Task 1) is workspace-scoped, so publish
// would eventually refuse it — but the reference would already be stored and
// handed back to the author as if it were valid, and the endpoint's own DTO
// would advertise another workspace's connection id. 404, not 403: the same rule
// as everywhere else in this feature.

#[tokio::test]
async fn create_refuses_a_database_from_another_workspace() {
    let f = skip_without_db!();
    let (other_id, _) = f.create_workspace("other").await;
    let foreign_db = f.create_database_in(other_id).await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "poach", "path": "/reports/poach",
                "database_id": foreign_db, "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a connection this workspace was never granted must be invisible, not merely forbidden. body: {body}"
    );

    let made: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dataapi.endpoints WHERE workspace_id = $1 AND name = 'poach'",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("count");
    assert_eq!(made, 0, "the refused create must not have created anything");
}

#[tokio::test]
async fn update_refuses_a_database_from_another_workspace() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let (other_id, _) = f.create_workspace("other").await;
    let foreign_db = f.create_database_in(other_id).await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "database_id": foreign_db }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    let still: Uuid = sqlx::query_scalar("SELECT database_id FROM dataapi.endpoints WHERE id = $1")
        .bind(id)
        .fetch_one(&f.db.pool)
        .await
        .expect("database_id");
    assert_eq!(
        still, f.database_id,
        "the refused update must not have repointed the connection"
    );
}

/// The check must not be "any row in query.databases": a database that does not
/// exist at all is refused the same way, and never reaches the FK as a 500.
#[tokio::test]
async fn create_refuses_a_database_that_does_not_exist() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "ghost", "path": "/reports/ghost",
                "database_id": Uuid::new_v4(), "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

// ---------- folder_id containment ----------
//
// `folder_id` is `database_id`'s twin: an id supplied in the BODY for an object
// the URL does not name. Unchecked it is worse, because nothing downstream would
// ever refuse it — an endpoint filed into another workspace's folder simply
// stays there. `analytics.folders` is polymorphic with no constraint on
// `entity_type` (0021, restated in 0113), so a workspace-local CHART folder is a
// valid uuid that would swallow the endpoint out of its own list.

#[tokio::test]
async fn create_refuses_a_folder_from_another_workspace() {
    let f = skip_without_db!();
    let (other_id, _) = f.create_workspace("other").await;
    let foreign_folder = f.create_folder_in(other_id, "data_api_endpoint").await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "filed-away", "path": "/reports/filed",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }],
                "folder_id": foreign_folder
            }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another workspace's folder must be invisible, not merely forbidden. body: {body}"
    );

    let made: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dataapi.endpoints WHERE workspace_id = $1 AND name = 'filed-away'",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("count");
    assert_eq!(made, 0, "the refused create must not have created anything");
}

/// The wrong `entity_type` is the second half of the guard, and the half a
/// workspace check alone would miss: this folder IS the caller's, and filing an
/// endpoint into it would succeed, after which the endpoint is absent from the
/// endpoint folder list and present in the chart one.
#[tokio::test]
async fn create_refuses_a_folder_of_the_wrong_entity_type() {
    let f = skip_without_db!();
    let chart_folder = f.create_folder_in(f.ws_id, "chart").await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "miscategorised", "path": "/reports/misc",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }],
                "folder_id": chart_folder
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

/// A folder id that exists nowhere must be the SAME 404, not a 500 quoting
/// `endpoints_folder_id_fkey` — otherwise 201-versus-500 is a free existence
/// oracle for folder ids in every other workspace.
#[tokio::test]
async fn create_refuses_a_folder_that_does_not_exist() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "ghost-folder", "path": "/reports/ghostf",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }],
                "folder_id": Uuid::new_v4()
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

#[tokio::test]
async fn update_refuses_a_folder_from_another_workspace() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let (other_id, _) = f.create_workspace("other").await;
    let foreign_folder = f.create_folder_in(other_id, "data_api_endpoint").await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "folder_id": foreign_folder }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    let still: Option<Uuid> =
        sqlx::query_scalar("SELECT folder_id FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("folder_id");
    assert_eq!(still, None, "the refused update must not have filed it");
}

/// The legitimate case must work, or every test above would pass just as well
/// against a route that refused all folders.
#[tokio::test]
async fn a_folder_of_this_workspace_and_type_is_accepted_and_clearable() {
    let f = skip_without_db!();
    let folder = f.create_folder_in(f.ws_id, "data_api_endpoint").await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, created) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "filed", "path": "/reports/filed",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }],
                "folder_id": folder
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    assert_eq!(
        created["folder_id"].as_str().expect("folder_id"),
        folder.to_string()
    );
    let id: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");

    // An explicit null clears it — the whole reason `folder_id` is a double
    // option. A plain Option would read this as "leave it alone".
    let (status, patched) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "folder_id": null }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {patched}");
    assert!(
        patched["folder_id"].is_null(),
        "explicit null must clear it: {patched}"
    );
}

// ---------- Path validation ----------
//
// Every family below is a different spelling of one URL. Stored unvalidated they
// are separate rows to the UNIQUE constraint and one route to a client, so the
// constraint whose entire job is to stop that would admit them; percent-encoding
// is worse still, because P4 matches `{*path}` and axum percent-DECODES a
// wildcard capture, making the stored form permanently unmatchable.

#[tokio::test]
async fn every_path_alias_family_is_a_bad_request() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    for (path, why) in [
        ("/reports/../revenue", "dot-dot segment"),
        ("/reports/./revenue", "dot segment"),
        ("/reports//revenue", "interior double slash"),
        ("/r%65ports/revenue", "percent-encoded letter"),
        ("/reports%2frevenue", "percent-encoded slash, lower case"),
        ("/reports%2Frevenue", "percent-encoded slash, upper case"),
        ("/reports/revenue?region=us", "query string"),
        ("/reports/revenue#top", "fragment"),
        ("/reports/my revenue", "interior space"),
        ("/reports/rev\tenue", "tab"),
        ("/reports/{id}", "brace template"),
        ("/reports/:id", "colon template"),
    ] {
        let (status, body) = f
            .post_as(
                &mgr,
                &f.collection(),
                serde_json::json!({
                    "name": why, "path": path,
                    "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
                }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{why} ({path:?}) must be refused, not stored. body: {body}"
        );
    }

    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoints WHERE workspace_id = $1")
            .bind(f.ws_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("count");
    assert_eq!(stored, 0, "not one of them may have been written");
}

/// PATCH is the other door into the `path` column and must be validated the same
/// way — otherwise an endpoint created with a clean path could be moved to an
/// unservable one.
#[tokio::test]
async fn a_patch_cannot_move_an_endpoint_to_an_alias_path() {
    let f = skip_without_db!();
    let id = f
        .create_endpoint("rev", "/reports/revenue", "SELECT 1")
        .await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    for path in ["/reports/%2e%2e/x", "/reports//x", "/reports/x?y=1"] {
        let (status, body) = f
            .patch_as(
                &mgr,
                &format!("{}/{id}", f.collection()),
                serde_json::json!({ "path": path }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path:?} body: {body}");
    }

    let unchanged: String = sqlx::query_scalar("SELECT path FROM dataapi.endpoints WHERE id = $1")
        .bind(id)
        .fetch_one(&f.db.pool)
        .await
        .expect("path");
    assert_eq!(unchanged, "/reports/revenue");
}

/// The 400 has to name the rule, or an author staring at a perfectly ordinary
/// looking URL has nothing to act on.
#[tokio::test]
async fn a_refused_path_says_which_rule_it_broke() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "enc", "path": "/reports/rev%20enue",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("percent"),
        "the message must name percent-encoding: {body}"
    );
}

// ---------- Uniqueness ----------

#[tokio::test]
async fn duplicate_method_and_path_is_a_conflict() {
    let f = skip_without_db!();
    f.create_endpoint("a", "/reports/revenue", "SELECT 1").await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "b", "path": "reports/revenue",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 2" }]
            }),
        )
        .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the UNIQUE (workspace, method, path) must surface as 409. body: {body}"
    );
    // The author has to be told WHICH path collided — a bare "conflict" on a
    // screen with a dozen drafts is not actionable.
    let msg = body["error"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("")
        .to_string();
    assert!(
        msg.contains("/reports/revenue"),
        "the 409 must name the colliding path: {body}"
    );
}

/// The UNIQUE is only a duplicate-URL guard if the stored path is CANONICAL.
/// `reports/revenue` and `/reports/revenue` are the same URL to a client but two
/// distinct text values to Postgres, so without normalisation the constraint
/// would happily admit both and P4 would have two endpoints claiming one route.
#[tokio::test]
async fn a_path_differing_only_by_its_leading_slash_is_the_same_path() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, created) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "a", "path": "reports/revenue/",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    assert_eq!(
        created["path"].as_str().expect("path"),
        "/reports/revenue",
        "the stored path must be canonical"
    );

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "b", "path": "/reports/revenue",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 2" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
}

/// A different METHOD at the same path is a different endpoint — the UNIQUE is
/// on the pair, and a 409 here would be wrong.
#[tokio::test]
async fn the_same_path_under_a_different_method_is_allowed() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;
    for method in ["GET", "POST"] {
        let (status, body) = f
            .post_as(
                &mgr,
                &f.collection(),
                serde_json::json!({
                    "name": method, "path": "/reports/revenue", "method": method,
                    "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{method} body: {body}");
    }
}

#[tokio::test]
async fn renaming_a_path_onto_an_existing_one_is_a_conflict() {
    let f = skip_without_db!();
    f.create_endpoint("a", "/reports/revenue", "SELECT 1").await;
    let id = f.create_endpoint("b", "/reports/cost", "SELECT 2").await;
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "path": "/reports/revenue" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    let msg = body["error"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("")
        .to_string();
    assert!(
        msg.contains("/reports/revenue"),
        "the 409 must name the colliding path: {body}"
    );
}

// ---------- The draft itself ----------

/// One round trip through every verb, asserting the fields actually persist.
/// Without this the negative tests above would all pass against a router whose
/// handlers did nothing at all.
#[tokio::test]
async fn a_draft_round_trips_through_create_get_update_and_delete() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, created) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "revenue",
                "description": "daily revenue",
                "path": "/reports/revenue",
                "method": "POST",
                "database_id": f.database_id,
                "statements": [{ "name": "main", "sql": "SELECT 1" }],
                "params": [{
                    "name": "region", "type": "string", "location": "query",
                    "required": true, "description": "Region code"
                }],
                "output": { "shape": "arrays", "naming": "camel" },
                "cache": { "enabled": true, "ttl_seconds": 120, "scope": "shared" },
                "limits": { "max_rows": 500, "timeout_seconds": 15 },
                "rate_limit": { "per_key_qps": 5, "burst": 10, "per_endpoint_qps": 50 }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let id: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(created["state"].as_str(), Some("draft"));
    assert_eq!(created["method"].as_str(), Some("POST"));
    assert!(created["live_version_id"].is_null());

    let (status, got) = f.get_as(&mgr, &format!("{}/{id}", f.collection())).await;
    assert_eq!(status, StatusCode::OK, "body: {got}");
    assert_eq!(got["name"].as_str(), Some("revenue"));
    assert_eq!(got["description"].as_str(), Some("daily revenue"));
    assert_eq!(got["statements"][0]["name"].as_str(), Some("main"));
    assert_eq!(got["statements"][0]["sql"].as_str(), Some("SELECT 1"));
    assert_eq!(got["params"][0]["name"].as_str(), Some("region"));
    assert_eq!(got["params"][0]["type"].as_str(), Some("string"));
    assert_eq!(got["output"]["shape"].as_str(), Some("arrays"));
    assert_eq!(got["cache"]["scope"].as_str(), Some("shared"));
    assert_eq!(got["cache"]["ttl_seconds"].as_u64(), Some(120));
    assert_eq!(got["limits"]["max_rows"].as_u64(), Some(500));
    assert_eq!(got["rate_limit"]["burst"].as_u64(), Some(10));

    let (status, patched) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "statements": [{ "name": "main", "sql": "SELECT 2" }], "limits": { "max_rows": 7 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {patched}");
    assert_eq!(patched["statements"][0]["sql"].as_str(), Some("SELECT 2"));
    assert_eq!(patched["limits"]["max_rows"].as_u64(), Some(7));
    // Untouched fields survive a partial PATCH.
    assert_eq!(patched["name"].as_str(), Some("revenue"));
    assert_eq!(patched["cache"]["ttl_seconds"].as_u64(), Some(120));
    assert_eq!(patched["params"][0]["name"].as_str(), Some("region"));

    let (status, body) = f
        .send(&mgr, "DELETE", &format!("{}/{id}", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let (status, _) = f.get_as(&mgr, &format!("{}/{id}", f.collection())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "it really is gone");
}

/// An unset spec field must come back as its documented default, not as `{}` or
/// an empty string. The row stores `'{}'::jsonb`, and it is `serde`'s defaults —
/// not the derive's — that have to fill it in.
#[tokio::test]
async fn absent_spec_fields_come_back_as_their_serde_defaults() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, created) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "bare", "path": "/reports/bare",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    assert_eq!(
        created["method"].as_str(),
        Some("GET"),
        "the default method"
    );
    assert_eq!(created["output"]["shape"].as_str(), Some("objects"));
    assert_eq!(created["output"]["naming"].as_str(), Some("as_is"));
    // The one default with a WIRE consequence: `bare` here would strip the
    // envelope off every endpoint anybody creates without asking for it.
    assert_eq!(created["output"]["envelope"].as_str(), Some("wrapped"));
    assert_eq!(created["cache"]["enabled"].as_bool(), Some(false));
    assert_eq!(created["cache"]["ttl_seconds"].as_u64(), Some(60));
    assert_eq!(created["cache"]["scope"].as_str(), Some("per_principal"));
    assert_eq!(created["limits"]["max_rows"].as_u64(), Some(10_000));
    assert_eq!(created["limits"]["timeout_seconds"].as_u64(), Some(30));
    assert_eq!(created["params"].as_array().expect("params").len(), 0);
}

/// A method the CHECK constraint would reject must be a 400 from the route, not
/// a 500 surfacing a constraint name to the author.
#[tokio::test]
async fn an_unsupported_method_is_a_bad_request() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "nope", "path": "/reports/nope", "method": "DELETE",
                "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn an_empty_name_or_path_is_a_bad_request() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    for (name, path) in [("", "/reports/x"), ("x", ""), ("x", "   ")] {
        let (status, body) = f
            .post_as(
                &mgr,
                &f.collection(),
                serde_json::json!({
                    "name": name, "path": path,
                    "database_id": f.database_id, "statements": [{ "name": "main", "sql": "SELECT 1" }]
                }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "name={name:?} path={path:?} body: {body}"
        );
    }
}

/// The draft row holds a LIST, in the order the author wrote it, and a PATCH
/// replaces that list whole.
///
/// The one test on this surface that a single-statement fixture cannot stand in
/// for: every other test here happens to use one statement, and every one of
/// them would still pass against a store that kept only the first.
#[tokio::test]
async fn a_draft_holds_several_named_statements_in_order() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    let (status, created) = f
        .post_as(
            &mgr,
            &f.collection(),
            serde_json::json!({
                "name": "pair", "path": "/reports/pair",
                "database_id": f.database_id,
                "statements": [
                    { "name": "rows", "sql": "SELECT 1 AS n" },
                    { "name": "total", "sql": "SELECT 2 AS n" },
                ]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let id: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");

    // Read back through the store, not just echoed from the request body: the
    // jsonb round trip is where a dropped or reordered element would show.
    let (status, got) = f.get_as(&mgr, &format!("{}/{id}", f.collection())).await;
    assert_eq!(status, StatusCode::OK, "body: {got}");
    let statements = got["statements"].as_array().expect("statements");
    assert_eq!(statements.len(), 2, "both statements must survive: {got}");
    assert_eq!(statements[0]["name"].as_str(), Some("rows"));
    assert_eq!(statements[0]["sql"].as_str(), Some("SELECT 1 AS n"));
    assert_eq!(statements[1]["name"].as_str(), Some("total"));
    assert_eq!(statements[1]["sql"].as_str(), Some("SELECT 2 AS n"));

    // Order is part of the definition — statements execute in order — so a
    // store that returned a set rather than a sequence would fail here.
    let (status, patched) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({
                "statements": [
                    { "name": "total", "sql": "SELECT 2 AS n" },
                    { "name": "rows", "sql": "SELECT 1 AS n" },
                ]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {patched}");
    assert_eq!(patched["statements"][0]["name"].as_str(), Some("total"));
    assert_eq!(patched["statements"][1]["name"].as_str(), Some("rows"));

    // A PATCH replaces the list whole rather than merging into it.
    let (status, patched) = f
        .patch_as(
            &mgr,
            &format!("{}/{id}", f.collection()),
            serde_json::json!({ "statements": [{ "name": "only", "sql": "SELECT 3 AS n" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {patched}");
    assert_eq!(
        patched["statements"].as_array().expect("statements").len(),
        1,
        "a PATCH replaces the list, it does not merge into it: {patched}"
    );
}

/// `output.envelope` is checked on the way IN, not only fallen back to on the
/// way out.
///
/// The serving path answers an unrecognised envelope with the documented default
/// and a `tracing::warn!`, which is right for a snapshot that is already frozen
/// and wrong as the only defence: `"Bare"` is stored, echoed back to the author
/// by the API confirming their choice, and then served WRAPPED — the opposite of
/// what they asked for — with nothing in the response saying so. The near-miss
/// spellings are the cases that matter, so both are asserted.
#[tokio::test]
async fn an_unrecognised_output_envelope_is_refused_on_the_way_in() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    for bad in ["Bare", "bare ", "", "wrapped\n"] {
        let (status, body) = f
            .post_as(
                &mgr,
                &f.collection(),
                serde_json::json!({
                    "name": "env", "path": format!("/env/{}", Uuid::new_v4().simple()),
                    "database_id": f.database_id,
                    "statements": [{ "name": "rows", "sql": "SELECT 1" }],
                    "output": { "envelope": bad },
                }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
        assert!(
            body.to_string()
                .contains("is not one of `wrapped` or `bare`"),
            "{bad:?}: the message must name the rule: {body}"
        );
    }

    // Both spellings that ARE the contract still pass, so the rule above is a
    // rule and not a field that can no longer be set.
    let ep = f.create_endpoint("envok", "/envok", "SELECT 1").await;
    for good in ["wrapped", "bare"] {
        let (status, body) = f
            .patch_as(
                &mgr,
                &format!("{}/{ep}", f.collection()),
                serde_json::json!({ "output": { "envelope": good } }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{good:?}: {body}");
        assert_eq!(body["output"]["envelope"], good, "{body}");
    }

    // And the PATCH is a second call site: a value that may not be created must
    // not be reachable by editing a row that holds a legitimate one.
    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("{}/{ep}", f.collection()),
            serde_json::json!({ "output": { "envelope": "Bare" } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let still: serde_json::Value =
        sqlx::query_scalar("SELECT output FROM dataapi.endpoints WHERE id = $1")
            .bind(ep)
            .fetch_one(&f.db.pool)
            .await
            .expect("output");
    assert_eq!(
        still["envelope"], "bare",
        "the refusal must precede the write: {still}"
    );
}

/// A statement list that breaks the naming rules is a 400 that names the rule —
/// not a 500 carrying `dataapi_endpoints_statements_nonempty` at the author,
/// which is what the schema alone would produce.
///
/// Each case asserts on the MESSAGE, not just the status: 400 is the answer to
/// an empty name, an empty list, a bad path and an unsupported method alike, so
/// a status on its own cannot say which refusal fired.
#[tokio::test]
async fn a_statement_list_that_breaks_the_rules_is_a_400_naming_the_rule() {
    let f = skip_without_db!();
    let mgr = f.principal_with(Fixture::MANAGER).await;

    for (statements, expected, why) in [
        (
            serde_json::json!([]),
            "at least one statement",
            "empty list",
        ),
        (
            serde_json::json!([
                { "name": "rows", "sql": "SELECT 1" },
                { "name": "rows", "sql": "SELECT 2" },
            ]),
            "rows",
            "a repeated name",
        ),
        (
            serde_json::json!([{ "name": "Rows", "sql": "SELECT 1" }]),
            "Rows",
            "a name that is not an identifier",
        ),
        // The LENGTH, which the identifier rule says nothing about: every
        // character below is one the alphabet rule accepts. The name becomes a
        // JSON key in every response body and a property name in the generated
        // document, neither of which has a limit of its own.
        (
            serde_json::json!([{
                "name": "r".repeat(egret_dataapi_api::MAX_STATEMENT_NAME_LEN + 1),
                "sql": "SELECT 1",
            }]),
            "and the limit is 64",
            "a name past the length limit",
        ),
        // The upper bound on the COUNT. Refused on the WRITE route, not only at
        // publish: statements run sequentially, so the count multiplies the
        // endpoint's worst-case wall clock and the CPU publish spends rendering
        // it, and an author who learns that only when they press Publish has
        // already written every one of them.
        (
            serde_json::Value::Array(
                (0..=egret_dataapi_api::MAX_STATEMENTS)
                    .map(|i| serde_json::json!({ "name": format!("s{i}"), "sql": "SELECT 1" }))
                    .collect(),
            ),
            "and the limit is 16",
            "more statements than the limit",
        ),
    ] {
        let (status, body) = f
            .post_as(
                &mgr,
                &f.collection(),
                serde_json::json!({
                    "name": "bad", "path": format!("/bad/{}", Uuid::new_v4().simple()),
                    "database_id": f.database_id, "statements": statements,
                }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(
            body.to_string().contains(expected),
            "{why}: the message must name what was wrong, expected {expected:?}: {body}"
        );
    }

    // And on the PATCH, where the list arrives the same way and the row already
    // holds a legitimate one.
    let ep = f
        .create_endpoint("patchable", "/patchable", "SELECT 1")
        .await;
    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("{}/{ep}", f.collection()),
            serde_json::json!({ "statements": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body.to_string().contains("at least one statement"),
        "the PATCH must be refused for the same reason: {body}"
    );

    // And the count, on the PATCH too: it is a second call site, and a list that
    // may not be created must not be reachable by editing one that could.
    let (status, body) = f
        .patch_as(
            &mgr,
            &format!("{}/{ep}", f.collection()),
            serde_json::json!({
                "statements": (0..=egret_dataapi_api::MAX_STATEMENTS)
                    .map(|i| serde_json::json!({ "name": format!("s{i}"), "sql": "SELECT 1" }))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body.to_string().contains("and the limit is 16"),
        "the PATCH must name the count rule it broke: {body}"
    );

    // Anti-triviality: the row still holds what it held, so the refusal above is
    // a refusal and not a 400 issued after the write.
    let still: serde_json::Value =
        sqlx::query_scalar("SELECT statements FROM dataapi.endpoints WHERE id = $1")
            .bind(ep)
            .fetch_one(&f.db.pool)
            .await
            .expect("statements");
    assert_eq!(
        still,
        serde_json::json!([{ "name": "main", "sql": "SELECT 1" }])
    );
}
