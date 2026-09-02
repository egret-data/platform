//! Shared helpers for `egret-workflow`'s DB-gated integration tests.
//!
//! Follows `crates/egret-query/tests/registry_repo.rs` (`repo_with_db`,
//! `seed_workspace`) rather than inventing a new shape.
#![allow(dead_code)]

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

/// Connect to the database named by `DATABASE_URL`, migrated, and ready.
///
/// `None` (self-skip) only when `egret_test_db::url()` says so -- no
/// `DATABASE_URL` and `EGRET_ALLOW_DB_SKIP` is set. Any other absence PANICS
/// (that crate's contract), so this suite cannot silently report `ok` having
/// asserted nothing.
pub async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// Seed a fresh user + workspace, returning `(workspace_id, user_id)` --
/// enough for every `FlowRepo` call in this crate's tests, which all take a
/// workspace and a `by: UserId`.
///
/// **The workspace's `slug` is set to its own `id`, stringified.** Every
/// existing caller (`tests/repo.rs`, `tests/publish.rs`) only ever needed a
/// real row to satisfy a foreign key and used the returned `Uuid` directly,
/// never a slug -- but `tests/routes.rs` (Task 7) drives `WsContext`, which
/// resolves the `{workspace}` PATH SEGMENT as a slug
/// (`WorkspaceRepo::get_by_slug`), not an id. Task 7's brief itself
/// interpolates this function's returned `ws` straight into a URL
/// (`format!("/api/ws/{ws}/workflows/flows")`), so making the slug equal the
/// id's own string form is what makes that literal, required test -- and
/// every other HTTP test in this crate -- resolve at all, without having to
/// widen this helper's return type for the two existing DB-only callers that
/// never look at a slug.
pub async fn seed_workspace(db: &Db) -> (Uuid, UserId) {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed user");

    let wid = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 't', $3)")
        .bind(wid)
        .bind(wid.to_string())
        .bind(uid)
        .execute(&db.pool)
        .await
        .expect("seed workspace");

    (wid, UserId(uid))
}

/// Grant `user` a fresh workspace-scoped role holding exactly `perms` in
/// `ws`. Mirrors `crates/egret-dataapi/tests/endpoints_routes.rs`'s
/// `principal_with` -- a NEW role per call, so one test's grant can never
/// leak into another's assertion about what a bare `ws_admin`/`ws_editor`
/// grants by default.
pub async fn grant(db: &Db, ws: Uuid, user: UserId, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) \
         VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_workflow_{}", Uuid::new_v4().simple()))
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
    .bind(user.0)
    .bind(role)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant role");
}

/// Turn `feature` OFF for `ws` -- the row the platform-admin feature-toggle
/// API would write. An absent row means enabled, so this is the only way to
/// say "off"; mirrors `crates/egret-notebook/tests/feature_gate.rs`'s
/// `disable`.
pub async fn disable_feature(db: &Db, ws: Uuid, feature: &str) {
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, $2, false)",
    )
    .bind(ws)
    .bind(feature)
    .execute(&db.pool)
    .await
    .expect("disable feature");
}

/// The composed HTTP app under test: `egret_workflow::management_router()`
/// nested at `/api` exactly as `egret-server` mounts it (`build_router`), so
/// `WsContext`'s feature gate sees the same `/api/ws/...` shape production
/// does. Holds `sessions` alongside the router because [`get`]/[`post`] mint
/// a session per call rather than taking a token -- the caller of this
/// module never has to construct one by hand.
pub struct App {
    router: axum::Router,
    sessions: egret_auth::session::SessionService,
}

/// Build [`App`] against a REAL `CedarPdp` (never a stub) -- an entitlement
/// or permission test against a mocked decision point would pin the
/// consumer, not the server, per this repo's own convention
/// (`crates/egret-dataapi/tests/endpoints_routes.rs`'s module doc: "driven
/// from the caller's side against the REAL Cedar PDP").
///
/// A fresh `WorkflowRefs` is enough here: no test in this file publishes a
/// flow that names an object outside this crate, so its three ports never
/// have to answer anything.
pub async fn app(db: &Db) -> App {
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: std::sync::Arc<dyn egret_core::authz::Pdp> = std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let state = egret_workflow::WorkflowState {
        repo: egret_workflow::FlowRepo::new(db.clone()),
        refs: std::sync::Arc::new(egret_workflow::WorkflowRefs {
            flows: egret_workflow::FlowRepo::new(db.clone()),
            dataapi_endpoints: std::sync::Arc::new(UnreachableDataApi),
            notebooks: std::sync::Arc::new(UnreachableNotebooks),
        }),
        sessions: sessions.clone(),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
    };
    let router = axum::Router::new()
        .nest("/api", egret_workflow::management_router())
        .with_state(state);
    App { router, sessions }
}

