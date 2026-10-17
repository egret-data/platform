//! Trash and restore for Data API endpoints: migration 0126's schema guarantee,
//! and the two routes built on it.
//!
//! Two layers, and both are load-bearing.
//!
//! The **schema** layer is the first two tests. The serving path resolves an
//! endpoint through exactly one query (`serving/router.rs::live_snapshot`), which
//! filters `state = 'live'`. That predicate can be forgotten by whoever writes the
//! next query — the Alerts programme shipped exactly that bug, a soft-deleted
//! schedule that kept firing. A CHECK constraint cannot be forgotten the way a
//! WHERE clause can, so those tests go straight to SQL and prove the guarantee
//! holds against a writer that never read this plan, let alone a route handler.
//!
//! The **route** layer is everything after it, driven end-to-end over HTTP against
//! the real Cedar PDP. A CHECK violation reaches an author as a 500 naming
//! `dataapi_endpoints_trashed_not_live`, which tells them nothing about what to do;
//! the route refuses first, with a message naming disable-first. The two are not
//! redundant — the route owns the message, the constraint owns the promise — and
//! [`trashing_a_live_endpoint_is_refused_by_the_route_before_it_writes`] pins the
//! route half by asserting the status AND that the row was left alone.
//!
//! The subtlest test here is
//! [`a_trashed_endpoint_still_holds_its_url_and_the_conflict_says_so`]. It drives
//! the SAME method and path through both branches of the conflict message —
//! blocked by a live endpoint, then blocked by a trashed one — so the two strings
//! it compares can differ only in their reason. Two different paths would have
//! produced two different messages whether or not the branch existed.
//!
//! The last section is the **trash boundary against the state machine**: six
//! writers could reach a trashed row and none of them knew the trash existed. Two
//! failed loudly (a 500 from the CHECK) and four succeeded silently, which is why
//! each of those tests asserts the status IS 409 rather than merely "an error" — see
//! the section comment for the full argument.
//!
//! The consumer-side end state is a separate file, `trash_serving.rs`: it needs the
//! serving router, and its module doc is careful about what a serving test of the
//! trash can and cannot prove.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS, like every sibling
//! suite in this crate — so check the DURATION, not just the `ok`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_audit::{AuditEvent, AuditSink};
use egret_core::{ColumnMeta, Error, UserId};
use egret_dataapi::{management_router, ClientStore, DataApiState, EndpointStore};
use egret_metastore::Db;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tower::ServiceExt;
use uuid::Uuid;

/// `describe_as` is reached — the task-3 tests drive `POST /publish`, whose
/// pre-flight validation resolves the output columns before any write — so it
/// answers rather than panicking; a panic in a port surfaces as a 500 and would read
/// as a route bug. Nothing in this file SERVES, so `execute_as` is `unreachable!`: a
/// query executed from a trash test would mean a handler reached the engine, which
/// is a finding rather than a nuisance.
struct OkPort;

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for OkPort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![ColumnMeta {
            name: "n".into(),
            ty: "integer".into(),
        }])
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, Error> {
        unreachable!("trash and restore must never execute a query")
    }
}

/// One emitted mutation, flattened to what an assertion actually reads.
#[derive(Debug)]
struct Emitted {
    actor: Option<Uuid>,
    workspace_id: Option<Uuid>,
    action: String,
    resource_type: String,
    resource_id: Option<String>,
    detail: serde_json::Value,
}

impl Emitted {
    /// The event's shape, minus the detail — asserted as a tuple so a wrong actor,
    /// workspace, ACTION STRING, resource type or resource id all read as one
    /// diff. The action and the id are the two an event-counting test would miss:
    /// a handler emitting `data_api_endpoint:delete` from `trash`, or stamping the
    /// workspace id where the endpoint id belongs, emits exactly one event either
    /// way.
    fn shape(&self) -> (Option<Uuid>, Option<Uuid>, &str, &str, Option<&str>) {
        (
            self.actor,
            self.workspace_id,
            self.action.as_str(),
            self.resource_type.as_str(),
            self.resource_id.as_deref(),
        )
    }
}

struct Caller {
    app: axum::Router,
    tok: String,
    user: Uuid,
}

