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
    errors: Mutex<VecDeque<Error>>,
}

impl FakePort {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
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
        _params: &[egret_query_api::Param],
        limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
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
    ws_b: Uuid,
    app: Uuid,
    viewer: User,
    editor: User,
    outsider: User,
    /// `trino_prod` in A.
    trino_prod: Uuid,
    port: Arc<FakePort>,
    audit_rx: Receiver<AuditEvent>,
    router: axum::Router,
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

        let trino_prod = insert_database(&db, ws_a, "trino_prod", "trino").await;
        insert_database(&db, ws_b, "b_only", "trino").await;

        // Built AFTER the grants: `CedarPdp` snapshots policies at build time.
        let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
            egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
                .await
                .expect("build CedarPdp"),
        );
        let port = Arc::new(FakePort::default());
        let (audit, audit_rx) = egret_audit::AuditSink::new(64);
        let st = AppsState {
            db: db.clone(),
            pdp,
            query: port.clone(),
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

        Self {
            db,
            ws_a,
            ws_b,
            app,
            viewer,
            editor,
            outsider,
            trino_prod,
            port,
            audit_rx,
            router,
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
                        AppMode::Run => Some(Uuid::new_v4()),
                        AppMode::Edit => None,
                    },
                    mode,
                    login_sid: user.login_sid,
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
    let f = Fixture::new().await;
    // A custom role with app:write only, granted after the fixture: rebuild.
    let user = insert_user(&f.db).await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'test') RETURNING id",
    )
    .bind(format!("test_apps_{}", Uuid::new_v4().simple()))
    .fetch_one(&f.db.pool)
    .await
    .expect("role");
    for p in ["app:read", "app:write"] {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&f.db.pool)
            .await
            .expect("perm");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role)
    .bind(f.ws_a)
    .execute(&f.db.pool)
    .await
    .expect("grant");
    let author = User {
        id: user,
        login_sid: insert_login_session(&f.db, user).await,
    };
    // The PDP snapshot predates this grant; a fresh fixture state is needed.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let (audit, _rx) = egret_audit::AuditSink::new(64);
    let port = Arc::new(FakePort::default());
    let st = AppsState {
        db: f.db.clone(),
        pdp,
        query: port.clone(),
        audit,
        sessions: AppSessionRepo::new(f.db.clone()),
        limiter: egret_core::ratelimit::RateLimiter::new(20, 2, 2),
        cfg: AppsConfig::default(),
    };
    let router = axum::Router::new()
        .nest("/api", egret_apps::gateway_router())
        .with_state(st);

    let send = |token: String| {
        let router = router.clone();
        async move {
            router
                .oneshot(
                    Request::post("/api/apps/query")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(
                            json!({"database": "trino_prod", "sql": "SELECT 1"}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    // Run mode works for this role (sanity: the grant is live in the PDP)...
    assert_eq!(
        send(f.mint(&author, AppMode::Run).await).await,
        StatusCode::OK
    );
    // ...edit mode does not, for want of query:run alone.
    assert_eq!(
        send(f.mint(&author, AppMode::Edit).await).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        port.calls().len(),
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
    let _ = f.ws_b;
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
        rate_window_secs: 2,
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
