//! Bookmarking an endpoint: `POST`/`DELETE
//! /api/ws/{workspace}/data-api/endpoints/{id}/bookmark`, and the per-user
//! `bookmarked` flag the list carries.
//!
//! Four properties this file exists to hold:
//!
//! 1. **The toggle is idempotent in both directions.** The UI's star is a
//!    toggle and a double click must not 500 — `ON CONFLICT DO NOTHING` on the
//!    insert, and a DELETE that matches nothing is a no-op.
//! 2. **A bookmark belongs to the SESSION's user.** The `user_id` written and
//!    the `user_id` the list's `EXISTS` reads both come from `ctx.principal`;
//!    if either came from anywhere else, one person's star would light for
//!    everybody.
//! 3. **`:read` is enough.** Bookmarking is a per-user view preference, not an
//!    authoring act, so it gates on `data_api_endpoint:read` exactly as every
//!    sibling seam does. Permissions here are FLAT — a reader holds no
//!    `:manage` at all, so a route gated on `:manage` would lock every reader
//!    out of their own stars.
//! 4. **An endpoint in another workspace is 404, never 403** — absence and
//!    denial must be indistinguishable from outside, the rule this crate's
//!    other suites already hold. The containment resolve also has to happen
//!    BEFORE the bookmarks table is touched: `analytics.bookmarks` has no
//!    workspace column of its own, so nothing else would refuse a foreign id.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like
//! `endpoints_routes.rs` — so check the duration, not just the `ok`.

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
    admin: Uuid,
    database_id: Uuid,
}

/// A principal: a router, a session token, and the user id behind both, so a
/// test can go and look at the row that principal's request wrote.
struct Caller {
    app: axum::Router,
    tok: String,
    user: Uuid,
}

/// A `QueryExecPort` that must never be consulted. Bookmarking talks to no
/// engine; a call that reached a coordinator would be a bug this makes loud.
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
        unreachable!("bookmarking must never reach the query engine")
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, egret_core::Error> {
        unreachable!("bookmarking must never reach the query engine")
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
        keys: Arc::new(egret_machine_idp::PgSigningKeyStore::new(db.clone())),
        issuer: "https://egret.test".into(),
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
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
         VALUES ($1, 'Data API Bookmarks Test', $2, 'active') RETURNING id",
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
    let admin = insert_user(&db, "dataapi_bm_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "dataapi-bm").await;
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
    /// `ws_admin` holds both (migration 0122); a reader holds only `:read`.
    const MANAGER: &'static [&'static str] =
        &["data_api_endpoint:read", "data_api_endpoint:manage"];

    /// A draft written straight to the table — tests about bookmarking should
    /// not depend on the create route also being correct.
    async fn create_endpoint(&self, name: &str, path: &str) -> Uuid {
        self.create_endpoint_in(self.ws_id, self.database_id, name, path)
            .await
    }

    async fn create_endpoint_in(
        &self,
        ws: Uuid,
        database_id: Uuid,
        name: &str,
        path: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), $5) RETURNING id",
        )
        .bind(ws)
        .bind(name)
        .bind(path)
        .bind(database_id)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint draft")
    }

    async fn create_workspace(&self, tag: &str) -> (Uuid, String) {
        insert_workspace(&self.db, self.admin, tag).await
    }

    async fn create_database_in(&self, ws: Uuid) -> Uuid {
        insert_database(&self.db, ws, self.admin).await
    }

    async fn principal_with(&self, perms: &[&str]) -> Caller {
        self.principal_with_in(perms, self.ws_id).await
    }

    async fn principal_with_in(&self, perms: &[&str], ws: Uuid) -> Caller {
        let user = insert_user(&self.db, "dataapi_bm_caller").await;
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name, description) \
             VALUES ('workspace', $1, 'test') RETURNING id",
        )
        .bind(format!("test_dataapi_bm_{}", Uuid::new_v4().simple()))
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

        // The REAL Cedar PDP: a mocked decision would pin the consumer, not the
        // server. Rebuilt per principal because it snapshots policies at build
        // time and a session caches its permission set.
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
        let app = axum::Router::new()
            .nest("/api", management_router())
            .with_state(st);
        Caller { app, tok, user }
    }

    async fn send(&self, c: &Caller, method: &str, path: &str) -> (StatusCode, serde_json::Value) {
        let res = c
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("Cookie", format!("egret_session={}", c.tok))
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

    fn collection(&self) -> String {
        format!("/api/ws/{}/data-api/endpoints", self.ws)
    }

    fn bookmark_url(&self, id: Uuid) -> String {
        format!("{}/{id}/bookmark", self.collection())
    }

    /// Rows in the generic table for this (user, endpoint) pair. Written as a
    /// literal `entity_type` on purpose: if the store's constant were renamed,
    /// the code would keep agreeing with itself and this would go red.
    async fn bookmark_rows(&self, user: Uuid, endpoint: Uuid) -> i64 {
        self.raw_bookmark_rows(user, "data_api_endpoint", endpoint)
            .await
    }

    /// The same count for ANY `entity_type`, so a test can watch a row this
    /// crate's routes must never touch — a neighbouring seam's bookmark keyed
    /// on an endpoint's id.
    async fn raw_bookmark_rows(&self, user: Uuid, entity_type: &str, entity_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM analytics.bookmarks \
             WHERE user_id = $1 AND entity_type = $2 AND entity_id = $3",
        )
        .bind(user)
        .bind(entity_type)
        .bind(entity_id)
        .fetch_one(&self.db.pool)
        .await
        .expect("count bookmarks")
    }

    /// A bookmark row written straight to the generic table, so a test can put
    /// one there that the routes would never write. `analytics.bookmarks` has a
    /// FK on `user_id` only (0020) — `entity_type` is unconstrained text and
    /// `entity_id` a bare uuid with no FK — so a `'chart'` row keyed on an
    /// endpoint's id is perfectly insertable, which is exactly the row a query
    /// that forgot its `entity_type` predicate would pick up.
    async fn insert_raw_bookmark(&self, user: Uuid, entity_type: &str, entity_id: Uuid) {
        sqlx::query(
            "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(user)
        .bind(entity_type)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await
        .expect("insert raw bookmark");
    }

    /// The `bookmarked` flag the list shows `c` for `id`.
    async fn listed_bookmarked(&self, c: &Caller, id: Uuid) -> bool {
        let (status, body) = self.send(c, "GET", &self.collection()).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        let row = body
            .as_array()
            .expect("array")
            .iter()
            .find(|r| r["id"].as_str() == Some(&id.to_string()))
            .unwrap_or_else(|| panic!("endpoint {id} missing from list: {body}"));
        row["bookmarked"]
            .as_bool()
            .unwrap_or_else(|| panic!("no `bookmarked` on the list row: {row}"))
    }
}