struct Fixture {
    db: Db,
    ws: String,
    ws_id: Uuid,
    admin: Uuid,
    database_id: Uuid,
    manager: Caller,
    /// The receiving half of the very sink the router holds. `emit` is a
    /// synchronous `try_send`, so an event is in this queue by the time the
    /// response has been returned — no sleeping, no polling.
    rx: Receiver<AuditEvent>,
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>, audit: AuditSink) -> DataApiState {
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
        query: Arc::new(OkPort),
        keys: Arc::new(egret_machine_idp::PgSigningKeyStore::new(db.clone())),
        issuer: "https://egret.test".into(),
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        audit,
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
         VALUES ($1, 'Data API Trash Test', $2, 'active') RETURNING id",
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

/// What the trash pair gates on: `:manage` to trash and restore, `:read` to list.
/// Held by ONE principal so a single caller can drive the whole surface — this file
/// is about the trash BEHAVIOUR, not about who may drive it.
///
/// The permission boundary for `/trash` and `/restore` is measured in
/// `lifecycle.rs`, beside every other lifecycle route:
/// `read_permission_cannot_drive_the_lifecycle` for the 403 a `:read`-only
/// principal gets, and `the_lifecycle_of_another_workspaces_endpoint_is_not_found`
/// for the 404 a manager in another workspace gets. Both also assert the row was
/// left alone, because a refusal that still wrote would look identical from the
/// status line.
///
/// It is NOT measured in `routes_authz.rs`, whatever an earlier version of this
/// comment claimed: that file covers service accounts and clients only and names
/// no endpoint route at all.
const MANAGER: &[&str] = &["data_api_endpoint:read", "data_api_endpoint:manage"];

async fn principal_with_in(db: &Db, audit: AuditSink, perms: &[&str], ws: Uuid) -> Caller {
    let user = insert_user(db, "trash_caller").await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_dataapi_trash_{}", Uuid::new_v4().simple()))
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
    let st = build_state(db.clone(), pdp, audit);
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

async fn fixture() -> Option<Fixture> {
    // One private database per test process, cloned from a migrated template.
    // See the `egret-test-db` crate for why sharing one database made the
    // suite slower every time it ran.
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    let admin = insert_user(&db, "dataapi_trash_admin").await;
    let (ws_id, ws) = insert_workspace(&db, admin, "dataapi-trash").await;
    let database_id = insert_database(&db, ws_id, admin).await;
    // Generous capacity: `emit` DROPS on a full channel rather than blocking, so a
    // tight one would turn "the handler emitted nothing" and "the test never
    // drained fast enough" into the same failure.
    let (sink, rx) = AuditSink::new(256);
    let manager = principal_with_in(&db, sink, MANAGER, ws_id).await;
    Some(Fixture {
        db,
        ws,
        ws_id,
        admin,
        database_id,
        manager,
        rx,
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
    /// An endpoint written straight to the table with the given `state`, so a test
    /// can put it in `'live'` without going through the publish route.
    async fn create_endpoint(&self, name: &str, path: &str, state: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO dataapi.endpoints \
               (workspace_id, name, path, method, database_id, statements, state, created_by) \
             VALUES ($1, $2, $3, 'GET', $4, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), $5, $6) RETURNING id",
        )
        .bind(self.ws_id)
        .bind(name)
        .bind(path)
        .bind(self.database_id)
        .bind(state)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint")
    }

    /// The same, already in the trash. Written in one statement rather than by
    /// calling the route, so the tests that READ the trash do not depend on the
    /// route that writes it.
    async fn create_trashed_endpoint(&self, name: &str, path: &str, state: &str) -> Uuid {
        let id = self.create_endpoint(name, path, state).await;
        sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await
            .expect("trash endpoint directly");
        id
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let c = &self.manager;
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

    /// `POST /endpoints` with a canonical GET path, for the conflict tests.
    async fn post_endpoint(&self, name: &str, path: &str) -> (StatusCode, serde_json::Value) {
        self.send(
            "POST",
            &self.collection(),
            Some(serde_json::json!({
                "name": name, "path": path,
                "database_id": self.database_id, "statements": [{ "name": "main", "sql": "SELECT 1 AS n" }]
            })),
        )
        .await
    }

    /// The error text of a refusal, whichever key the seam's error body used.
    fn error_text(body: &serde_json::Value) -> String {
        body["error"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or("")
            .to_string()
    }

    async fn deleted_at(&self, id: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
        sqlx::query_scalar("SELECT deleted_at FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("select endpoint")
    }

    async fn state_of(&self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT state FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("select endpoint state")
    }

    /// Everything emitted so far.
    fn drained(&mut self) -> Vec<Emitted> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AuditEvent::Mutation {
                    actor,
                    workspace_id,
                    action,
                    resource_type,
                    resource_id,
                    detail,
                    ..
                } => out.push(Emitted {
                    actor,
                    workspace_id,
                    action,
                    resource_type,
                    resource_id,
                    detail,
                }),
                other => panic!("unexpected audit event: {other:?}"),
            }
        }
        out
    }

    /// Exactly one event, or a failure naming what was actually emitted.
    fn one_event(&mut self) -> Emitted {
        let mut evs = self.drained();
        assert_eq!(evs.len(), 1, "expected exactly one audit event: {evs:?}");
        evs.remove(0)
    }
}

// ---------- The schema guarantee (migration 0126) ----------

/// The CHECK constraint added in migration 0126 must refuse a direct SQL
/// UPDATE that would soft-delete a `live` endpoint — bypassing every route,
/// because the guarantee has to hold against a writer that never read this
/// plan, not merely against the handlers this crate happens to expose today.
#[tokio::test]
async fn trashing_a_live_endpoint_is_refused_by_the_database() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue", "live").await;

    let err = sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&f.db.pool)
        .await
        .expect_err("the database must refuse to trash a live endpoint");

    let msg = err.to_string();
    assert!(
        msg.contains("dataapi_endpoints_trashed_not_live"),
        "expected the CHECK constraint to be the reason for the refusal, got: {msg}"
    );

    let deleted_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("select endpoint");
    assert_eq!(
        deleted_at, None,
        "a refused UPDATE must leave the row untouched, not half-applied"
    );
}