/// A port that must never be consulted -- every reference-resolving test in
/// this file goes through `tests/publish.rs`'s `FakeRefs` instead, so a
/// route test that reaches either of these ports is a bug (the wrong flow
/// resolved, or a route calling `publish` when it should not).
struct UnreachableDataApi;
#[async_trait::async_trait]
impl egret_dataapi_api::DataApiEndpointPort for UnreachableDataApi {
    async fn workspace_of(&self, _id: Uuid) -> egret_core::Result<Option<Uuid>> {
        unreachable!("tests/routes.rs must not publish a flow with a dataapi.call task")
    }
}
struct UnreachableNotebooks;
#[async_trait::async_trait]
impl egret_analytics_api::NotebookDocsPort for UnreachableNotebooks {
    async fn get_summary(
        &self,
        _id: Uuid,
    ) -> egret_core::Result<Option<(egret_analytics_api::NotebookSummary, Uuid)>> {
        unreachable!("tests/routes.rs must not publish a flow with an egret.python task")
    }
    async fn list_engine_options(
        &self,
        _ws: Uuid,
    ) -> egret_core::Result<Vec<egret_analytics_api::EngineOption>> {
        unreachable!()
    }
    async fn record_output(
        &self,
        _nb: Uuid,
        _cid: Uuid,
        _outputs: &[egret_analytics_api::CellOutput],
        _execution_count: Option<i32>,
        _elapsed_ms: Option<i32>,
    ) -> egret_core::Result<()> {
        unreachable!()
    }
    async fn build_runtime(
        &self,
        _ws: Uuid,
        _user_id: UserId,
    ) -> egret_core::Result<egret_analytics_api::NotebookRuntime> {
        unreachable!()
    }
}

async fn send(
    app: &App,
    method: &str,
    path: &str,
    user: UserId,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    use tower::ServiceExt;
    let token = app
        .sessions
        .create(user, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    let mut b = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("Cookie", format!("egret_session={token}"));
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    app.router
        .clone()
        .oneshot(
            b.body(
                body.map(|v| axum::body::Body::from(v.to_string()))
                    .unwrap_or_else(axum::body::Body::empty),
            )
            .unwrap(),
        )
        .await
        .unwrap()
}

pub async fn get(app: &App, path: &str, user: UserId) -> axum::response::Response {
    send(app, "GET", path, user, None).await
}

pub async fn post(
    app: &App,
    path: &str,
    user: UserId,
    body: serde_json::Value,
) -> axum::response::Response {
    send(app, "POST", path, user, Some(body)).await
}

pub async fn put(
    app: &App,
    path: &str,
    user: UserId,
    body: serde_json::Value,
) -> axum::response::Response {
    send(app, "PUT", path, user, Some(body)).await
}

pub async fn delete(app: &App, path: &str, user: UserId) -> axum::response::Response {
    send(app, "DELETE", path, user, None).await
}

pub async fn json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// Parse `source` and create a draft through `FlowRepo` -- shared by
/// `tests/publish.rs`, which needs a draft row to publish in every test.
///
/// `namespace`/`flow_id` come from the parsed flow itself (the same fields
/// `FlowRepo::create`'s caller is expected to supply, per `repo.rs`'s own
/// doc), not hard-coded here, so a test's YAML is the single source of truth
/// for what gets stored.
pub async fn draft(
    repo: &egret_workflow::FlowRepo,
    ws: Uuid,
    user: UserId,
    source: &str,
) -> egret_workflow::FlowRow {
    let flow = egret_workflow::spec::parse(source).expect("parse draft source");
    let plan = serde_json::to_value(&flow).expect("serialize draft plan");
    repo.create(
        ws,
        user,
        &egret_workflow::CreateFlowInput {
            namespace: flow.namespace.clone(),
            flow_id: flow.id.clone(),
            source: source.to_string(),
            plan,
            folder_id: None,
        },
    )
    .await
    .expect("create draft")
}
