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
/// API would write. An absent row means the key's `default_enabled` (ON for
/// every key but `workflow_containers`), so this is the only way to say "off"
/// for those.
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

/// Turn `feature` ON for `ws` with an explicit row -- the only way to turn on
/// a feature whose `default_enabled` is false (`workflow_containers`, W5a).
pub async fn enable_feature(db: &Db, ws: Uuid, feature: &str) {
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, $2, true) \
         ON CONFLICT (workspace_id, feature) DO UPDATE SET enabled = true",
    )
    .bind(ws)
    .bind(feature)
    .execute(&db.pool)
    .await
    .expect("enable feature");
}

/// A real `analytics.folders` row, for the folder-scoping tests
/// (whole-branch review, critical finding 1). `entity_type` is a parameter
/// because the wrong type is one of the two ways a folder id can be
/// illegitimate, and the guard has to refuse both -- mirrors
/// `crates/egret-dataapi/tests/endpoints_routes.rs`'s identical helper.
pub async fn create_folder(db: &Db, ws: Uuid, owner: UserId, entity_type: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO analytics.folders (workspace_id, entity_type, name, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(ws)
    .bind(entity_type)
    .bind(format!("f-{}", Uuid::new_v4().simple()))
    .bind(owner.0)
    .fetch_one(&db.pool)
    .await
    .expect("insert folder")
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
    /// **The audit sink's receiving end, kept rather than dropped.**
    ///
    /// It used to be dropped on the line that built the sink
    /// (`AuditSink::new(16).0`), and W10 Task 5's mutation **M11** is what that
    /// cost: replacing a route's entire audit payload with `{"mutated": …}`
    /// left the suite at `test result: ok. 24 passed; 0 failed` -- byte-identical
    /// to the control. Every `audit.emit` in this crate was unasserted.
    ///
    /// It is load-bearing for one claim in particular: `replay_execution`'s doc
    /// says the relationship between a manual replay and what it replays *"is in
    /// the audit trail and nowhere in the data"*. With nothing reading the
    /// trail, that sentence was unfalsifiable.
    ///
    /// `Mutex` because [`audit_mutations`] takes `&App` -- every other helper
    /// here does, and threading `&mut` through them to serve one assertion is
    /// the tail wagging the dog.
    audit: std::sync::Mutex<tokio::sync::mpsc::Receiver<egret_audit::AuditEvent>>,
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
    app_with_scripts(db, egret_workflow::scripts::ScriptSettings::default()).await
}

/// [`app`], with the platform half of the W5a script-task gate
/// (`plugin_configurations`, the default runner) set to `scripts`.
pub async fn app_with_scripts(db: &Db, scripts: egret_workflow::scripts::ScriptSettings) -> App {
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
    let (audit_sink, audit_rx) = egret_audit::AuditSink::new(16);
    let state = egret_workflow::WorkflowState {
        db: db.clone(),
        repo: egret_workflow::FlowRepo::new(db.clone()),
        executions: egret_workflow::ExecutionRepo::new(db.clone()),
        logs: egret_workflow::logs::LogRepo::new(db.pool.clone()),
        refs: std::sync::Arc::new(egret_workflow::WorkflowRefs {
            flows: egret_workflow::FlowRepo::new(db.clone()),
            dataapi_endpoints: std::sync::Arc::new(UnreachableDataApi),
        }),
        sessions: sessions.clone(),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        audit: audit_sink,
        scripts,
    };
    let router = axum::Router::new()
        .nest("/api", egret_workflow::management_router())
        .with_state(state);
    App {
        router,
        sessions,
        audit: std::sync::Mutex::new(audit_rx),
    }
}

/// The `(action, resource_id, detail)` of every audit mutation emitted since the
/// last call, drained.
///
/// `try_recv` rather than `recv().await`: `AuditSink::emit` is synchronous and
/// non-blocking (`try_send`), so by the time an HTTP response has been awaited
/// the event is already in the channel -- and a helper that *awaited* would hang
/// for ever on the assertion that matters most, "this route emitted NOTHING".
///
/// The sink is built with capacity 16 and `emit` DROPS on a full channel with
/// only a `warn`, so a test that drives more than 16 mutations without draining
/// sees silent loss. Every caller here drains after a handful; a test that needs
/// more must drain as it goes.
pub fn audit_mutations(app: &App) -> Vec<(String, Option<String>, serde_json::Value)> {
    let mut rx = app.audit.lock().expect("audit receiver");
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            egret_audit::AuditEvent::Mutation {
                action,
                resource_id,
                detail,
                ..
            } => out.push((action, resource_id, detail)),
            other => panic!("unexpected audit event shape: {other:?}"),
        }
    }
    out
}