/// The same UPDATE against a non-`live` endpoint (here: `draft`) is exactly
/// what soft-delete needs to allow — this pins the constraint to `state =
/// 'live'` specifically, not to trashing in general.
#[tokio::test]
async fn trashing_a_draft_endpoint_is_allowed() {
    let f = skip_without_db!();
    let id = f.create_endpoint("cost", "/reports/cost", "draft").await;

    sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&f.db.pool)
        .await
        .expect("a draft endpoint must be trashable");

    let deleted_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .expect("select endpoint");
    assert!(deleted_at.is_some(), "deleted_at must have been written");
}

// ---------- The route: refusing a live endpoint ----------

/// **Behaviour 1.** A live endpoint is refused at the API, with a message naming
/// disable-first — not a 500 carrying `dataapi_endpoints_trashed_not_live`, which
/// is what the author would get if the route wrote first and let the CHECK answer.
///
/// Three assertions, each catching a different way of getting this wrong:
/// the STATUS (409, so a client can tell a refusal from a fault), the MESSAGE
/// (which must name the way out), and the ROW (untouched, so a route that
/// half-applied the write before noticing is caught).
#[tokio::test]
async fn trashing_a_live_endpoint_is_refused_by_the_route_before_it_writes() {
    let f = skip_without_db!();
    let id = f.create_endpoint("rev", "/reports/revenue", "live").await;

    let (status, body) = f
        .send("POST", &format!("{}/{id}/trash", f.collection()), None)
        .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a live endpoint must be refused as a conflict, not as a 500 from the CHECK: {body}"
    );
    let msg = Fixture::error_text(&body);
    assert!(
        msg.contains("disable"),
        "the refusal must name disable-first as the way forward: {body}"
    );
    assert!(
        !msg.contains("dataapi_endpoints_trashed_not_live"),
        "the author must not be shown a constraint name — that is the CHECK \
         answering, which means the route wrote first: {body}"
    );
    assert_eq!(
        f.deleted_at(id).await,
        None,
        "a refused trash must leave the row untouched"
    );
    assert_eq!(f.state_of(id).await, "live", "and must not restate it");
}

// ---------- The route: trashing what it may ----------

/// **Behaviour 2.** Both non-live states are trashable, and `deleted_at` lands in
/// the database as well as in the response body — a handler that returned a
/// plausible DTO without writing would pass a body-only assertion.
#[tokio::test]
async fn trashing_a_draft_or_disabled_endpoint_sets_deleted_at() {
    let f = skip_without_db!();
    for (state, path) in [("draft", "/reports/draft"), ("disabled", "/reports/off")] {
        let id = f.create_endpoint(state, path, state).await;

        let (status, body) = f
            .send("POST", &format!("{}/{id}/trash", f.collection()), None)
            .await;

        assert_eq!(status, StatusCode::OK, "{state}: {body}");
        assert!(
            body["deleted_at"].is_string(),
            "{state}: the response must carry the timestamp it wrote: {body}"
        );
        assert!(
            f.deleted_at(id).await.is_some(),
            "{state}: deleted_at must have been written to the row, not only returned"
        );
        // The state is not touched by trashing: what stops a trashed endpoint
        // serving is that it was already not `live`, and restore has to know what
        // it is bringing back.
        assert_eq!(
            f.state_of(id).await,
            state,
            "{state}: state must not change"
        );
    }
}

// ---------- The route: restore ----------

/// **Behaviour 3.** Restore lands on `disabled` whatever the endpoint was before,
/// and clears `deleted_at`.
///
/// `draft` is the interesting half: it proves the route SETS the state rather than
/// leaving it alone, so the mutation that makes restore write `live` — putting an
/// endpoint back on the internet from an undo button — has an assertion to fail.
#[tokio::test]
async fn restoring_a_trashed_endpoint_brings_it_back_disabled() {
    let f = skip_without_db!();
    for (was, path) in [
        ("draft", "/reports/back-draft"),
        ("disabled", "/reports/back-off"),
    ] {
        let id = f.create_trashed_endpoint(was, path, was).await;

        let (status, body) = f
            .send("POST", &format!("{}/{id}/restore", f.collection()), None)
            .await;

        assert_eq!(status, StatusCode::OK, "was {was}: {body}");
        assert_eq!(
            body["state"],
            serde_json::json!("disabled"),
            "restore must return the endpoint DISABLED, never live and never \
             straight back to whatever it was: {body}"
        );
        assert_eq!(
            body["deleted_at"],
            serde_json::Value::Null,
            "was {was}: a restored endpoint is not in the trash: {body}"
        );
        assert_eq!(f.state_of(id).await, "disabled", "was {was}");
        assert_eq!(f.deleted_at(id).await, None, "was {was}");
    }
}

/// Restore is not a generic state setter: applied to an endpoint that is not in the
/// trash it would take a LIVE one out of service, which is `disable`'s job and
/// `disable`'s audit event.
#[tokio::test]
async fn restoring_an_endpoint_that_is_not_in_the_trash_is_refused() {
    let f = skip_without_db!();
    let id = f.create_endpoint("live", "/reports/serving", "live").await;

    let (status, body) = f
        .send("POST", &format!("{}/{id}/restore", f.collection()), None)
        .await;

    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(
        f.state_of(id).await,
        "live",
        "a refused restore must not have taken a serving endpoint down"
    );
}

// ---------- The route: the list ----------