/// The star is a toggle: a double click must not 500, in either direction.
#[tokio::test]
async fn bookmarking_is_idempotent_in_both_directions() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue").await;
    let c = f.principal_with(Fixture::MANAGER).await;
    let url = f.bookmark_url(id);

    let (first, body) = f.send(&c, "POST", &url).await;
    assert_eq!(first, StatusCode::OK, "body: {body}");
    let (second, body) = f.send(&c, "POST", &url).await;
    assert_eq!(
        second,
        StatusCode::OK,
        "a second bookmark must not error: {body}"
    );
    assert_eq!(
        f.bookmark_rows(c.user, id).await,
        1,
        "two POSTs must leave exactly one row"
    );

    let (first, body) = f.send(&c, "DELETE", &url).await;
    assert_eq!(first, StatusCode::OK, "body: {body}");
    let (second, body) = f.send(&c, "DELETE", &url).await;
    assert_eq!(
        second,
        StatusCode::OK,
        "a second unbookmark must not error: {body}"
    );
    assert_eq!(
        f.bookmark_rows(c.user, id).await,
        0,
        "the bookmark must be gone"
    );
}

/// A bookmark is per user. The `user_id` written and the one the list reads
/// both come from the session; taking either from anywhere else would light
/// one person's star for everybody.
#[tokio::test]
async fn a_bookmark_is_per_user() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue").await;
    let a = f.principal_with(Fixture::MANAGER).await;
    let b = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f.send(&a, "POST", &f.bookmark_url(id)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    assert!(
        f.listed_bookmarked(&a, id).await,
        "A bookmarked it, so A must see it starred"
    );
    assert!(
        !f.listed_bookmarked(&b, id).await,
        "B never bookmarked it and must not see A's star"
    );
    assert_eq!(f.bookmark_rows(b.user, id).await, 0, "no row for B");
}

