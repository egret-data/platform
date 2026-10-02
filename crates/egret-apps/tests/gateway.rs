//! `POST /api/apps/query` from the caller's side: real database, REAL Cedar PDP
//! (a mocked decision pins the consumer, not the server), a recording fake
//! `QueryExecPort` standing in for the engines.
//!
//! Every test builds its own users, workspaces, app and databases with fresh
//! random ids, so tests are independent within the process's shared database.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_apps::session::{AppMode, AppSessionRepo, NewAppSession};
use egret_apps::AppsState;
use egret_audit::AuditEvent;
use egret_config::AppsConfig;
use egret_core::{ColumnMeta, Error, TabularResult, UserId, WorkspaceId};
use egret_metastore::Db;
use egret_query_api::Param;
use serde_json::{json, Value};
use tokio::sync::mpsc::Receiver;
use tower::ServiceExt;
use uuid::Uuid;

const IDLE: Duration = Duration::from_secs(1800);
const ABS: Duration = Duration::from_secs(8 * 3600);

// ---------------------------------------------------------------- fake port

/// One recorded `execute_as*` call: `(ws, database_id, principal, sql, max_rows)`.
type Call = (Uuid, Uuid, UserId, String, usize);

#[derive(Default)]
struct FakePort {
    calls: Mutex<Vec<Call>>,
    /// The `params` of each `execute_as_with_params` call (not of `execute_as`).
    param_calls: Mutex<Vec<Vec<Param>>>,
    errors: Mutex<VecDeque<Error>>,
}

impl FakePort {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn param_calls(&self) -> Vec<Vec<Param>> {
        self.param_calls.lock().unwrap().clone()
    }
    fn queue_error(&self, e: Error) {
        self.errors.lock().unwrap().push_back(e);
    }
    fn record(
        &self,
        ws: Uuid,
        db: Uuid,
        p: UserId,
        sql: &str,
        max_rows: usize,
    ) -> Result<TabularResult, Error> {
        self.calls
            .lock()
            .unwrap()
            .push((ws, db, p, sql.to_string(), max_rows));
        if let Some(e) = self.errors.lock().unwrap().pop_front() {
            return Err(e);
        }
        Ok(TabularResult {
            columns: vec![ColumnMeta {
                name: "n".into(),
                ty: "integer".into(),
            }],
            rows: vec![vec![json!(1)]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
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
        unreachable!("the gateway never describes")
    }

    async fn execute_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        self.record(ws, database_id, principal, sql, limits.max_rows)
    }

    async fn execute_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        params: &[Param],
        limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        self.param_calls.lock().unwrap().push(params.to_vec());
        self.record(ws, database_id, principal, sql, limits.max_rows)
    }
}

// ------------------------------------------------------------------ fixture

struct User {
    id: Uuid,
    login_sid: Uuid,
}

struct Fixture {
    db: Db,
    ws_a: Uuid,
    app: Uuid,
    /// A published version of `app`: run sessions pin it (migration 0163's
    /// `app_sessions_version_app_fk` refuses a version of no app / another app).
    version: Uuid,
    viewer: User,
    editor: User,
    outsider: User,
    /// `trino_prod` in A.
    trino_prod: Uuid,
    port: Arc<FakePort>,
    audit_rx: Receiver<AuditEvent>,
    router: axum::Router,
    cfg: AppsConfig,
}

/// The gateway over a FRESH `CedarPdp` — it snapshots grants at build time, so
/// it must be built after every grant a test relies on.
async fn build_router(
    db: &Db,
    port: Arc<FakePort>,
    cfg: AppsConfig,
) -> (axum::Router, Receiver<AuditEvent>) {
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let (audit, audit_rx) = egret_audit::AuditSink::new(64);
    let st = AppsState {
        db: db.clone(),
        pdp,
        query: port,
        audit,
        sessions: AppSessionRepo::new(db.clone()),
        limiter: egret_core::ratelimit::RateLimiter::new(
            cfg.rate_max,
            cfg.rate_window_secs,
            cfg.rate_window_secs,
        ),
        cfg,
    };
    let router = axum::Router::new()
        .nest("/api", egret_apps::gateway_router())
        .with_state(st);
    (router, audit_rx)
}

async fn insert_user(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("apps_gw_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn insert_login_session(db: &Db, user: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
         VALUES ($1, $2, now() + interval '8 hours', now() + interval '30 minutes') RETURNING id",
    )
    .bind(format!("gw{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .expect("insert login session")
}

async fn insert_workspace(db: &Db, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Apps GW', $2, 'active') RETURNING id",
    )
    .bind(format!("apps-gw-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace")
}

/// Grants a SEEDED workspace role (`ws_viewer`/`ws_editor`), the way a real
/// member holds it — so the test exercises the shipped role → permission rows.
async fn grant_role(db: &Db, user: Uuid, role: &str, ws: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("seeded role");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role_id)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant role");
}

async fn insert_database(db: &Db, ws: Uuid, name: &str, engine: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port) \
         VALUES ($1, $2, $3, 'localhost', 8080) RETURNING id",
    )
    .bind(ws)
    .bind(name)
    .bind(engine)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

impl Fixture {
    async fn new() -> Self {
        Self::with_cfg(AppsConfig::default()).await
    }

