//! Issuing a client for a service account hands out that account's access.
//! Written from the caller's side: a real non-admin principal against the REAL
//! Cedar PDP, not a mocked decision — a mocked flag pins the consumer, not the
//! server, and an admin measurement bypasses the check entirely.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like
//! `clients_store.rs`; run with
//! `DATABASE_URL=postgres://egret:egret@localhost:5432/egret`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_dataapi::{management_router, ClientStore, DataApiState};
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
}

/// A principal: a router + session token whose PDP already sees its grants.
/// Both are rebuilt per principal because `CedarPdp` snapshots policies at build
/// time and a session caches its permission set.
struct Caller {
    app: axum::Router,
    tok: String,
}

/// A `QueryExecPort` that must never be consulted: nothing in this file drives
/// the publish lifecycle, and a route that reached the engine anyway would be
/// doing something none of these tests asked for.
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
        unreachable!("no route in this test should reach the query engine")
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, egret_core::Error> {
        unreachable!("no route in this test should reach the query engine")
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> DataApiState {
    DataApiState {
        db: db.clone(),
        clients: ClientStore::new(db.clone()),
        endpoints: egret_dataapi::EndpointStore::new(db.clone()),
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
        // A sink whose receiver is dropped immediately: `emit` is best-effort
        // and logs rather than failing when the channel is closed, so nothing in
        // this file depends on it. What the audited routes emit is asserted in
        // `audit.rs`, which keeps the receiver.
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // This file is about route-level authz, not Activity — a fixed
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
         VALUES ($1, 'Data API Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn fixture() -> Option<Fixture> {
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    let admin = insert_user(&db, "dataapi_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "dataapi").await;
    Some(Fixture {
        db,
        ws,
        ws_id,
        admin,
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
    /// A service account in `self.ws`: the `users` row plus the workspace
    /// binding, written directly — these tests are about WHO MAY ISSUE, so the
    /// account's own creation route is not the subject here.
    async fn create_service_account(&self, name: &str) -> Uuid {
        self.create_service_account_in(name, self.ws_id).await
    }

    async fn create_service_account_in(&self, name: &str, ws: Uuid) -> Uuid {
        let sa: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status) \
             VALUES ('service', $1, $2, 'active') RETURNING id",
        )
        .bind(format!(
            "svc.{name}.{}@svc.egret.local",
            Uuid::new_v4().simple()
        ))
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .expect("service account user");
        sqlx::query(
            "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, created_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(sa)
        .bind(ws)
        .bind(name)
        .bind(self.admin)
        .execute(&self.db.pool)
        .await
        .expect("service_accounts row");
        sa
    }

    async fn create_workspace(&self, tag: &str) -> (Uuid, String) {
        insert_workspace(&self.db, self.admin, tag).await
    }

    /// A brand-new, non-admin user holding exactly `perms` in `self.ws`.
    async fn principal_with(&self, perms: &[&str]) -> Caller {
        self.principal_with_in(perms, self.ws_id).await
    }

    /// The realistic manager: `ws_admin` is seeded with BOTH `:read` and
    /// `:manage` (migration 0120), because egret's permissions are flat and
    /// `:manage` implies nothing. A manage-only principal is not a role anyone
    /// holds, so tests that exercise the manage path grant the real pair; the
    /// asymmetry that matters — read alone cannot ISSUE — is pinned separately
    /// by `read_permission_cannot_issue_a_client`.
    const MANAGER: &'static [&'static str] = &["service_account:read", "service_account:manage"];

    /// The same, in an arbitrary workspace — the cross-workspace test needs a
    /// principal who is a legitimate MANAGER somewhere else.
    async fn principal_with_in(&self, perms: &[&str], ws: Uuid) -> Caller {
        let user = insert_user(&self.db, "dataapi_caller").await;
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name, description) \
             VALUES ('workspace', $1, 'test') RETURNING id",
        )
        .bind(format!("test_dataapi_{}", Uuid::new_v4().simple()))
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

    async fn get_as(&self, c: &Caller, path: &str) -> (StatusCode, serde_json::Value) {
        self.send(c, "GET", path, None).await
    }
}

#[tokio::test]
async fn read_permission_cannot_issue_a_client() {
    let f = skip_without_db!();
    let sa = f.create_service_account("reporting").await;
    let reader = f.principal_with(&["service_account:read"]).await;

    let (status, _) = f
        .post_as(
            &reader,
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
            serde_json::json!({ "name": "prod" }),
        )
        .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "read must not imply issue");
}

#[tokio::test]
async fn a_client_cannot_be_issued_across_workspaces() {
    let f = skip_without_db!();
    let sa = f.create_service_account("reporting").await; // in f.ws
    let (other_id, other_ws) = f.create_workspace("other").await;
    let manager = f.principal_with_in(Fixture::MANAGER, other_id).await;

    let (status, _) = f
        .post_as(
            &manager,
            &format!("/api/ws/{other_ws}/data-api/service-accounts/{sa}/clients"),
            serde_json::json!({ "name": "prod" }),
        )
        .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the account does not belong to that workspace"
    );
}

#[tokio::test]
async fn the_secret_is_returned_once_and_never_again() {
    let f = skip_without_db!();
    let sa = f.create_service_account("reporting").await;
    let manager = f.principal_with(Fixture::MANAGER).await;
    let path = format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws);

    let (status, created) = f
        .post_as(&manager, &path, serde_json::json!({ "name": "prod" }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    assert!(created["client_id"]
        .as_str()
        .expect("client_id")
        .starts_with("egret-dataapi-"));
    assert!(!created["client_secret"]
        .as_str()
        .expect("client_secret")
        .is_empty());

    let (status, listed) = f.get_as(&manager, &path).await;
    assert_eq!(status, StatusCode::OK, "body: {listed}");
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert!(
        listed[0].get("client_secret").is_none(),
        "the client list must never carry secret material: {listed}"
    );
}

/// The read side must still work for a reader — otherwise
/// `read_permission_cannot_issue_a_client` would pass just as well against a
/// router that denied `:read` everything.
#[tokio::test]
async fn read_permission_can_still_list() {
    let f = skip_without_db!();
    let sa = f.create_service_account("reporting").await;
    let reader = f.principal_with(&["service_account:read"]).await;

    let (status, body) = f
        .get_as(
            &reader,
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body.as_array().expect("array").len(), 0);
}

/// Creating an account is a `:manage` act too, and it must produce BOTH rows:
/// the `users` principal and the workspace binding. A `users` row alone would be
/// an identity no workspace owns.
#[tokio::test]
async fn creating_an_account_makes_a_real_principal_bound_to_the_workspace() {
    let f = skip_without_db!();
    let manager = f.principal_with(Fixture::MANAGER).await;

    let (status, created) = f
        .post_as(
            &manager,
            &format!("/api/ws/{}/data-api/service-accounts", f.ws),
            serde_json::json!({ "name": "Nightly ETL" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let id: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(
        created["email"].as_str().expect("email"),
        format!("svc.nightly-etl@{}.service.egret.local", f.ws)
    );

    let provider: String = sqlx::query_scalar("SELECT provider FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&f.db.pool)
        .await
        .expect("users row");
    assert_eq!(provider, "service");
    let bound: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM dataapi.service_accounts WHERE user_id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("service_accounts row");
    assert_eq!(bound, f.ws_id);
}

/// Deleting a service account must take its `access_grants` rows with it.
///
/// `access_grants.principal_id` is polymorphic and has NO foreign key to
/// `users(id)` (migration 0068), and no trigger cleans it — so a bare
/// `DELETE FROM users` leaves grant rows pointing at a principal that no longer
/// exists. A service account is the case where this bites hardest: spec §5.3's
/// whole model is that an admin GRANTS the account a role through
/// `access_grants`, so the granted state is the normal state at deletion time.
#[tokio::test]
async fn deleting_an_account_takes_its_access_grants_with_it() {
    let f = skip_without_db!();
    let manager = f.principal_with(Fixture::MANAGER).await;
    let (status, created) = f
        .post_as(
            &manager,
            &format!("/api/ws/{}/data-api/service-accounts", f.ws),
            serde_json::json!({ "name": "granted" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let sa: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");

    // Grant it a workspace role, exactly as §5.3 says an admin would.
    let granted = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'workspace', $2 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(sa)
    .bind(f.ws_id)
    .execute(&f.db.pool)
    .await
    .expect("grant role")
    .rows_affected();
    assert_eq!(granted, 1, "ws_viewer must exist and have been granted");

    let (status, body) = f
        .send(
            &manager,
            "DELETE",
            &format!("/api/ws/{}/data-api/service-accounts/{sa}", f.ws),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type = 'user' AND principal_id = $1",
    )
    .bind(sa)
    .fetch_one(&f.db.pool)
    .await
    .expect("count grants");
    assert_eq!(
        left, 0,
        "the deleted principal must leave no access_grants rows behind"
    );

    // And the account itself is genuinely gone, so the assertion above cannot be
    // satisfied by a delete that silently did nothing at all.
    let users_left: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
        .bind(sa)
        .fetch_one(&f.db.pool)
        .await
        .expect("count users");
    assert_eq!(users_left, 0);
}

// ---------- Negative gates ----------
//
// One test per handler whose `require(...)` line had nothing watching it. The
// suite above measures the asymmetry between `:read` and `:manage` and the
// cross-workspace boundary, but every one of those calls arrives holding SOME
// permission — so deleting a handler's `require(...)` entirely would not have
// failed a single one of them. `principal_with(&[])` is a real, authenticated,
// workspace-member principal holding NO service-account permission: exactly the
// caller a missing gate lets through.

/// Deleting a service account is the most destructive verb here — it hard-deletes
/// the principal, its grants and every client it owns.
#[tokio::test]
async fn deleting_an_account_requires_manage() {
    let f = skip_without_db!();
    let sa = f.create_service_account("reporting").await;
    let nobody = f.principal_with(&[]).await;

    let (status, body) = f
        .send(
            &nobody,
            "DELETE",
            &format!("/api/ws/{}/data-api/service-accounts/{sa}", f.ws),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");

    // And the account really is still there — otherwise a 403 returned AFTER a
    // successful delete would read the same from outside.
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
        .bind(sa)
        .fetch_one(&f.db.pool)
        .await
        .expect("count users");
    assert_eq!(left, 1, "the refused delete must not have deleted anything");
}

/// Listing accounts is `:read`. The list names every machine principal in the
/// workspace, so it is not public to workspace membership.
#[tokio::test]
async fn listing_accounts_requires_read() {
    let f = skip_without_db!();
    f.create_service_account("reporting").await;
    let nobody = f.principal_with(&[]).await;

    let (status, body) = f
        .get_as(
            &nobody,
            &format!("/api/ws/{}/data-api/service-accounts", f.ws),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

/// Creating an account is `:manage` — it mints a new platform principal.
#[tokio::test]
async fn creating_an_account_requires_manage() {
    let f = skip_without_db!();
    let nobody = f.principal_with(&[]).await;

    let (status, body) = f
        .post_as(
            &nobody,
            &format!("/api/ws/{}/data-api/service-accounts", f.ws),
            serde_json::json!({ "name": "Sneaky" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");

    let made: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dataapi.service_accounts WHERE workspace_id = $1 AND name = 'Sneaky'",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("count accounts");
    assert_eq!(made, 0, "the refused create must not have created anything");
}

/// Revoking is `:manage`, and a client in another workspace is invisible rather
/// than merely forbidden — the same 404 rule as `scoped_account`.
#[tokio::test]
async fn a_client_cannot_be_revoked_across_workspaces() {
    let f = skip_without_db!();
    let sa = f.create_service_account("reporting").await;
    let manager = f.principal_with(Fixture::MANAGER).await;
    let (_, created) = f
        .post_as(
            &manager,
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
            serde_json::json!({ "name": "prod" }),
        )
        .await;
    let client_id: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");

    let (other_id, other_ws) = f.create_workspace("other").await;
    let outsider = f.principal_with_in(Fixture::MANAGER, other_id).await;
    let (status, _) = f
        .send(
            &outsider,
            "DELETE",
            &format!("/api/ws/{other_ws}/data-api/clients/{client_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Still usable: the cross-workspace call must not have revoked it.
    let (status, listed) = f
        .get_as(
            &manager,
            &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed[0]["revoked_at"].is_null());
}