/// Un-starring is per user and per kind too — the DELETE's predicates, which
/// the READ-side tests above cannot reach.
///
/// The delete runs against a generic table with no workspace and no ownership
/// of its own, so its `WHERE` is the entire containment. A fixture with one
/// user and one bookmark kind cannot tell "delete MY endpoint star" from
/// "delete every star on this row" or "delete every bookmark on this uuid" —
/// all three destroy the same single row. So this one arranges a state where
/// they differ: two users starring the SAME endpoint, plus a `'chart'` bookmark
/// keyed on that endpoint's id and owned by the deleting user.
///
/// Dropping `user_id` un-stars the endpoint for everyone in the workspace;
/// dropping `entity_type` reaches into a neighbouring seam's bookmarks. Both
/// return 200, write nothing to any log, and are visible only as somebody
/// else's star quietly going out.
#[tokio::test]
async fn unbookmarking_touches_only_this_users_endpoint_star() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue").await;
    let a = f.principal_with(Fixture::MANAGER).await;
    let b = f.principal_with(Fixture::MANAGER).await;

    // BOTH users star the same endpoint, through the route.
    let (status, body) = f.send(&a, "POST", &f.bookmark_url(id)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let (status, body) = f.send(&b, "POST", &f.bookmark_url(id)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // And A holds another seam's bookmark on the very same uuid.
    f.insert_raw_bookmark(a.user, "chart", id).await;

    assert_eq!(f.bookmark_rows(a.user, id).await, 1, "A starred it");
    assert_eq!(f.bookmark_rows(b.user, id).await, 1, "B starred it");

    let (status, body) = f.send(&a, "DELETE", &f.bookmark_url(id)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    assert_eq!(
        f.bookmark_rows(a.user, id).await,
        0,
        "A asked to un-star it, so A's row must be gone"
    );
    assert_eq!(
        f.bookmark_rows(b.user, id).await,
        1,
        "B's star on the same endpoint must survive A's DELETE"
    );
    assert!(
        f.listed_bookmarked(&b, id).await,
        "and B must still SEE it starred, not merely have a row"
    );
    assert_eq!(
        f.raw_bookmark_rows(a.user, "chart", id).await,
        1,
        "a `chart` bookmark on the same uuid belongs to another seam and must survive"
    );
}

/// Bookmarking is a VIEW PREFERENCE, so `:read` is enough. Permissions here are
/// flat: a reader holds no `:manage` at all, so a route gated on `:manage`
/// would lock every reader out of their own stars.
#[tokio::test]
async fn read_permission_is_enough_to_bookmark() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue").await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;

    let (status, body) = f.send(&reader, "POST", &f.bookmark_url(id)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a reader must be able to bookmark: {body}"
    );
    assert_eq!(f.bookmark_rows(reader.user, id).await, 1);
    assert!(f.listed_bookmarked(&reader, id).await);

    let (status, body) = f.send(&reader, "DELETE", &f.bookmark_url(id)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a reader must be able to unbookmark: {body}"
    );
    assert_eq!(f.bookmark_rows(reader.user, id).await, 0);
}

/// An endpoint in another workspace is 404, never 403 — and nothing is written.
/// `analytics.bookmarks` carries no workspace of its own, so the containment
/// resolve is the only thing standing between a caller and a row keyed on a
/// foreign endpoint id.
#[tokio::test]
async fn bookmarking_another_workspaces_endpoint_is_404() {
    let f = skip_without_db!();
    let (other_ws, _slug) = f.create_workspace("dataapi-bm-other").await;
    let other_db = f.create_database_in(other_ws).await;
    let foreign = f
        .create_endpoint_in(other_ws, other_db, "theirs", "/theirs")
        .await;
    // A legitimate manager in the fixture's workspace, reaching for an id that
    // belongs to another one.
    let c = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f.send(&c, "POST", &f.bookmark_url(foreign)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a foreign endpoint must be indistinguishable from an absent one: {body}"
    );
    assert_eq!(
        f.bookmark_rows(c.user, foreign).await,
        0,
        "a 404 returned AFTER the insert would look the same from outside"
    );

    let (status, body) = f.send(&c, "DELETE", &f.bookmark_url(foreign)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the same on the way out: {body}"
    );
}

/// The flag is per ROW and per KIND, not merely per user.
///
/// The `EXISTS` sub-select has three predicates and a test with one endpoint
/// and one bookmark kind cannot tell them apart: with a single row in the
/// workspace, "this user has a bookmark" and "this user has a bookmark for THIS
/// endpoint, as an endpoint" are the same sentence. So this one holds a
/// workspace where they differ — two endpoints with only the first starred, and
/// a `'chart'` bookmark deliberately keyed on the SECOND endpoint's id.
///
/// A correlation dropped from the sub-select would light every star in the
/// list for anyone holding any bookmark at all; a dropped `entity_type` would
/// let another seam's star light an endpoint. Both are silent, both look
/// exactly like a working feature until someone bookmarks a second thing.
#[tokio::test]
async fn the_flag_is_per_endpoint_and_per_entity_type() {
    let f = skip_without_db!();
    let starred = f.create_endpoint("rev", "/reports/revenue").await;
    let plain = f.create_endpoint("cost", "/reports/cost").await;
    let c = f.principal_with(Fixture::MANAGER).await;

    let (status, body) = f.send(&c, "POST", &f.bookmark_url(starred)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // Same user, same uuid as `plain`, different KIND. Nothing about this row
    // concerns the endpoint list.
    f.insert_raw_bookmark(c.user, "chart", plain).await;

    assert!(
        f.listed_bookmarked(&c, starred).await,
        "the endpoint this caller starred must read true"
    );
    assert!(
        !f.listed_bookmarked(&c, plain).await,
        "an endpoint this caller never starred must read false, even though the \
         caller holds a bookmark elsewhere and a `chart` bookmark on this very id"
    );
}