/// **Behaviour 4.** The list excludes trashed rows by default and returns ONLY
/// them when asked.
///
/// The fixture holds two endpoints, one trashed and one not, and both assertions
/// check for a present id AND an absent one. That is what separates "the filter
/// works" from "the list came back empty" and from "the filter is ignored" — a
/// handler that dropped the predicate would return both rows in both calls and
/// fail the absence half; one that returned nothing would fail the presence half.
#[tokio::test]
async fn the_list_excludes_trashed_endpoints_unless_asked_for_them() {
    let f = skip_without_db!();
    let kept = f.create_endpoint("kept", "/reports/kept", "draft").await;
    let binned = f
        .create_trashed_endpoint("binned", "/reports/binned", "draft")
        .await;

    let ids = |body: &serde_json::Value| -> Vec<String> {
        body.as_array()
            .expect("a list")
            .iter()
            .map(|e| e["id"].as_str().expect("id").to_string())
            .collect()
    };

    let (status, body) = f.send("GET", &f.collection(), None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let default = ids(&body);
    assert!(
        default.contains(&kept.to_string()),
        "the default list must still show untrashed endpoints: {body}"
    );
    assert!(
        !default.contains(&binned.to_string()),
        "the default list must not show trashed ones: {body}"
    );

    let (status, body) = f
        .send("GET", &format!("{}?trashed=true", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let trashed = ids(&body);
    assert!(
        trashed.contains(&binned.to_string()),
        "?trashed=true must show the trashed endpoint: {body}"
    );
    assert!(
        !trashed.contains(&kept.to_string()),
        "?trashed=true is the Trash screen, not the whole workspace: {body}"
    );

    // And the DTO carries the timestamp the Trash screen sorts by, on the trashed
    // row and not on the other one.
    let row = body
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == serde_json::json!(binned.to_string()))
        .expect("the trashed row");
    assert!(
        row["deleted_at"].is_string(),
        "the list DTO must expose deleted_at: {row}"
    );
}

// ---------- The route: the audit trail ----------

/// **Behaviour 5.** Both routes emit their own event, and the ACTION STRING and
/// RESOURCE ID are asserted rather than the event count.
///
/// A handler that emitted `data_api_endpoint:delete` from trash, or that stamped
/// the workspace id as the resource, emits exactly one event either way — so a
/// counting test would confirm a trail that names the wrong act on the wrong
/// object. The two events are also checked to be DIFFERENT actions: copying the
/// trash emit into restore is the plausible mistake, and it would leave the trail
/// showing an endpoint trashed twice and never restored.
#[tokio::test]
async fn trash_and_restore_each_emit_their_own_audit_event() {
    let mut f = skip_without_db!();
    let id = f
        .create_endpoint("audited", "/reports/audited", "draft")
        .await;

    let (status, body) = f
        .send("POST", &format!("{}/{id}/trash", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ev = f.one_event();
    assert_eq!(
        ev.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:trash",
            "data_api_endpoint",
            Some(id.to_string().as_str()),
        ),
        "trash must be recorded as itself, against the endpoint it trashed",
    );
    assert_eq!(
        ev.detail["path"],
        serde_json::json!("/reports/audited"),
        "the trail must name the URL that left the authoring surface: {:?}",
        ev.detail
    );

    let (status, body) = f
        .send("POST", &format!("{}/{id}/restore", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ev2 = f.one_event();
    assert_eq!(
        ev2.shape(),
        (
            Some(f.manager.user),
            Some(f.ws_id),
            "data_api_endpoint:restore",
            "data_api_endpoint",
            Some(id.to_string().as_str()),
        ),
        "restore is the other half of a reversible pair and must be told apart \
         from trash in the trail",
    );
    assert_ne!(
        ev.action, ev2.action,
        "the two halves must not share one action string"
    );
    assert_eq!(
        ev2.detail["state"],
        serde_json::json!("disabled"),
        "the state restore landed in is the decision this route makes: {:?}",
        ev2.detail
    );
}

// ---------- The route: a trashed endpoint keeps its URL ----------

/// **Behaviour 6.** `UNIQUE (workspace_id, method, path)` has no
/// `WHERE deleted_at IS NULL`, so a trashed endpoint keeps its URL and a fresh
/// create at that URL is still a 409 — which is what makes restore incapable of
/// failing with a conflict of its own. The message therefore has to name the real
/// cause, because the row it points at appears in no list the author is looking at.
///
/// **Both branches are driven at the SAME method and path.** `GET /reports/orders`
/// is blocked first by a live endpoint and then, after that one is gone, by a
/// trashed one. Two different paths would have produced two different strings
/// whether or not the branch existed, so `assert_ne!` would have passed on an
/// implementation that emitted one message for both cases — which is precisely the
/// defect this test exists to prevent.
#[tokio::test]
async fn a_trashed_endpoint_still_holds_its_url_and_the_conflict_says_so() {
    let f = skip_without_db!();
    const PATH: &str = "/reports/orders";

    // Branch A: a visible, live endpoint holds the slot.
    let live = f.create_endpoint("live-holder", PATH, "live").await;
    let (status, body) = f.post_endpoint("newcomer", PATH).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    let msg_live = Fixture::error_text(&body);
    assert!(
        msg_live.contains(PATH),
        "every 409 from this pair must name the colliding URL: {body}"
    );
    assert!(
        !msg_live.to_lowercase().contains("trash"),
        "a visible endpoint is not in the trash, and sending the author to an \
         empty Trash screen is the mirror image of the bug: {body}"
    );

    // Free the slot, then let a TRASHED endpoint hold it instead. Same method,
    // same path — so the only thing that can differ between the two messages is
    // the reason.
    sqlx::query("DELETE FROM dataapi.endpoints WHERE id = $1")
        .bind(live)
        .execute(&f.db.pool)
        .await
        .expect("free the slot");
    let binned = f
        .create_trashed_endpoint("binned-holder", PATH, "draft")
        .await;

    let (status, body) = f.post_endpoint("newcomer", PATH).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a trashed endpoint keeps its URL, so this is still a conflict: {body}"
    );
    let msg_trashed = Fixture::error_text(&body);
    assert!(
        msg_trashed.contains(PATH),
        "the trashed 409 must name the URL too: {body}"
    );
    assert_ne!(
        msg_live, msg_trashed,
        "the two causes must not share one message — a single string for both is \
         a dead end for the author who cannot see the row it names"
    );
    let lower = msg_trashed.to_lowercase();
    assert!(
        lower.contains("trash"),
        "the refusal must say the holder is in the trash: {body}"
    );
    assert!(
        lower.contains("restore") && lower.contains("delete"),
        "and must name both ways out — restore it, or delete it permanently: {body}"
    );

    // The advice has to be true: deleting the trashed holder frees the URL.
    let (status, body) = f
        .send("DELETE", &format!("{}/{binned}", f.collection()), None)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "DELETE must still reach a trashed endpoint — emptying the trash is that \
         call, and the 409's advice depends on it: {body}"
    );
    let (status, body) = f.post_endpoint("newcomer", PATH).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "with the trashed holder permanently deleted the URL must be free: {body}"
    );
}

/// Restoring is the OTHER way out named by that message, and it must not be able
/// to fail with a conflict — which is the whole reason the trashed endpoint keeps
/// its slot instead of releasing it.
#[tokio::test]
async fn restore_cannot_collide_because_the_slot_was_never_released() {
    let f = skip_without_db!();
    const PATH: &str = "/reports/held";
    let binned = f.create_trashed_endpoint("binned", PATH, "disabled").await;

    // Nothing could have taken the URL in the meantime: an attempt is refused.
    let (status, body) = f.post_endpoint("squatter", PATH).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the slot must still be held while the endpoint is in the trash: {body}"
    );

    let (status, body) = f
        .send(
            "POST",
            &format!("{}/{binned}/restore", f.collection()),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "restore must not be able to 409 on its own URL: {body}"
    );
    assert_eq!(body["path"], serde_json::json!(PATH), "body: {body}");
}

/// Emptying the trash is `DELETE …/endpoints/{id}` — it is what an engine delete's
/// "trashed Data API endpoint" dependent tells the author to do. It must take the
/// endpoint's children with it and leave its access log.
///
/// `compared_against` (0131) references `endpoint_versions` with NO `ON DELETE`
/// action, and its comment says nothing deletes versions. A cascade deletes the
/// referencing and referenced versions in one statement, which NO ACTION allows
/// — this pins that, because RESTRICT would not.
#[tokio::test]
async fn emptying_the_trash_takes_versions_and_alerts_and_keeps_the_access_log() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("purge-me", "/purge/me", "disabled")
        .await;
    let v1: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot) \
         VALUES ($1, 1, '{}'::jsonb) RETURNING id",
    )
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .expect("v1");
    let v2: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot, breaking_changes, compared_against) \
         VALUES ($1, 2, '{}'::jsonb, '[]'::jsonb, $2) RETURNING id",
    )
    .bind(id)
    .bind(v1)
    .fetch_one(&f.db.pool)
    .await
    .expect("v2 compared against v1");
    sqlx::query("UPDATE dataapi.endpoints SET live_version_id = $2 WHERE id = $1")
        .bind(id)
        .bind(v2)
        .execute(&f.db.pool)
        .await
        .expect("point at v2");
    let alert: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, validator_type, dataapi_metric, dataapi_window_s, dataapi_endpoint_id) \
         VALUES ($1, 'dataapi_alert', 'purge-watch', '0 9 * * *', 'operator', 'call_count', 3600, $2) \
         RETURNING id",
    )
    .bind(f.ws_id)
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .expect("dataapi alert");
    let request_id = format!("purge-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO dataapi.access_log (workspace_id, endpoint_id, status_code, duration_ms, request_id) \
         VALUES ($1, $2, 200, 5, $3)",
    )
    .bind(f.ws_id)
    .bind(id)
    .bind(&request_id)
    .execute(&f.db.pool)
    .await
    .expect("access log row");

    let (status, body) = f
        .send("DELETE", &format!("{}/{id}", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let count = |sql: &'static str, bind: Uuid| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(bind)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        count("SELECT count(*) FROM dataapi.endpoints WHERE id = $1", id).await,
        0
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM dataapi.endpoint_versions WHERE endpoint_id = $1",
            id
        )
        .await,
        0,
        "both versions, compared_against chain included"
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM automation.schedules WHERE id = $1",
            alert
        )
        .await,
        0
    );
    let log_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE request_id = $1")
            .bind(&request_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(
        log_rows, 1,
        "the access log outlives the endpoint by design (0123)"
    );
}