    async fn with_cfg(cfg: AppsConfig) -> Self {
        let url = egret_test_db::url()
            .await
            .expect("test database (DATABASE_URL)");
        let db = Db::connect(&url, 5).await.expect("connect");

        let owner = insert_user(&db).await;
        let ws_a = insert_workspace(&db, owner).await;
        let ws_b = insert_workspace(&db, owner).await;

        let mut users = Vec::new();
        for role in [Some("ws_viewer"), Some("ws_editor"), None] {
            let id = insert_user(&db).await;
            if let Some(r) = role {
                grant_role(&db, id, r, ws_a).await;
            }
            let login_sid = insert_login_session(&db, id).await;
            users.push(User { id, login_sid });
        }
        let outsider = users.pop().unwrap();
        let editor = users.pop().unwrap();
        let viewer = users.pop().unwrap();

        let app: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'gw app', $2) RETURNING id",
        )
        .bind(ws_a)
        .bind(editor.id)
        .fetch_one(&db.pool)
        .await
        .expect("insert app");
        let version: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no, source) \
             VALUES ($1, 1, 'import marimo') RETURNING id",
        )
        .bind(app)
        .fetch_one(&db.pool)
        .await
        .expect("insert version");

        let trino_prod = insert_database(&db, ws_a, "trino_prod", "trino").await;
        insert_database(&db, ws_b, "b_only", "trino").await;

        let port = Arc::new(FakePort::default());
        let (router, audit_rx) = build_router(&db, port.clone(), cfg.clone()).await;

        Self {
            db,
            ws_a,
            app,
            version,
            viewer,
            editor,
            outsider,
            trino_prod,
            port,
            audit_rx,
            router,
            cfg,
        }
    }

    /// Rebuild the router (and its PDP snapshot) after adding grants. Same port.
    async fn rebuild(&mut self) {
        let (router, audit_rx) = build_router(&self.db, self.port.clone(), self.cfg.clone()).await;
        self.router = router;
        self.audit_rx = audit_rx;
    }

    /// A fresh user with a live login session.
    async fn new_user(&self) -> User {
        let id = insert_user(&self.db).await;
        User {
            id,
            login_sid: insert_login_session(&self.db, id).await,
        }
    }

    async fn mint(&self, user: &User, mode: AppMode) -> String {
        AppSessionRepo::new(self.db.clone())
            .mint(
                &NewAppSession {
                    user_id: UserId(user.id),
                    workspace_id: WorkspaceId(self.ws_a),
                    app_id: self.app,
                    version_id: match mode {
                        AppMode::Run => Some(self.version),
                        AppMode::Edit => None,
                    },
                    mode,
                    login_sid: user.login_sid,
                    grant_id: None,
                    impersonator: None,
                },
                IDLE,
                ABS,
            )
            .await
            .expect("mint")
    }

    async fn viewer_run(&self) -> String {
        self.mint(&self.viewer, AppMode::Run).await
    }

    async fn send_raw(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.router.clone().oneshot(req).await.expect("oneshot");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, body)
    }

    async fn query(&self, token: &str, body: Value) -> (StatusCode, Value) {
        self.send_raw(
            Request::post("/api/apps/query")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    async fn select1(&self, token: &str) -> (StatusCode, Value) {
        self.query(token, json!({"database": "trino_prod", "sql": "SELECT 1"}))
            .await
    }

    async fn exec(&self, sql: &'static str) {
        sqlx::query(sql)
            .bind(self.app)
            .execute(&self.db.pool)
            .await
            .expect("exec");
    }

    /// Every audit event emitted so far.
    fn drain_audit(&mut self) -> Vec<AuditEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.audit_rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

/// A workspace role holding exactly `perms`.
async fn custom_role(db: &Db, perms: &[&str]) -> Uuid {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_apps_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("role");
    for p in perms {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&db.pool)
            .await
            .expect("perm");
    }
    role
}

/// `principal_type` is `user` or `group`.
async fn grant(db: &Db, principal_type: &str, principal: Uuid, role: Uuid, ws: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ($1, $2, $3, 'workspace', $4)",
    )
    .bind(principal_type)
    .bind(principal)
    .bind(role)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant");
}