/// A `WorkflowRefs` whose data-API port must never be asked -- for a direct
/// `publish` of a flow that does not name one.
pub fn workflow_refs(db: &Db) -> egret_workflow::WorkflowRefs {
    egret_workflow::WorkflowRefs {
        flows: egret_workflow::FlowRepo::new(db.clone()),
        dataapi_endpoints: std::sync::Arc::new(UnreachableDataApi),
    }
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

/// A request with a session for `user`, a `Content-Type` of the caller's
/// choosing and a raw body -- for the multipart start route (W9a Task 7),
/// which [`post`]'s JSON body cannot express. An empty `content_type`
/// sends none.
pub async fn send_raw(
    app: &App,
    method: &str,
    path: &str,
    user: UserId,
    content_type: &str,
    body: Vec<u8>,
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
    // An empty `content_type` sends no header at all.
    if !content_type.is_empty() {
        b = b.header("Content-Type", content_type);
    }
    app.router
        .clone()
        .oneshot(b.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap()
}

pub async fn get(app: &App, path: &str, user: UserId) -> axum::response::Response {
    send(app, "GET", path, user, None).await
}

/// A request to the **authenticated** app carrying **no session cookie at
/// all** — the 401 case.
///
/// Separate from [`get`] rather than an `Option<UserId>` parameter on it,
/// because every other caller in this crate wants a session minted for it and
/// an `Option` would put a `Some(user)` on ~40 call sites to serve the three
/// tests that want `None`. [`anon`] is the same idea for the *webhook* app,
/// which is a different router; this one deliberately hits the `/api` mount,
/// where `Auth` is supposed to refuse before `WsContext` resolves anything.
pub async fn anon_get(app: &App, path: &str) -> axum::response::Response {
    use tower::ServiceExt;
    app.router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

pub async fn post(
    app: &App,
    path: &str,
    user: UserId,
    body: serde_json::Value,
) -> axum::response::Response {
    send(app, "POST", path, user, Some(body)).await
}

/// `POST` with **no request body at all** — not an empty JSON object.
///
/// A separate helper rather than `post(app, path, user, json!(null))`, because
/// the two are different requests on the wire and exactly one route cares:
/// `POST .../flows/{id}/executions` takes `body: Option<Json<..>>`, so "no body"
/// and "a body that says `null`" reach different arms of its extractor. A test
/// that could only send one of them could not tell a handler that requires a
/// body from one that does not.
pub async fn post_no_body(app: &App, path: &str, user: UserId) -> axum::response::Response {
    send(app, "POST", path, user, None).await
}

/// `POST` to the authenticated app carrying **no session cookie** — the 401
/// case for a write route. [`anon_get`]'s sibling; see its doc.
pub async fn anon_post(app: &App, path: &str) -> axum::response::Response {
    use tower::ServiceExt;
    app.router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
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

// ---------------------------------------------------------------------------
// The webhook surface (W6 Task 7)
// ---------------------------------------------------------------------------

/// The **anonymous** webhook app: `egret_workflow::webhook_router()` nested at
/// `/workflows/v1`, exactly as `egret-server::build_router` mounts it.
///
/// A separate builder from [`app`] rather than a second route on it, because
/// the two differ in the thing under test: [`app`] nests under `/api` so
/// `WsContext`'s gate sees the shape it needs, and this one deliberately does
/// not, because there is no session for that gate to run in. Mounting the
/// webhook router under `/api` in a test would prove the opposite of what this
/// suite claims.
///
/// It carries no `SessionService` handle because nothing here mints a session:
/// [`anon`] sends the request with no cookie at all, which is the whole point.
pub struct WebhookApp {
    router: axum::Router,
}

pub async fn webhook_app(db: &Db) -> WebhookApp {
    webhook_app_with_guard(
        db,
        std::sync::Arc::new(egret_workflow::WebhookGuard::new(
            egret_workflow::WEBHOOK_MAX_CONCURRENCY,
        )),
    )
    .await
}

/// [`webhook_app`] with the guard supplied, so a test can drive a bound it
/// genuinely exceeds.
///
/// Fix round 1's sweep: without this the bulkhead had a unit test of the
/// SEMAPHORE and nothing reaching the handler's use of it — deleting the
/// `try_acquire` from `handle` passed the whole suite.
pub async fn webhook_app_with_guard(
    db: &Db,
    guard: std::sync::Arc<egret_workflow::WebhookGuard>,
) -> WebhookApp {
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
        db: db.clone(),
        repo: egret_workflow::FlowRepo::new(db.clone()),
        executions: egret_workflow::ExecutionRepo::new(db.clone()),
        logs: egret_workflow::logs::LogRepo::new(db.pool.clone()),
        refs: std::sync::Arc::new(egret_workflow::WorkflowRefs {
            flows: egret_workflow::FlowRepo::new(db.clone()),
            dataapi_endpoints: std::sync::Arc::new(UnreachableDataApi),
        }),
        sessions,
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
        scripts: Default::default(),
    };
    WebhookApp {
        router: axum::Router::new()
            .nest(
                "/workflows/v1",
                egret_workflow::webhook_router_with_guard(guard),
            )
            .with_state(state),
    }
}

/// Send a request carrying **no credential of any kind** -- no cookie, no
/// bearer, nothing but the URL and the body.
///
/// That is what a webhook caller is, and it is why every assertion in
/// `tests/webhook_route.rs` is meaningful: there is nothing for the server to
/// authenticate except the key in the path.
pub async fn anon(
    app: &WebhookApp,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> axum::response::Response {
    use tower::ServiceExt;
    let mut b = axum::http::Request::builder().method(method).uri(path);
    if let Some(ct) = content_type {
        b = b.header("Content-Type", ct);
    }
    app.router
        .clone()
        .oneshot(b.body(axum::body::Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
}

/// Insert a flow draft directly, so a webhook test never has to go through the
/// authenticated management router to set one up.
///
/// `state` is a parameter because `'disabled'` is one of the refusals under
/// test and `soft_delete` is another.
pub async fn seed_flow(db: &Db, ws: Uuid, owner: UserId, source: &str, state: &str) -> Uuid {
    let row = draft(
        &egret_workflow::FlowRepo::new(db.clone()),
        ws,
        owner,
        source,
    )
    .await;
    if state != "draft" {
        sqlx::query("UPDATE workflow.flows SET state = $2 WHERE id = $1")
            .bind(row.id)
            .bind(state)
            .execute(&db.pool)
            .await
            .expect("set flow state");
    }
    row.id
}

/// How many executions exist in `ws`. The assertion that actually matters for
/// a cross-workspace test: a 404 with a side-effect is the failure, and only a
/// count can see it.
pub async fn execution_count(db: &Db, ws: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .expect("count executions")
}

/// The stored `ExecutionTrigger` of the one execution in `ws`.
pub async fn only_execution_trigger(db: &Db, ws: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT execution_trigger FROM workflow.executions WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .expect("read execution_trigger")
}

pub async fn body_text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// The trigger loop (W12 Task 4; reused by Task 5's `tests/runtime.rs`)
// ---------------------------------------------------------------------------

/// Put a `*/1 * * * *` trigger so that **exactly `ticks` cron instants are
/// due**: `next_evaluation_date = date_trunc('minute', now()) - (ticks - 1) min`.
///
/// Truncated to the minute so the date is itself a cron instant, which is what
/// `tick` expects a stored date to be. `ticks - 1`, not `ticks`: the current
/// minute's own instant is already `<= now()`, so `- ticks` minutes would leave
/// `ticks + 1` instants due and "one tick behind" would fire twice.
///
/// Because "due" moves with the wall clock, a minute boundary crossed during a
/// test makes one more instant due. So this first waits out the last ten
/// seconds of a minute, giving every test that calls it at least that long
/// before the count it set up can change.
pub async fn make_due(db: &Db, flow: Uuid, trig: &str, ticks: i32) {
    assert!(ticks >= 1, "make_due is for due triggers");
    let sec: f64 = sqlx::query_scalar("SELECT extract(second FROM now())::float8")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    if sec >= 50.0 {
        tokio::time::sleep(std::time::Duration::from_secs_f64(60.5 - sec)).await;
    }
    let n = sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = \
           date_trunc('minute', now()) - make_interval(mins => $3 - 1) \
         WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .bind(ticks)
    .execute(&db.pool)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1, "make_due must land on exactly one trigger row");
}

/// Every `scheduled_date` of `flow`'s executions, ascending.
pub async fn scheduled_dates(db: &Db, flow: Uuid) -> Vec<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar(
        "SELECT scheduled_date FROM workflow.executions \
         WHERE flow_id = $1 ORDER BY scheduled_date",
    )
    .bind(flow)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// The Flow-trigger processor (W13a Task 4)
// ---------------------------------------------------------------------------

/// A registry that answers every task it is handed with one outcome, so a
/// test can finish a one-task execution in the state it wants through the
/// REAL worker tick (and so through the real `settle`, which is what writes
/// the outbox). `tests/runtime.rs`'s `Recorder` is the pattern.
pub struct Answering(pub bool);

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for Answering {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        Ok(if self.0 {
            egret_workflow::worker::Outcome::success()
        } else {
            egret_workflow::worker::Outcome::failed()
        })
    }
    async fn cancel(&self, task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        panic!(
            "no fixture here declares a timeout, so nothing kills {}",
            task.task_id
        );
    }
}

/// Finish `execution` in `state` (`"SUCCESS"` or `"FAILED"`) through the real
/// worker tick, answering every task with that outcome.
///
/// **`tick` is global**: it claims every runnable run in the database, not
/// only this execution's. Callers clear the database first (the processor's
/// tests do, in their fixture), and nothing here asserts on the others.
pub async fn drive_to(db: &Db, execution: Uuid, state: &str) {
    let reg = Answering(state == "SUCCESS");
    for _ in 0..50 {
        egret_workflow::worker::tick(&db.pool, 64, &reg)
            .await
            .expect("a worker tick");
        let now: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
            .bind(execution)
            .fetch_one(&db.pool)
            .await
            .expect("read the execution's state");
        if now == state {
            return;
        }
    }
    panic!("execution {execution} did not reach {state} in 50 ticks");
}

/// Every outbox event, oldest first: `(source execution, state)`.
pub async fn events(db: &Db) -> Vec<(Uuid, String)> {
    sqlx::query_as(
        "SELECT source_execution_id, state FROM workflow.flow_trigger_events ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read the outbox")
}

/// Every execution a Flow trigger fired from `source`:
/// `(listener flow, execution_depth, execution_trigger)`, by flow then id.
pub async fn fired_by(db: &Db, source: Uuid) -> Vec<(Uuid, i32, serde_json::Value)> {
    sqlx::query_as(
        "SELECT flow_id, execution_depth, execution_trigger FROM workflow.executions \
          WHERE trigger_execution_id = $1 ORDER BY flow_id, id",
    )
    .bind(source)
    .fetch_all(&db.pool)
    .await
    .expect("read the fired executions")
}