// ---------- The trash boundary against the state machine (task 3) ----------
//
// `crates/egret-dataapi/src/lifecycle.rs` contained ZERO occurrences of
// `deleted_at` before this task: the trash boundary and the state machine did not
// know about each other. Six writers could reach a trashed row, and they failed in
// two very different ways:
//
// * **Loudly.** `publish` on a trashed DRAFT and `enable` on a trashed endpoint
//   both try to write `state = 'live'`, which migration 0126's
//   `CHECK (deleted_at IS NULL OR state <> 'live')` refuses. That is the design
//   working — but it reaches the author as a 500 naming
//   `dataapi_endpoints_trashed_not_live`, which is not a refusal: it leaks a
//   database error and names no way forward.
// * **Silently.** `publish` on a trashed DISABLED endpoint, `disable`, `rollback`
//   and `PATCH` all leave `state` alone or write a non-`live` value, so the CHECK
//   stays satisfied and every one of them SUCCEEDS. They mutate a row that appears
//   on no list the author is looking at.
//
// **The falsifiability trap is specific.** Asserting only "it returned an error"
// would pass against the 500 the first two produced before the guards existed, and
// would prove nothing at all against the four that returned 200. So every test
// below asserts three things: the status IS 409, the body does NOT carry the
// constraint name (which is the CHECK answering, i.e. the route wrote first), and
// the message names RESTORE as the way forward. Each also asserts the row was left
// alone, because a guard that refuses after writing is the mirror-image defect.