fn mutation(ev: &AuditEvent) -> &Value {
    let AuditEvent::Mutation { detail, .. } = ev else {
        panic!("expected Mutation, got {ev:?}");
    };
    detail
}

fn error_text(body: &Value) -> String {
    body["error"].as_str().unwrap_or_default().to_string()
}

// -------------------------------------------------------------------- cases

/// 1: no credential, a malformed bearer, and a cookie alone are all 401.
#[tokio::test]
async fn case01_missing_or_garbage_credential_is_401() {
    let f = Fixture::new().await;
    let body = json!({"database": "trino_prod", "sql": "SELECT 1"}).to_string();

    let none = Request::post("/api/apps/query")
        .header("content-type", "application/json")
        .body(Body::from(body.clone()))
        .unwrap();
    assert_eq!(f.send_raw(none).await.0, StatusCode::UNAUTHORIZED);

    let (s, _) = f
        .query(
            "garbage",
            json!({"database": "trino_prod", "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Well-formed prefix, unknown token.
    let (s, _) = f
        .query(
            "ega_notarealtoken",
            json!({"database": "x", "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let token = f.viewer_run().await;
    let cookie = Request::post("/api/apps/query")
        .header("cookie", format!("egret_session={token}"))
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    assert_eq!(f.send_raw(cookie).await.0, StatusCode::UNAUTHORIZED);

    // Even an unparseable body is 401 first: authentication precedes everything.
    let junk = Request::post("/api/apps/query")
        .body(Body::from("{not json"))
        .unwrap();
    assert_eq!(f.send_raw(junk).await.0, StatusCode::UNAUTHORIZED);

    assert!(f.port.calls().is_empty(), "port must not be called");
}

/// 2: revoked, idle-expired, and login-session-deleted tokens are 401.
#[tokio::test]
async fn case02_dead_tokens_are_401() {
    let f = Fixture::new().await;

    let revoked = f.viewer_run().await;
    AppSessionRepo::new(f.db.clone())
        .revoke(&revoked)
        .await
        .expect("revoke");
    assert_eq!(f.select1(&revoked).await.0, StatusCode::UNAUTHORIZED);

    let idle = f.viewer_run().await;
    sqlx::query(
        "UPDATE analytics.app_sessions SET idle_expires_at = now() - interval '1 second' \
         WHERE app_id = $1 AND token_hash = $2",
    )
    .bind(f.app)
    .bind(egret_core::token::hash_token(&idle))
    .execute(&f.db.pool)
    .await
    .expect("expire");
    assert_eq!(f.select1(&idle).await.0, StatusCode::UNAUTHORIZED);

    let orphan = f.viewer_run().await;
    assert_eq!(
        f.select1(&orphan).await.0,
        StatusCode::OK,
        "live before logout"
    );
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(f.viewer.login_sid)
        .execute(&f.db.pool)
        .await
        .expect("logout");
    assert_eq!(f.select1(&orphan).await.0, StatusCode::UNAUTHORIZED);
}

/// 3: disabling the user kills a token minted before.
#[tokio::test]
async fn case03_disabled_user_is_401() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(f.viewer.id)
        .execute(&f.db.pool)
        .await
        .expect("disable");
    assert_eq!(f.select1(&token).await.0, StatusCode::UNAUTHORIZED);
    assert!(f.port.calls().is_empty());
}

/// 4: the `apps` feature switched off for the workspace.
#[tokio::test]
async fn case04_feature_disabled_is_403() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, 'apps', false)",
    )
    .bind(f.ws_a)
    .execute(&f.db.pool)
    .await
    .expect("disable feature");
    let (s, body) = f.select1(&token).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "feature_disabled", "{body}");
    assert!(f.port.calls().is_empty());
}

/// 5: a soft-deleted app is gone.
#[tokio::test]
async fn case05_deleted_app_is_404() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    f.exec("UPDATE analytics.apps SET deleted_at = now() WHERE id = $1")
        .await;
    assert_eq!(f.select1(&token).await.0, StatusCode::NOT_FOUND);
    assert!(f.port.calls().is_empty());
}