impl Fixture {
    /// A published version of `id`, with `live_version_id` pointed at it.
    ///
    /// Written straight to the table rather than through `POST /publish`, because
    /// the endpoint under test is about to be TRASHED and the guards this file
    /// measures would refuse the publish. The snapshot is a bare `{}` — nothing
    /// here deserialises it; `enable` and `rollback` read only the pointer.
    /// # The snapshot is a REAL one, built from the endpoint row
    ///
    /// It used to be `'{}'::jsonb`, which no production path can produce —
    /// `publish` is the only writer of this column — and which nothing read,
    /// because publish's only look at this table was `MAX(version)` inside its
    /// own INSERT. Package 6 makes publish read the LIVE version's snapshot to
    /// diff against, so `{}` now fails to deserialize and the publish 500s. That
    /// is the correct behaviour for corrupt data (`EndpointStore::versions` has
    /// answered the same way since P3, and quietly skipping the diff instead
    /// would mean an unreadable snapshot silently disables the breaking-change
    /// gate) — so the fix is the fixture, not the rule. It is pinned from the
    /// other side by `publish_refuses_when_the_live_snapshot_cannot_be_read` in
    /// `tests/lifecycle.rs`.
    ///
    /// Built from `dataapi.endpoints` rather than hard-coded so it stays the
    /// endpoint's OWN contract: a snapshot that disagreed with the draft would
    /// make the publish below a breaking change and a 409, which is a second way
    /// for this test to fail for a reason that has nothing to do with the trash.
    /// The columns are what `OkPort` describes.
    async fn add_version(&self, id: Uuid, version: i32) -> Uuid {
        let vid: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.endpoint_versions \
                 (endpoint_id, version, snapshot, note, created_by) \
             SELECT e.id, $2, jsonb_build_object( \
                      'statements', jsonb_build_array(jsonb_build_object( \
                          'name', 'main', 'sql', 'SELECT 1', \
                          'columns', jsonb_build_array(jsonb_build_object( \
                              'name', 'n', 'ty', 'integer', 'required', true)))), \
                      'method', e.method, \
                      'path', e.path, \
                      'database_id', e.database_id, \
                      'params', e.params, \
                      'output', e.output, \
                      'cache', e.cache, \
                      'limits', e.limits, \
                      'rate_limit', e.rate_limit, \
                      'delegated', false), \
                    '', $3 \
               FROM dataapi.endpoints e WHERE e.id = $1 \
             RETURNING id",
        )
        .bind(id)
        .bind(version)
        .bind(self.admin)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert endpoint version");
        sqlx::query("UPDATE dataapi.endpoints SET live_version_id = $2 WHERE id = $1")
            .bind(id)
            .bind(vid)
            .execute(&self.db.pool)
            .await
            .expect("point live_version_id");
        vid
    }

    async fn live_version_of(&self, id: Uuid) -> Option<Uuid> {
        sqlx::query_scalar("SELECT live_version_id FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("select live_version_id")
    }

    async fn version_count(&self, id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM dataapi.endpoint_versions WHERE endpoint_id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("count versions")
    }

    async fn name_of(&self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT name FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("select name")
    }

    /// The three assertions every trash-boundary refusal owes the author.
    ///
    /// A helper rather than six copies so no test can quietly settle for two of
    /// them — and it takes the STATUS, not a bool, so the failure message names
    /// what actually came back (a 500 from the CHECK and a 200 from an unguarded
    /// write are the two things that were happening, and they must be told apart
    /// in the output).
    fn assert_refused_naming_restore(what: &str, status: StatusCode, body: &serde_json::Value) {
        // One status assertion, not two. An `assert_ne!(status, 500)` used to sit
        // under this and could never fail: 500 != 409, so the equality above has
        // already returned by the time it is reached. It advertised a
        // falsifiability it did not supply — the 500 case is real, and it is this
        // assertion's message that has to name it.
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "{what}: a trashed endpoint must be refused as a 409 conflict. \
             A 500 means the CHECK constraint answered after the route wrote; \
             a 2xx means nothing refused it at all. Got {status}: {body}"
        );
        let msg = Fixture::error_text(body);
        assert!(
            !msg.contains("dataapi_endpoints_trashed_not_live"),
            "{what}: the author must not be shown a constraint name — that is the \
             CHECK answering, which means the route wrote first: {body}"
        );
        let lower = msg.to_lowercase();
        assert!(
            lower.contains("trash"),
            "{what}: the refusal must say the endpoint is in the trash: {body}"
        );
        assert!(
            lower.contains("restore"),
            "{what}: the refusal must name RESTORE as the way forward — that is the \
             whole difference between this and the 500 it replaces: {body}"
        );
    }
}