/// 6: a token for a user with no grant in the app's workspace.
#[tokio::test]
async fn case06_outsider_is_403() {
    let f = Fixture::new().await;
    let token = f.mint(&f.outsider, AppMode::Run).await;
    assert_eq!(f.select1(&token).await.0, StatusCode::FORBIDDEN);
    assert!(f.port.calls().is_empty());
}

/// 7: a `ws_viewer` (who lacks `query:run`) can run a published app, and the
/// query runs as the viewer in the token's workspace.
#[tokio::test]
async fn case07_viewer_run_is_200_as_viewer() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    let (s, body) = f.select1(&token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let calls = f.port.calls();
    assert_eq!(calls.len(), 1);
    let (ws, db, principal, sql, _) = &calls[0];
    assert_eq!(*ws, f.ws_a);
    assert_eq!(*db, f.trino_prod);
    assert_eq!(*principal, UserId(f.viewer.id));
    assert_eq!(sql, "SELECT 1");
}

/// 8: edit mode needs `app:write` + `query:run`; a viewer has neither.
#[tokio::test]
async fn case08_viewer_edit_is_403() {
    let f = Fixture::new().await;
    let token = f.mint(&f.viewer, AppMode::Edit).await;
    assert_eq!(f.select1(&token).await.0, StatusCode::FORBIDDEN);
    assert!(f.port.calls().is_empty());
}

/// 8b: edit mode with `app:write` but WITHOUT `query:run` is still refused —
/// pins the second check on its own (a ws_editor holds both, so case 9 cannot).
#[tokio::test]
async fn case08b_edit_without_query_run_is_403() {
    let mut f = Fixture::new().await;
    let author = f.new_user().await;
    let role = custom_role(&f.db, &["app:read", "app:write"]).await;
    grant(&f.db, "user", author.id, role, f.ws_a).await;
    f.rebuild().await;

    // Run mode works for this role (sanity: the grant is live in the PDP)...
    let run = f.mint(&author, AppMode::Run).await;
    assert_eq!(f.select1(&run).await.0, StatusCode::OK);
    // ...edit mode does not, for want of query:run alone.
    let edit = f.mint(&author, AppMode::Edit).await;
    assert_eq!(f.select1(&edit).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        f.port.calls().len(),
        1,
        "only the run-mode call reached the port"
    );
}