/// **`publish` on a trashed DRAFT.** The loud half: `state = CASE WHEN state =
/// 'disabled' THEN 'disabled' ELSE 'live' END` moves a draft to `live`, which
/// migration 0126's CHECK refuses — so before the guard this was a 500 naming the
/// constraint.
///
/// The version count is asserted at zero afterwards, because `publish` INSERTs the
/// frozen version before it updates the endpoint: a guard that lived only in the
/// UPDATE and did not roll the transaction back would leave an immutable published
/// version behind for an endpoint that was refused.
#[tokio::test]
async fn publishing_a_trashed_draft_endpoint_is_refused() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-draft", "/reports/pub-draft", "draft")
        .await;
    let before = f.deleted_at(id).await;

    let (status, body) = f
        .send(
            "POST",
            &format!("{}/{id}/publish", f.collection()),
            Some(serde_json::json!({})),
        )
        .await;

    Fixture::assert_refused_naming_restore("publish a trashed draft", status, &body);
    assert_eq!(f.state_of(id).await, "draft", "the row must be left alone");
    assert_eq!(f.deleted_at(id).await, before, "and must stay in the trash");
    assert_eq!(
        f.version_count(id).await,
        0,
        "a refused publish must not leave a frozen version behind"
    );
    assert_eq!(f.live_version_of(id).await, None);
}

/// **`publish` on a trashed DISABLED endpoint — the asymmetric case, and the one
/// that failed SILENTLY.**
///
/// The `CASE` leaves a `disabled` endpoint disabled, so the CHECK stays satisfied
/// and before the guard this returned **200**: it froze a new immutable version
/// into the trash and told the author it had published. No 500, no refusal, and
/// nothing on any list to show what happened. A test asserting merely "an error"
/// would have passed on the sibling above and failed here, which is exactly why
/// this case needs its own test rather than a loop over both states.
#[tokio::test]
async fn publishing_a_trashed_disabled_endpoint_is_refused() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-off", "/reports/pub-off", "disabled")
        .await;

    let (status, body) = f
        .send(
            "POST",
            &format!("{}/{id}/publish", f.collection()),
            Some(serde_json::json!({})),
        )
        .await;

    Fixture::assert_refused_naming_restore("publish a trashed disabled endpoint", status, &body);
    assert_eq!(f.state_of(id).await, "disabled");
    assert!(f.deleted_at(id).await.is_some(), "still in the trash");
    assert_eq!(
        f.version_count(id).await,
        0,
        "publishing into the trash must not have frozen a version"
    );
    assert_eq!(
        f.live_version_of(id).await,
        None,
        "and must not have repointed what the endpoint would serve"
    );
}

/// **`enable` on a trashed endpoint — the danger this whole task exists for.**
///
/// `enable` calls `set_state(.., "live")` after checking only that
/// `live_version_id` is set. Without migration 0126's CHECK this would have put a
/// trashed row into `live`, and `live_snapshot` — which filters `state = 'live'`
/// and nothing else — would have served it. The CHECK turns that into a 500; the
/// guard turns the 500 into an answer.
///
/// The endpoint is set up PUBLISHED and disabled, which is the only shape that
/// reaches the `set_state` call at all: a never-published trashed endpoint is
/// refused earlier by `enable`'s own `live_version_id` check, so it would have
/// measured that guard instead of this one.
#[tokio::test]
async fn enabling_a_trashed_endpoint_is_refused() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-live", "/reports/en-off", "disabled")
        .await;
    let vid = f.add_version(id, 1).await;

    let (status, body) = f
        .send("POST", &format!("{}/{id}/enable", f.collection()), None)
        .await;

    Fixture::assert_refused_naming_restore("enable a trashed endpoint", status, &body);
    assert_eq!(
        f.state_of(id).await,
        "disabled",
        "a trashed endpoint must not be put back into service"
    );
    assert!(f.deleted_at(id).await.is_some(), "still in the trash");
    assert_eq!(f.live_version_of(id).await, Some(vid));
}