/// 9: an editor's edit token runs.
#[tokio::test]
async fn case09_editor_edit_is_200() {
    let f = Fixture::new().await;
    let token = f.mint(&f.editor, AppMode::Edit).await;
    let (s, body) = f.select1(&token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(f.port.calls()[0].2, UserId(f.editor.id));
}

/// 10: a database in ANOTHER workspace and one that exists nowhere are
/// indistinguishable — same status, byte-identical body.
#[tokio::test]
async fn case10_foreign_and_missing_database_are_identical_404() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    let foreign = f
        .query(&token, json!({"database": "b_only", "sql": "SELECT 1"}))
        .await;
    let missing = f
        .query(
            &token,
            json!({"database": format!("nowhere_{}", Uuid::new_v4().simple()), "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(foreign.0, StatusCode::NOT_FOUND, "{}", foreign.1);
    assert_eq!(foreign, missing);
    assert!(f.port.calls().is_empty());
}

/// 11: writes and multi-statement batches are refused with a reason.
#[tokio::test]
async fn case11_non_read_only_sql_is_400_with_reason() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    let (s, body) = f
        .query(
            &token,
            json!({"database": "trino_prod", "sql": "DELETE FROM t"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(error_text(&body).contains("may only read"), "{body}");
    let (s, body) = f
        .query(
            &token,
            json!({"database": "trino_prod", "sql": "SELECT 1; DROP TABLE t"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        error_text(&body).contains("exactly one statement"),
        "{body}"
    );
    assert!(f.port.calls().is_empty());
}

/// 12 + 16: an engine-side denial is 403, and audited as `forbidden`.
#[tokio::test]
async fn case12_and_16_port_forbidden_is_403_and_audited() {
    let mut f = Fixture::new().await;
    let token = f.viewer_run().await;
    f.port.queue_error(Error::Forbidden);
    assert_eq!(f.select1(&token).await.0, StatusCode::FORBIDDEN);
    let events = f.drain_audit();
    assert_eq!(events.len(), 1);
    let AuditEvent::Mutation { detail, .. } = &events[0] else {
        panic!("expected Mutation, got {:?}", events[0]);
    };
    assert_eq!(detail["outcome"], "forbidden");
}

/// 13: an engine syntax error reaches the caller verbatim as a 400.
#[tokio::test]
async fn case13_port_invalid_input_is_400_with_message() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    f.port.queue_error(Error::InvalidInput(
        "line 1: mismatched input 'FROMM'".into(),
    ));
    let (s, body) = f.select1(&token).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        error_text(&body).contains("line 1: mismatched input 'FROMM'"),
        "{body}"
    );
}

/// 14: `max_rows` is clamped to the config cap; absent means the cap.
#[tokio::test]
async fn case14_max_rows_is_capped() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    for (asked, seen) in [(Some(1_000_000), 10_000), (Some(5), 5), (None, 10_000)] {
        let mut body = json!({"database": "trino_prod", "sql": "SELECT 1"});
        if let Some(n) = asked {
            body["max_rows"] = json!(n);
        }
        assert_eq!(f.query(&token, body).await.0, StatusCode::OK);
        assert_eq!(f.port.calls().last().unwrap().4, seen, "asked {asked:?}");
    }
}

/// 15: the success body and its single audit event, which carries a hash of
/// the SQL and never the SQL itself.
#[tokio::test]
async fn case15_success_body_and_audit() {
    let mut f = Fixture::new().await;
    let token = f.viewer_run().await;
    let sql = "SELECT 'secret_literal_42' AS n";
    let (s, body) = f
        .query(&token, json!({"database": "trino_prod", "sql": sql}))
        .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body["columns"].is_array(), "{body}");
    assert!(body["rows"].is_array(), "{body}");
    assert_eq!(body["truncated"], false);

    let events = f.drain_audit();
    assert_eq!(events.len(), 1, "{events:?}");
    let AuditEvent::Mutation {
        action,
        resource_id,
        detail,
        actor,
        workspace_id,
        ..
    } = &events[0]
    else {
        panic!("expected Mutation, got {:?}", events[0]);
    };
    assert_eq!(action, "app:query");
    assert_eq!(resource_id.as_deref(), Some(f.app.to_string().as_str()));
    assert_eq!(*actor, Some(f.viewer.id));
    assert_eq!(*workspace_id, Some(f.ws_a));
    assert_eq!(detail["outcome"], "ok");
    assert!(
        detail["version_id"].is_string(),
        "run token pins a version: {detail}"
    );
    assert_eq!(detail["database"], "trino_prod");
    assert_eq!(detail["row_count"], 1);
    assert!(detail["duration_ms"].is_u64(), "{detail}");
    let expected_hash = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(sql.as_bytes()))
    };
    assert_eq!(detail["sql_sha256"], expected_hash.as_str());
    let text = detail.to_string();
    assert!(!text.contains("secret_literal_42"), "SQL leaked: {text}");
    assert!(!text.contains("SELECT"), "SQL leaked: {text}");
}

/// 17: per-token rate limit; another token is unaffected in the same window.
#[tokio::test]
async fn case17_rate_limited_per_token() {
    let f = Fixture::with_cfg(AppsConfig {
        rate_max: 20,
        // long window so 25 sequential DB calls cannot straddle a reset
        rate_window_secs: 60,
        ..AppsConfig::default()
    })
    .await;
    let hot = f.viewer_run().await;
    let mut statuses = Vec::new();
    for _ in 0..25 {
        statuses.push(f.select1(&hot).await.0);
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "{statuses:?}"
    );
    assert_eq!(statuses[0], StatusCode::OK);
    let cool = f.viewer_run().await;
    assert_eq!(f.select1(&cool).await.0, StatusCode::OK);
}

/// 18: a registered database on an engine the read-only gate has no dialect for.
#[tokio::test]
async fn case18_unsupported_engine_is_400() {
    let f = Fixture::new().await;
    // `query.databases.engine` is CHECK-constrained to trino/starrocks today, so a
    // third engine is only reachable by relaxing the CHECK. Done in one
    // transaction that re-adds it NOT VALID, so the constraint is back in force
    // for every later insert in this process's database.
    let name = format!("rw_{}", Uuid::new_v4().simple());
    let mut tx = f.db.pool.begin().await.expect("begin");
    sqlx::query("ALTER TABLE query.databases DROP CONSTRAINT databases_engine_check")
        .execute(&mut *tx)
        .await
        .expect("drop check");
    sqlx::query(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port) \
         VALUES ($1, $2, 'risingwave', 'localhost', 4566)",
    )
    .bind(f.ws_a)
    .bind(&name)
    .execute(&mut *tx)
    .await
    .expect("insert");
    sqlx::query(
        "ALTER TABLE query.databases ADD CONSTRAINT databases_engine_check \
         CHECK (engine = ANY (ARRAY['trino'::text, 'starrocks'::text])) NOT VALID",
    )
    .execute(&mut *tx)
    .await
    .expect("re-add check");
    tx.commit().await.expect("commit");

    let token = f.viewer_run().await;
    let (s, body) = f
        .query(&token, json!({"database": name, "sql": "SELECT 1"}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        error_text(&body).contains("engine not supported by data apps"),
        "{body}"
    );
    assert!(f.port.calls().is_empty());
}

/// Fix round 1: an archived workspace is gone — the same 404 as a deleted app,
/// and audited as `not_found`.
#[tokio::test]
async fn archived_workspace_is_404_like_a_missing_app() {
    let mut f = Fixture::new().await;
    let token = f.viewer_run().await;
    let deleted_app_body = {
        let other = f.viewer_run().await;
        assert_eq!(f.select1(&other).await.0, StatusCode::OK, "live first");
        f.exec("UPDATE analytics.apps SET deleted_at = now() WHERE id = $1")
            .await;
        let r = f.select1(&other).await;
        f.exec("UPDATE analytics.apps SET deleted_at = NULL WHERE id = $1")
            .await;
        r
    };
    f.drain_audit();

    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(f.ws_a)
        .execute(&f.db.pool)
        .await
        .expect("archive");
    let archived = f.select1(&token).await;
    assert_eq!(archived.0, StatusCode::NOT_FOUND, "{}", archived.1);
    assert_eq!(
        archived, deleted_app_body,
        "indistinguishable from a deleted app"
    );
    let events = f.drain_audit();
    assert_eq!(events.len(), 1);
    assert_eq!(mutation(&events[0])["outcome"], "not_found");
    assert_eq!(f.port.calls().len(), 1, "only the pre-archive call ran");
}

/// Fix round 1: a 429 on a resolved session is audited as `rate_limited`, with
/// no database or SQL hash (the body was never read).
#[tokio::test]
async fn rate_limited_call_is_audited() {
    let mut f = Fixture::with_cfg(AppsConfig {
        rate_max: 1,
        rate_window_secs: 60,
        ..AppsConfig::default()
    })
    .await;
    let token = f.viewer_run().await;
    assert_eq!(f.select1(&token).await.0, StatusCode::OK);
    assert_eq!(f.select1(&token).await.0, StatusCode::TOO_MANY_REQUESTS);
    let events = f.drain_audit();
    assert_eq!(events.len(), 2, "{events:?}");
    let d = mutation(&events[1]);
    assert_eq!(d["outcome"], "rate_limited");
    assert_eq!(d["database"], "");
    assert!(d["sql_sha256"].is_null(), "{d}");
}

/// P4a: a flood is one audit row, not one per rejected call. The limiter keeps a
/// token locked for a window after the call that tripped it; every call inside that
/// window is a 429 with no audit, so a runaway cell cannot fill the audit table.
#[tokio::test]
async fn a_flood_over_the_limit_is_audited_once() {
    let mut f = Fixture::with_cfg(AppsConfig {
        rate_max: 1,
        rate_window_secs: 60,
        ..AppsConfig::default()
    })
    .await;
    let token = f.viewer_run().await;
    assert_eq!(f.select1(&token).await.0, StatusCode::OK);
    for _ in 0..50 {
        assert_eq!(f.select1(&token).await.0, StatusCode::TOO_MANY_REQUESTS);
    }
    let events = f.drain_audit();
    let outcomes: Vec<&str> = events
        .iter()
        .map(|e| mutation(e)["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["ok", "rate_limited"], "{outcomes:?}");
}

/// P4a: a query made through an impersonated session names the real admin in the
/// audit event; it is never recorded as the user's own act.
#[tokio::test]
async fn an_impersonated_session_audits_the_impersonator() {
    let mut f = Fixture::new().await;
    let admin = f.new_user().await;
    let token = AppSessionRepo::new(f.db.clone())
        .mint(
            &NewAppSession {
                user_id: UserId(f.viewer.id),
                workspace_id: WorkspaceId(f.ws_a),
                app_id: f.app,
                version_id: Some(f.version),
                mode: AppMode::Run,
                login_sid: f.viewer.login_sid,
                grant_id: None,
                impersonator: Some(UserId(admin.id)),
            },
            IDLE,
            ABS,
        )
        .await
        .unwrap();
    assert_eq!(f.select1(&token).await.0, StatusCode::OK);
    let events = f.drain_audit();
    let AuditEvent::Mutation {
        actor,
        impersonator,
        ..
    } = &events[0]
    else {
        panic!("expected Mutation");
    };
    assert_eq!(*actor, Some(f.viewer.id));
    assert_eq!(*impersonator, Some(admin.id));
}

/// Fix round 1: an unparseable body on a resolved session is 400, audited as
/// `invalid`. (Before a session resolves, a 401 is not audited — case 1.)
#[tokio::test]
async fn unparseable_body_is_400_and_audited() {
    let mut f = Fixture::new().await;
    let token = f.viewer_run().await;
    let (s, body) = f
        .send_raw(
            Request::post("/api/apps/query")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(error_text(&body).contains("invalid request body"), "{body}");
    let events = f.drain_audit();
    assert_eq!(events.len(), 1);
    let d = mutation(&events[0]);
    assert_eq!(d["outcome"], "invalid");
    assert_eq!(d["database"], "");
    assert!(d["sql_sha256"].is_null(), "{d}");
    assert!(f.port.calls().is_empty());

    // An unauthenticated junk body is 401 and leaves no audit row.
    let (s, _) = f
        .send_raw(
            Request::post("/api/apps/query")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(f.drain_audit().is_empty());
}

/// Fix round 1: bound parameters reach `execute_as_with_params`, typed; an
/// integer above i64::MAX is refused rather than rounded through f64.
#[tokio::test]
async fn params_are_bound_and_out_of_range_integers_refused() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    let (s, body) = f
        .query(
            &token,
            json!({"database": "trino_prod", "sql": "SELECT ?, ?, ?, ?, ?",
                   "params": [null, true, 7, 1.5, "x"]}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(
        f.port.param_calls(),
        vec![vec![
            Param::Null,
            Param::Bool(true),
            Param::I64(7),
            Param::F64(1.5),
            Param::Str("x".into()),
        ]]
    );

    let (s, body) = f
        .send_raw(
            Request::post("/api/apps/query")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(format!(
                    r#"{{"database":"trino_prod","sql":"SELECT ?","params":[{}]}}"#,
                    u64::MAX
                )))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(error_text(&body).contains("out of range"), "{body}");
    assert_eq!(f.port.calls().len(), 1, "the refused call never ran");
}

/// Fix round 1: the auth scheme is case-insensitive (RFC 7235).
#[tokio::test]
async fn bearer_scheme_is_case_insensitive() {
    let f = Fixture::new().await;
    let token = f.viewer_run().await;
    for scheme in ["bearer", "BEARER"] {
        let (s, body) = f
            .send_raw(
                Request::post("/api/apps/query")
                    .header("authorization", format!("{scheme} {token}"))
                    .body(Body::from(
                        json!({"database": "trino_prod", "sql": "SELECT 1"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{scheme}: {body}");
    }
    let (s, _) = f
        .send_raw(
            Request::post("/api/apps/query")
                .header("authorization", "bearer x")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

/// Fix round 1: group membership reaches the PDP subject. Two things depend
/// on it, and this pins both:
/// - a role granted to a GROUP authorizes the member (the PDP resolves this
///   one from the DB itself);
/// - a classification on the app that clears ONLY by group membership reads
///   `Subject::group_ids` — what the gateway loads from `user_groups`. Without
///   that load the member is forbidden like any non-member.
#[tokio::test]
async fn group_membership_authorizes_and_clears_a_group_gated_app() {
    let mut f = Fixture::new().await;
    let member = f.new_user().await;
    let group: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("apps_gw_{}", Uuid::new_v4().simple()))
        .fetch_one(&f.db.pool)
        .await
        .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(member.id)
        .bind(group)
        .execute(&f.db.pool)
        .await
        .expect("membership");
    let role = custom_role(&f.db, &["app:read"]).await;
    grant(&f.db, "group", group, role, f.ws_a).await;

    // Gate the app on membership of `group` (no role clears it).
    let tag: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, 'Dept') RETURNING id")
            .bind(format!("dept-{}", Uuid::new_v4().simple()))
            .fetch_one(&f.db.pool)
            .await
            .expect("tag");
    sqlx::query(
        "INSERT INTO tag_assignments (tag_id, object_type, object_id) VALUES ($1, 'app', $2)",
    )
    .bind(tag)
    .bind(f.app)
    .execute(&f.db.pool)
    .await
    .expect("tag app");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, '{}', $2)",
    )
    .bind(tag)
    .bind(vec![group])
    .execute(&f.db.pool)
    .await
    .expect("classification");
    f.rebuild().await;

    let token = f.mint(&member, AppMode::Run).await;
    let (s, body) = f.select1(&token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(f.port.calls()[0].2, UserId(member.id));

    // The fixture viewer holds app:read directly but is not in the group.
    let outside = f.viewer_run().await;
    assert_eq!(f.select1(&outside).await.0, StatusCode::FORBIDDEN);
}