/// **`disable` on a trashed endpoint.** Silent before the guard: `state =
/// 'disabled'` satisfies the CHECK, so a trashed DRAFT was quietly restated to
/// `disabled` and answered 200.
///
/// It cannot resurrect serving, so this is the mildest of the six — but it
/// rewrites what `restore` will bring back, and it is a mutation of a row that
/// appears on no list the author is looking at. `draft` is the interesting state
/// precisely because the write is observable: on an already-`disabled` endpoint an
/// unguarded `disable` is indistinguishable from a no-op.
#[tokio::test]
async fn disabling_a_trashed_endpoint_is_refused() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-dis", "/reports/dis", "draft")
        .await;

    let (status, body) = f
        .send("POST", &format!("{}/{id}/disable", f.collection()), None)
        .await;

    Fixture::assert_refused_naming_restore("disable a trashed endpoint", status, &body);
    assert_eq!(
        f.state_of(id).await,
        "draft",
        "a refused disable must not have restated a trashed draft — that is what \
         restore would then bring back"
    );
    assert!(f.deleted_at(id).await.is_some(), "still in the trash");
}

/// **`rollback` on a trashed endpoint.** Silent before the guard: rollback writes
/// `live_version_id` and deliberately leaves `state` alone, so the CHECK never
/// fires and it answered 200 — repointing which frozen SQL a trashed endpoint
/// would serve the moment anything enabled it.
///
/// Two versions exist so the write is observable: with one version a rollback to
/// it is indistinguishable from a no-op, and the assertion would hold whether the
/// guard existed or not.
#[tokio::test]
async fn rolling_back_a_trashed_endpoint_is_refused() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-rb", "/reports/rb", "disabled")
        .await;
    let v1 = f.add_version(id, 1).await;
    let v2 = f.add_version(id, 2).await;
    assert_eq!(
        f.live_version_of(id).await,
        Some(v2),
        "fixture precondition"
    );

    let (status, body) = f
        .send(
            "POST",
            &format!("{}/{id}/rollback", f.collection()),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await;

    Fixture::assert_refused_naming_restore("roll a trashed endpoint back", status, &body);
    assert_eq!(
        f.live_version_of(id).await,
        Some(v2),
        "a refused rollback must not have repointed the live version"
    );
    assert_ne!(f.live_version_of(id).await, Some(v1));
    assert!(f.deleted_at(id).await.is_some(), "still in the trash");
}

/// **`PATCH` on a trashed endpoint.** Silent before the guard: the UPDATE touches
/// neither `state` nor `live_version_id`, so it answered 200 and edited a row the
/// author can only see on the Trash screen.
///
/// The concrete harm is not hypothetical. A trashed endpoint KEEPS its URL —
/// `UNIQUE (workspace_id, method, path)` has no `WHERE deleted_at IS NULL`, which
/// is what [`restore_cannot_collide_because_the_slot_was_never_released`] pins — so
/// a PATCH of `path` on a trashed row silently changes WHICH URL is held hostage,
/// and the 409 another author then gets points at a row they cannot see.
#[tokio::test]
async fn editing_a_trashed_endpoint_is_refused() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-edit", "/reports/edit", "draft")
        .await;

    let (status, body) = f
        .send(
            "PATCH",
            &format!("{}/{id}", f.collection()),
            Some(serde_json::json!({ "name": "renamed-in-the-trash" })),
        )
        .await;

    Fixture::assert_refused_naming_restore("edit a trashed endpoint", status, &body);
    assert_eq!(
        f.name_of(id).await,
        "binned-edit",
        "a refused edit must not have landed"
    );
    assert!(f.deleted_at(id).await.is_some(), "still in the trash");
}

/// The other half of every test above: the SAME six calls against the SAME
/// endpoints, once restored, must all succeed.
///
/// Without this, a guard could refuse everything unconditionally — `deleted_at IS
/// NOT NULL` mistyped as a tautology, or the predicate dropped into the wrong
/// clause — and all six refusal tests would still pass. This is what makes them
/// tests of the trash boundary rather than tests that the routes are broken.
#[tokio::test]
async fn every_refused_transition_succeeds_once_the_endpoint_is_restored() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("binned-all", "/reports/all", "disabled")
        .await;
    let v1 = f.add_version(id, 1).await;
    let _v2 = f.add_version(id, 2).await;

    let (status, body) = f
        .send("POST", &format!("{}/{id}/restore", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "restore: {body}");

    // PATCH.
    let (status, body) = f
        .send(
            "PATCH",
            &format!("{}/{id}", f.collection()),
            Some(serde_json::json!({ "name": "renamed" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch after restore: {body}");
    assert_eq!(f.name_of(id).await, "renamed");

    // rollback.
    let (status, body) = f
        .send(
            "POST",
            &format!("{}/{id}/rollback", f.collection()),
            Some(serde_json::json!({ "version": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "rollback after restore: {body}");
    assert_eq!(f.live_version_of(id).await, Some(v1));

    // enable, then disable — the pair that writes `state`.
    let (status, body) = f
        .send("POST", &format!("{}/{id}/enable", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "enable after restore: {body}");
    assert_eq!(f.state_of(id).await, "live");
    let (status, body) = f
        .send("POST", &format!("{}/{id}/disable", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "disable after restore: {body}");
    assert_eq!(f.state_of(id).await, "disabled");

    // publish. Last, because it freezes a NEW version and the rollback assertion
    // above would then be reading a moved target.
    let (status, body) = f
        .send(
            "POST",
            &format!("{}/{id}/publish", f.collection()),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "publish after restore: {body}");
    assert_eq!(
        f.version_count(id).await,
        3,
        "publish after restore must have frozen a third version: {body}"
    );
}
