//! DB-gated ROUTE tests for Alerts & Reports. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
//!
//! These assert the three things the repo tests cannot see, because they only
//! exist at the HTTP layer:
//!
//! 1. **Authz.** `alert:manage` gates every write. A `ws_viewer` reading the
//!    list is fine; a `ws_viewer` creating a schedule is not.
//! 2. **Cross-workspace is 404, not 403.** Matching the rest of the API: a
//!    403 would confirm the id exists.
//! 3. **Validation is a 400 with a usable message**, not a 500 from a CHECK
//!    constraint firing deep in the write path.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_automation::render::token::RenderTokenRepo;
use egret_automation::{automation_router, AutomationState, ScheduleRepo};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// This file doesn't exercise rendering; every database in it is
/// non-delegated, so the minter is never actually called.
struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

struct Fx {
    app: axum::Router,
    slug: String,
    /// A second workspace the editor is ALSO a member of, so a cross-workspace
    /// request fails on scoping rather than on membership.
    other_slug: String,
    editor_tok: String,
    viewer_tok: String,
    database: Uuid,
    db: Db,
    /// The workspace behind `slug`, and the service that mints a session for a
    /// principal a test creates itself.
    ws: Uuid,
    /// The workspace behind `other_slug` — needed by id (not just slug) to
    /// create a chart/dashboard IN it for the cross-workspace target tests.
    other_ws: Uuid,
    sessions: egret_auth::session::SessionService,
}

impl Fx {
    async fn token_for(&self, user: Uuid) -> String {
        self.sessions
            .create(egret_core::ids::UserId(user), "test", "127.0.0.1")
            .await
            .expect("create session")
            .token
    }
}

async fn mk_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn grant_ws_role(db: &Db, user: Uuid, ws: Uuid, role: &str) {
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3",
    )
    .bind(ws)
    .bind(user)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add member")
    .rows_affected();
    // A typo'd role grants NOTHING, which would make every "denied" assertion
    // below pass for the wrong reason.
    assert_eq!(n, 1, "role {role} must exist and have been granted");
}

/// A PLATFORM-scoped grant (`scope_id = NULL`) — `platform_admin` reaches
/// every workspace through `WsContext`'s bypass
/// (`egret-workspace/src/context.rs:108-113`), never through an
/// `access_grants` row on the workspace itself, so this is deliberately a
/// different statement from `grant_ws_role` rather than the same one with
/// `ws: None`.
async fn grant_platform_role(db: &Db, user: Uuid, role: &str) {
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'platform', NULL FROM roles r \
         WHERE r.scope = 'platform' AND r.name = $2",
    )
    .bind(user)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add platform grant")
    .rows_affected();
    assert_eq!(n, 1, "role {role} must exist and have been granted");
}

/// The inverse of `grant_ws_role` — used to make a schedule's existing owner
/// stop being a member of `ws`, so an update that does not touch `owners` can
/// be checked against a stored owner who has since left.
async fn revoke_ws_role(db: &Db, user: Uuid, ws: Uuid) {
    sqlx::query(
        "DELETE FROM access_grants \
         WHERE principal_type = 'user' AND principal_id = $1 \
           AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(user)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("revoke member");
}

async fn mk_ws(db: &Db, owner: Uuid, slug: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Alerts Test', $2, 'active') RETURNING id",
    )
    .bind(slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace")
}

async fn setup() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let editor = mk_user(&db, "al_editor").await;
    let viewer = mk_user(&db, "al_viewer").await;
    let slug = format!("al-{}", Uuid::new_v4().simple());
    let other_slug = format!("alo-{}", Uuid::new_v4().simple());
    let ws = mk_ws(&db, editor, &slug).await;
    let other_ws = mk_ws(&db, editor, &other_slug).await;

    grant_ws_role(&db, editor, ws, "ws_editor").await;
    grant_ws_role(&db, editor, other_ws, "ws_editor").await;
    grant_ws_role(&db, viewer, ws, "ws_viewer").await;

    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let secrets = egret_secrets::SecretStore::new(db.clone(), None);
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = egret_query::DatabaseRepo::new(db.clone(), secrets, ws_secrets);
    let smtp = egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(
            db.clone(),
            Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
        ),
    );
    // "Run now" goes through the SAME `AutomationRunner` the tick uses — this
    // is the one place both `AutomationState.runner` (the route) and a real
    // `runner::spawn` ticker (production) would share, so a manual run and a
    // scheduled run cannot evaluate an alert differently. This fixture never
    // spawns the ticker, so the runner here is exercised only via the route.
    let runner = Arc::new(egret_automation::runner::AutomationRunner {
        db: db.clone(),
        schedules: ScheduleRepo::new(
            db.clone(),
            // Crypto-enabled: `SecretStore::new(db, None)` cannot ENCRYPT, so a
            // webhook secret would fail to vault on write.
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        databases: databases.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        minter: Arc::new(NoMinter),
        query: Arc::new(egret_automation::evaluator::EngineRunner {
            databases: databases.clone(),
            factory: Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
            limits: egret_query::execution::model::Limits {
                max_rows: 1000,
                timeout: std::time::Duration::from_secs(5),
            },
        }),
        pdp: pdp.clone(),
        dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        resolver: Arc::new(egret_automation::deliver::destination::SystemResolver),
        policy: egret_automation::deliver::destination::SsrfPolicy::default(),
        smtp: smtp.clone(),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        charts: egret_bi::ChartRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        render_tokens: RenderTokenRepo::new(db.clone()),
        renderer: Arc::new(egret_automation::render::client::HttpRenderer::new(
            "http://127.0.0.1:1".into(),
        )),
        public_url: "https://egret.test".into(),
        browser_url: "https://egret.test".into(),
    });
    let st = AutomationState {
        db: db.clone(),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        schedules: ScheduleRepo::new(
            db.clone(),
            // Crypto-enabled: `SecretStore::new(db, None)` cannot ENCRYPT, so a
            // webhook secret would fail to vault on write.
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        sessions: sessions.clone(),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        smtp,
        render_tokens: RenderTokenRepo::new(db.clone()),
        charts: egret_bi::ChartRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db.clone()),
        databases: databases.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        dataapi_endpoints: Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        // Not exercised by this file's tests; wired for the struct to
        // construct at all.
        query: Arc::new(egret_automation::evaluator::EngineRunner {
            databases,
            factory: Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
            limits: egret_query::execution::model::Limits {
                max_rows: 1000,
                timeout: std::time::Duration::from_secs(5),
            },
        }),
        runner,
    };

    let mut tok = Vec::new();
    for u in [editor, viewer] {
        tok.push(
            sessions
                .create(egret_core::ids::UserId(u), "test", "127.0.0.1")
                .await
                .expect("create session")
                .token,
        );
    }
    Some(Fx {
        app: automation_router().with_state(st),
        slug,
        other_slug,
        viewer_tok: tok.pop().unwrap(),
        editor_tok: tok.pop().unwrap(),
        database,
        db,
        ws,
        other_ws,
        sessions,
    })
}

/// A dashboard that exists only in `ws` — used to prove a report/alert in a
/// DIFFERENT workspace cannot name it.
async fn mk_dashboard(db: &Db, ws: Uuid, owner: Uuid) -> Uuid {
    egret_bi::DashboardRepo::new(db.clone())
        .create(ws, "target dashboard", "", Some(owner))
        .await
        .expect("create dashboard")
        .id
}

/// A chart that exists only in `ws`, with its own database + dataset so
/// `ChartRepo::create`'s FK to `analytics.datasets` is satisfied. The chart is
/// never run in these tests — only its existence and workspace matter.
async fn mk_chart(db: &Db, ws: Uuid, owner: Uuid) -> Uuid {
    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("insert database");

    let dataset = egret_bi::DatasetRepo::new(db.clone())
        .create(
            ws,
            database,
            "target dataset",
            "",
            &egret_bi::SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "tiny".into(),
                table: "orders".into(),
            },
            &[],
            Some(egret_core::UserId(owner)),
        )
        .await
        .expect("create dataset");

    egret_bi::ChartRepo::new(db.clone())
        .create(
            ws,
            dataset.id,
            "target chart",
            "",
            "table",
            &serde_json::json!({}),
            Some(owner),
        )
        .await
        .expect("create chart")
        .id
}

async fn req(
    fx: &Fx,
    tok: &str,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Cookie", format!("egret_session={tok}"));
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    let res = fx
        .app
        .clone()
        .oneshot(
            b.body(body.map(Body::from).unwrap_or(Body::empty()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// A body valid for both POST and PUT.
///
/// `active` is spelled out because `UpdateSchedule` requires it: PUT is a full
/// replace, and defaulting a missing `active` to true would silently resume a
/// schedule the user had paused.
fn alert_json(fx: &Fx, name: &str) -> String {
    serde_json::json!({
        "type": "alert",
        "name": name,
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "database_id": fx.database,
        "sql": "SELECT count(*) FROM orders",
        "validator_type": "not_null",
        "recipients": [{"kind": "email", "config": {"addresses": ["on-call@t.local"]}}],
    })
    .to_string()
}

/// Like [`req`] but returns the RAW body text.
///
/// The webhook-secret assertions have to read the bytes on the wire, not a
/// parsed field: a secret that leaked under some other key, or nested inside
/// `config`, would still satisfy `v["secret"].is_none()`.
async fn req_raw(
    fx: &Fx,
    tok: &str,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Cookie", format!("egret_session={tok}"));
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    let res = fx
        .app
        .clone()
        .oneshot(
            b.body(body.map(Body::from).unwrap_or(Body::empty()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// An alert delivering to a webhook whose POSTs are HMAC-signed.
fn alert_with_webhook(fx: &Fx, name: &str, secret: &str) -> String {
    serde_json::json!({
        "type": "alert",
        "name": name,
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "database_id": fx.database,
        "sql": "SELECT count(*) FROM orders",
        "validator_type": "not_null",
        "recipients": [{
            "kind": "webhook",
            "config": {"url": "https://hooks.example.com/egret", "secret": secret},
        }],
    })
    .to_string()
}

macro_rules! fixture {
    () => {
        match setup().await {
            Some(f) => f,
            None => {
                eprintln!("skip: DATABASE_URL unset");
                return;
            }
        }
    };
}

#[tokio::test]
async fn an_editor_can_create_and_read_back_a_schedule() {
    let fx = fixture!();
    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "orders")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "create failed: {made}");
    assert_eq!(made["name"], "orders");
    assert_eq!(made["type"], "alert");
    // The creator becomes the owner without being named in the body.
    assert_eq!(made["owners"].as_array().unwrap().len(), 1);
    assert_eq!(made["recipients"].as_array().unwrap().len(), 1);
    assert!(made["next_run_at"].is_string(), "next run is computed");

    let id = made["id"].as_str().unwrap();
    let (s, got) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["sql"], "SELECT count(*) FROM orders");
}

#[tokio::test]
async fn a_viewer_may_read_the_list_but_not_write() {
    // The conjunction matters: asserting only the denial would also pass if the
    // viewer could not reach the module at all, which is a different bug.
    let fx = fixture!();
    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "a member must be able to read the list");

    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "sneaky")),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "creating a schedule requires alert:manage"
    );

    // ...and nothing was written.
    let (_, list) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_viewer_cannot_update_or_delete_someone_elses_schedule() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "guarded")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(alert_json(&fx, "hijacked")),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "DELETE",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, still) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(still["name"], "guarded", "the row must be untouched");
}

#[tokio::test]
async fn a_schedule_of_another_workspace_is_404_not_403() {
    // The editor is a member of BOTH workspaces, so a 403 here would mean the
    // scoping check never ran — and any status other than 404 discloses that
    // the id exists somewhere.
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "elsewhere")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    for (method, body) in [
        ("GET", None),
        ("PUT", Some(alert_json(&fx, "elsewhere"))),
        ("DELETE", None),
    ] {
        let (s, _) = req(
            &fx,
            &fx.editor_tok,
            method,
            &format!("/ws/{}/alerts/{id}", fx.other_slug),
            body,
        )
        .await;
        assert_eq!(
            s,
            StatusCode::NOT_FOUND,
            "{method} must 404 across workspaces"
        );
    }

    // The run log is reached through the schedule, so it must 404 too.
    let (s, _) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}/runs", fx.other_slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_bodies_are_400_with_a_message_naming_the_field() {
    let fx = fixture!();
    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "unparseable cron",
            serde_json::json!({"crontab": "every tuesday"}),
            "invalid crontab",
        ),
        (
            "alert with no sql",
            serde_json::json!({"sql": null}),
            "an alert needs sql",
        ),
        (
            "unknown timezone",
            serde_json::json!({"timezone": "Mars/Olympus"}),
            "unknown timezone",
        ),
        (
            "blank name",
            serde_json::json!({"name": "   "}),
            "name is required",
        ),
    ];

    for (label, patch, expected) in cases {
        let mut body: serde_json::Value =
            serde_json::from_str(&alert_json(&fx, "valid base")).unwrap();
        for (k, v) in patch.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (s, err) = req(
            &fx,
            &fx.editor_tok,
            "POST",
            &format!("/ws/{}/alerts", fx.slug),
            Some(body.to_string()),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{label} should be a 400");
        let msg = err["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains(expected),
            "{label}: expected a message containing {expected:?}, got {msg:?}"
        );
    }
}

#[tokio::test]
async fn an_update_that_names_no_owners_keeps_the_existing_ones() {
    // The editor does not manage owners yet, so it sends none. If "no owners"
    // meant "reset to the creator", every save through the UI would silently
    // drop a co-owner — and owners are not decoration: P2 runs the alert's
    // query with an owner's permissions, so this quietly changes WHOSE access
    // the schedule executes under.
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "co-owned")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    // Give it a second owner directly, standing in for the sharing UI P1 lacks.
    let second = mk_user(&fx.db, "al_second").await;
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(Uuid::parse_str(id).unwrap())
        .bind(second)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    // A normal save from the editor: no `owners` key at all.
    let mut body: serde_json::Value = serde_json::from_str(&alert_json(&fx, "co-owned")).unwrap();
    body["description"] = serde_json::json!("edited");
    assert!(body.get("owners").is_none(), "the editor sends no owners");

    let (s, after) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(body.to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {after}");
    assert_eq!(after["description"], "edited");
    assert_eq!(
        after["owners"].as_array().unwrap().len(),
        2,
        "both owners must survive an edit that did not mention owners: {}",
        after["owners"]
    );
}

#[tokio::test]
async fn an_update_that_does_name_owners_replaces_them() {
    // The other half: when the caller DOES send an owner list, it is honoured
    // in full — otherwise removing a co-owner would be impossible.
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "reassigned")),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let second = mk_user(&fx.db, "al_new_owner").await;
    // §4's owners[] fix: an owner must be a member of the schedule's
    // workspace — the identity `resolve_owner`/`owner_identity` runs the
    // query as, not merely "some user id".
    grant_ws_role(&fx.db, second, fx.ws, "ws_viewer").await;

    let mut body: serde_json::Value = serde_json::from_str(&alert_json(&fx, "reassigned")).unwrap();
    body["owners"] = serde_json::json!([second]);
    let (s, after) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(body.to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {after}");
    let owners = after["owners"].as_array().unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0]["user_id"], second.to_string());
}

#[tokio::test]
async fn a_report_without_a_target_is_400_not_a_constraint_violation() {
    let fx = fixture!();
    let body = serde_json::json!({
        "type": "report",
        "name": "no target",
        "crontab": "0 9 * * *",
        "report_format": "png",
    });
    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(body.to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(err["error"]
        .as_str()
        .unwrap()
        .contains("a report needs a chart_id or a dashboard_id"));
}

#[tokio::test]
async fn a_duplicate_name_is_409_rather_than_500() {
    let fx = fixture!();
    let path = format!("/ws/{}/alerts", fx.slug);
    let (s, _) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &path,
        Some(alert_json(&fx, "twice")),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &path,
        Some(alert_json(&fx, "twice")),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "got {err}");
}

#[tokio::test]
async fn the_list_narrows_to_one_tab_by_type() {
    let fx = fixture!();
    req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "an alert")),
    )
    .await;

    let (s, alerts) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts?type=alert", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(alerts.as_array().unwrap().len(), 1);

    let (_, reports) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts?type=report", fx.slug),
        None,
    )
    .await;
    assert!(
        reports.as_array().unwrap().is_empty(),
        "the Reports tab must not show alerts"
    );
}

#[tokio::test]
async fn the_type_of_an_existing_schedule_cannot_be_changed_by_an_update() {
    // `UpdateSchedule` carries no `type`, so a body claiming one is ignored
    // rather than honoured — flipping an alert into a report would leave a row
    // with an alert's columns and a report's constraints.
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "immutable")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let mut body: serde_json::Value = serde_json::from_str(&alert_json(&fx, "immutable")).unwrap();
    body["type"] = serde_json::json!("report");
    body["report_format"] = serde_json::json!("png");
    let (s, after) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(body.to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {after}");
    assert_eq!(after["type"], "alert", "type must be immutable");
    // `report_format` is NO LONGER a proxy for "the report block was dropped":
    // an alert may legitimately carry one now, as the format of its optional
    // attachment. What still proves the type did not flip is that the
    // alert-only block survived — a report cannot hold these.
    assert!(!after["sql"].is_null(), "the alert kept its query: {after}");
    assert!(!after["validator_type"].is_null());
}

#[tokio::test]
async fn the_run_log_comes_back_newest_first() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "logged")),
    )
    .await;
    let id: Uuid = made["id"].as_str().unwrap().parse().unwrap();
    for (state, ago) in [("error", 60), ("success", 1)] {
        sqlx::query(
            "INSERT INTO automation.schedule_runs (schedule_id, state, started_at) \
             VALUES ($1,$2, now() - ($3 || ' minutes')::interval)",
        )
        .bind(id)
        .bind(state)
        .bind(ago.to_string())
        .execute(&fx.db.pool)
        .await
        .unwrap();
    }

    let (s, runs) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}/runs", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let states: Vec<&str> = runs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["success", "error"]);
}

#[tokio::test]
async fn can_manage_distinguishes_a_viewer_from_an_editor() {
    // The flag decides whether the browser offers Create/Edit/Delete at all.
    // Asserted as a conjunction against the SAME row, because a constant `true`
    // or constant `false` would satisfy either half alone — and a constant
    // `true` is exactly the bug that hands a reader destructive buttons.
    let fx = fixture!();
    req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "flagged")),
    )
    .await;

    let (s, as_editor) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(as_editor[0]["can_manage"], true);

    let (s, as_viewer) = req(
        &fx,
        &fx.viewer_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(as_viewer[0]["can_manage"], false);

    // And the flag must predict the write, or it is decoration: the viewer it
    // says `false` for is actually refused.
    let id = as_viewer[0]["id"].as_str().unwrap();
    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "DELETE",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// ── Run now (P5) ─────────────────────────────────────────────────────────

#[tokio::test]
async fn running_a_schedule_now_writes_a_run_log_row() {
    // The route must go through the SAME `AutomationRunner` the tick uses
    // (`Fx.runner` above), so this asserts on the RUN LOG it wrote — never on
    // the absence of an error. This fixture's database points at a host
    // nothing is listening on, so the expected outcome is a recorded
    // `error`, not a fabricated success; what matters is that the manual run
    // actually executed and logged SOMETHING through the real evaluator.
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "run me")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let (s, run) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts/{id}/run", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {run}");
    assert_eq!(
        run["state"], "error",
        "the fake database refuses the connection"
    );

    let (s, runs) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}/runs", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rows = runs.as_array().unwrap();
    assert_eq!(rows.len(), 1, "the manual run must appear in the run log");
    assert_eq!(rows[0]["id"], run["id"], "same row the route returned");
}

#[tokio::test]
async fn running_a_schedule_records_its_last_state_but_not_a_new_next_run() {
    // The manual-run contract from `runner.rs`: `run_now` must not call
    // `release`, so the schedule's own cadence is untouched by a click — but
    // its outcome is the schedule's last state (KI-032), or the list and the
    // detail badge keep saying "Never run" above a run log that shows runs.
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "cadence")),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let before = made["next_run_at"].as_str().unwrap().to_string();
    assert!(made["last_state"].is_null(), "never run yet: {made}");

    let (_, run) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts/{id}/run", fx.slug),
        None,
    )
    .await;

    let (_, after) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(
        after["next_run_at"].as_str().unwrap(),
        before,
        "a manual run must not change next_run_at"
    );
    assert_eq!(
        after["last_state"], run["state"],
        "the manual run's outcome is the schedule's last state: {after}"
    );
    let (_, list) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .expect("the schedule is listed");
    assert_eq!(row["last_state"], run["state"], "the list agrees: {row}");
}

#[tokio::test]
async fn a_viewer_cannot_run_a_schedule() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "guarded run")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "POST",
        &format!("/ws/{}/alerts/{id}/run", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "run now requires alert:manage");

    let (_, runs) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}/runs", fx.slug),
        None,
    )
    .await;
    assert!(
        runs.as_array().unwrap().is_empty(),
        "a forbidden request must not have run anything"
    );
}

#[tokio::test]
async fn running_a_schedule_of_another_workspace_is_404() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "elsewhere run")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let (s, _) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts/{id}/run", fx.other_slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

// ── Active toggle (P5) ──────────────────────────────────────────────────

#[tokio::test]
async fn toggling_active_flips_only_that_field() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "togglable")),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    assert_eq!(made["active"], true);

    let (s, after) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}/active", fx.slug),
        Some(serde_json::json!({"active": false}).to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {after}");
    assert_eq!(after["active"], false);
    assert_eq!(after["name"], "togglable", "nothing else must change");

    let (_, detail) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(detail["active"], false);
    assert_eq!(
        detail["sql"], "SELECT count(*) FROM orders",
        "the rest of the row must be untouched by the toggle"
    );
}

#[tokio::test]
async fn a_viewer_cannot_toggle_active() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "guarded toggle")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let (s, _) = req(
        &fx,
        &fx.viewer_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}/active", fx.slug),
        Some(serde_json::json!({"active": false}).to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (_, still) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(
        still["active"], true,
        "the viewer's request must not have applied"
    );
}

#[tokio::test]
async fn toggling_active_of_another_workspace_is_404() {
    let fx = fixture!();
    let (_, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "elsewhere toggle")),
    )
    .await;
    let id = made["id"].as_str().unwrap();

    let (s, _) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}/active", fx.other_slug),
        Some(serde_json::json!({"active": false}).to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unauthenticated_request_is_rejected() {
    let fx = fixture!();
    let (s, _) = req(
        &fx,
        "not-a-session-token",
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert!(
        s == StatusCode::UNAUTHORIZED || s == StatusCode::FORBIDDEN,
        "expected an auth rejection, got {s}"
    );
}

/// The webhook signing secret must never travel back to a client.
///
/// Same rule, and the same assertion style, as the SMTP password (P3): checked
/// against the RAW body of every response that carries a schedule, because a
/// secret nested inside `config` still satisfies a `v["secret"].is_none()`.
fn alert_with_slack(fx: &Fx, name: &str, token: &str) -> String {
    serde_json::json!({
        "type": "alert",
        "name": name,
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "database_id": fx.database,
        "sql": "SELECT count(*) FROM orders",
        "validator_type": "not_null",
        "recipients": [{
            "kind": "slack",
            "config": {"channel": "#data-alerts", "token": token},
        }],
    })
    .to_string()
}

/// A workspace member holding a bespoke role with NO alert permissions.
///
/// Deliberately a new role rather than revoking `alert:read` from `ws_viewer`:
/// these tests share one database, and the first version of this test did
/// exactly that — it passed once, left the seeded role permanently stripped,
/// and failed on every run afterwards while also breaking anything else using
/// that database. A test that mutates shared reference data has to put it
/// back, and one that panics mid-way never does.
async fn member_without_alert_perms(db: &Db, ws: Uuid) -> String {
    let user = mk_user(db, "noalert").await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) \
         VALUES ('workspace', $1, 'test: workspace member with no alert permissions') \
         RETURNING id",
    )
    .bind(format!("ws_test_noalert_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("create role");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(user)
    .bind(role)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("grant");
    user.to_string()
}

/// Reading a schedule needs `alert:read`.
///
/// Until this gate existed, `detail` and `runs` had NO authorization check at
/// all: workspace membership plus the `alerts` entitlement handed out the SQL,
/// the database id, the recipient addresses and the engine errors in the run
/// log. Every write was gated on `alert:manage`; the reads were gated on
/// nothing, and `alert:read` — added by migration 0113 for the folder router —
/// was checked nowhere on alerts.
#[tokio::test]
async fn reading_a_schedule_needs_alert_read() {
    let fx = fixture!();

    let (s, created) = req_raw(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_with_webhook(&fx, "gated", "s3cret")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "create failed: {created}");
    let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A viewer holds `alert:read` (0113 granted it to all three seeded roles),
    // so the access ordinary members already had is unchanged.
    for path in [
        format!("/ws/{}/alerts", fx.slug),
        format!("/ws/{}/alerts/{id}", fx.slug),
        format!("/ws/{}/alerts/{id}/runs", fx.slug),
    ] {
        let (s, body) = req_raw(&fx, &fx.viewer_tok, "GET", &path, None).await;
        assert_eq!(s, StatusCode::OK, "viewer denied {path}: {body}");
    }

    // A member of the SAME workspace whose role carries no alert permissions is
    // refused. That is the measurement that matters: membership plus the
    // `alerts` entitlement was all these routes ever required, so a principal
    // who has both and still cannot read is proof the permission is the gate.
    let outsider = member_without_alert_perms(&fx.db, fx.ws).await;
    let outsider_tok = fx.token_for(outsider.parse().unwrap()).await;
    for path in [
        format!("/ws/{}/alerts", fx.slug),
        format!("/ws/{}/alerts/{id}", fx.slug),
        format!("/ws/{}/alerts/{id}/runs", fx.slug),
    ] {
        let (s, body) = req_raw(&fx, &outsider_tok, "GET", &path, None).await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "a member without alert:read still read {path}: {body}"
        );
        assert!(
            !body.contains("SELECT count(*)"),
            "the refusal still carried the SQL: {body}"
        );
    }
}

/// A slack bot token is a credential, and must report itself exactly the way a
/// webhook secret does.
///
/// It did not: `redact_recipient_config` special-cased webhooks and returned
/// every other kind verbatim, so a slack recipient shipped `token_id` — the
/// vault reference the webhook branch deliberately hides — and never shipped
/// `has_token`, which the editor reads to decide whether to say a token is
/// stored. The editor therefore always claimed none was.
#[tokio::test]
async fn a_slack_token_reports_itself_like_a_webhook_secret() {
    const TOKEN: &str = "xoxb-not-in-any-response";
    let fx = fixture!();

    let (s, created) = req_raw(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_with_slack(&fx, "to slack", TOKEN)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "create failed: {created}");
    let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (s, got) = req_raw(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Asserted against the RAW body: a token nested inside `config` still
    // satisfies a struct-field check.
    assert!(!got.contains(TOKEN), "the response leaks the token: {got}");
    assert!(
        !got.contains("token_id"),
        "the response hands out the vault reference: {got}"
    );

    let v: serde_json::Value = serde_json::from_str(&got).unwrap();
    let slack = v["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "slack")
        .expect("the slack recipient survived");
    assert_eq!(slack["config"]["channel"], "#data-alerts");
    assert_eq!(
        slack["config"]["has_token"], true,
        "the editor cannot tell a token is stored: {got}"
    );
}

/// `secret_id` is withheld too — a vault reference is not a thing the editor
/// needs, and handing one out invites a fetch nobody intended.
#[tokio::test]
async fn a_webhook_secret_never_appears_in_any_response() {
    const SECRET: &str = "sup3r-s3cret-hmac-key";
    let fx = fixture!();

    let (s, created) = req_raw(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_with_webhook(&fx, "signed", SECRET)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "create failed: {created}");
    assert!(
        !created.contains(SECRET),
        "the POST response echoed the secret back: {created}"
    );

    let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, got) = req_raw(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        !got.contains(SECRET),
        "the GET response leaks the secret: {got}"
    );
    assert!(
        !got.contains("secret_id"),
        "the GET response hands out the vault reference: {got}"
    );

    // ...but the client can still tell that signing is configured.
    let v: serde_json::Value = serde_json::from_str(&got).unwrap();
    let hook = v["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "webhook")
        .expect("the webhook recipient survived");
    assert_eq!(hook["config"]["url"], "https://hooks.example.com/egret");
    assert_eq!(hook["config"]["has_secret"], true);
    assert!(hook["config"].get("secret").is_none());
}

/// The secret is not merely hidden — it is not in the row.
#[tokio::test]
async fn a_webhook_secret_is_not_stored_in_the_recipient_row() {
    const SECRET: &str = "not-in-the-table";
    let fx = fixture!();
    let (s, created) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_with_webhook(&fx, "at-rest", SECRET)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "create failed: {created}");
    // Scoped to the id just created, not the name: every run makes its own
    // workspace, so a name lookup can pick up a row from an earlier run.
    let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();

    let config: serde_json::Value = sqlx::query_scalar(
        "SELECT config FROM automation.schedule_recipients \
         WHERE schedule_id = $1 AND kind = 'webhook'",
    )
    .bind(id)
    .fetch_one(&fx.db.pool)
    .await
    .expect("the webhook row exists");
    assert!(
        !config.to_string().contains(SECRET),
        "cleartext secret at rest in schedule_recipients.config: {config}"
    );
    assert!(
        config.get("secret_id").is_some(),
        "the row should reference a vault entry instead: {config}"
    );
}

/// An update that does not carry a secret keeps the stored one.
///
/// The editor cannot render a secret it is never sent, so its blank field must
/// not mean "remove" — the same three-way rule the SMTP password follows.
#[tokio::test]
async fn saving_a_webhook_without_a_secret_keeps_the_stored_one() {
    const SECRET: &str = "keep-me-through-a-save";
    let fx = fixture!();
    let (s, created) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_with_webhook(&fx, "kept", SECRET)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap().to_string();

    // What the editor actually sends back: the URL, no secret.
    let body = serde_json::json!({
        "name": "kept",
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "database_id": fx.database,
        "sql": "SELECT count(*) FROM orders",
        "validator_type": "not_null",
        "recipients": [{
            "kind": "webhook",
            "config": {"url": "https://hooks.example.com/egret"},
        }],
    })
    .to_string();
    let (s, updated) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "update failed: {updated}");

    let hook = updated["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "webhook")
        .expect("the webhook survived the save");
    assert_eq!(
        hook["config"]["has_secret"], true,
        "a save with no secret field silently unsigned the webhook: {updated}"
    );
}

// ── SEC-2: a schedule may only name objects in its own workspace ──────────
//
// Task 1 made the READ safe: a foreign chart/dashboard simply does not
// resolve. These prove the WRITE side refuses a foreign target at save, so
// the normal experience is a 400 rather than a schedule that silently starts
// failing "no longer exists" the next time it ticks.
//
// Every principal below is `fx.editor_tok` — a `ws_editor`, not an admin —
// which is the role migration 0110 grants `alert:manage` to, and the one
// that actually separates "may manage schedules" from "may see the target
// object". `editor` is ALSO a `ws_editor` of `fx.other_ws` (see `setup`),
// which makes this a STRONGER proof than requiring zero grant there: even a
// principal who can manage schedules in both workspaces cannot point one
// workspace's schedule at the other's object.

/// A report body. `targets` rides alongside `chart_id`/`dashboard_id` because
/// `to_new` requires a `chart_id` or `dashboard_id` on every report
/// regardless of whether `targets` is used — so the `schedule_targets` tests
/// below still need a legitimate same-workspace `chart_id` for the body to
/// pass THAT check before reaching the one this file is testing.
fn report_json(
    name: &str,
    format: &str,
    chart_id: Option<Uuid>,
    dashboard_id: Option<Uuid>,
    targets: &[(&str, Uuid)],
) -> String {
    let mut body = serde_json::json!({
        "type": "report",
        "name": name,
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "report_format": format,
        "recipients": [{"kind": "email", "config": {"addresses": ["dest@t.local"]}}],
        "targets": targets
            .iter()
            .map(|(k, id)| serde_json::json!({"kind": k, "object_id": id}))
            .collect::<Vec<_>>(),
    });
    if let Some(id) = chart_id {
        body["chart_id"] = serde_json::json!(id);
    }
    if let Some(id) = dashboard_id {
        body["dashboard_id"] = serde_json::json!(id);
    }
    body.to_string()
}

#[tokio::test]
async fn create_refuses_a_report_naming_another_workspaces_chart() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let foreign_chart = mk_chart(&fx.db, fx.other_ws, owner).await;

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "foreign chart",
            "csv",
            Some(foreign_chart),
            None,
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("chart_id"),
        "refusal should name the field: {msg}"
    );
}

#[tokio::test]
async fn create_refuses_a_report_naming_another_workspaces_dashboard() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let foreign_dashboard = mk_dashboard(&fx.db, fx.other_ws, owner).await;

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "foreign dashboard",
            "png",
            None,
            Some(foreign_dashboard),
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("dashboard_id"),
        "refusal should name the field: {msg}"
    );
}

#[tokio::test]
async fn update_refuses_repointing_a_report_at_another_workspaces_target() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    let foreign_chart = mk_chart(&fx.db, fx.other_ws, owner).await;
    let foreign_dashboard = mk_dashboard(&fx.db, fx.other_ws, owner).await;

    // Seeded legitimately, with the schedule's own workspace's chart.
    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "repointable",
            "csv",
            Some(own_chart),
            None,
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    // Repoint at the other workspace's chart.
    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(report_json(
            "repointable",
            "csv",
            Some(foreign_chart),
            None,
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "chart repoint: got {err}");
    assert!(err["error"]
        .as_str()
        .unwrap_or_default()
        .contains("chart_id"));

    // Repoint at the other workspace's dashboard instead.
    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(report_json(
            "repointable",
            "png",
            None,
            Some(foreign_dashboard),
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "dashboard repoint: got {err}");
    assert!(err["error"]
        .as_str()
        .unwrap_or_default()
        .contains("dashboard_id"));

    // And the schedule was NOT changed by either rejected attempt.
    let (_, still) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(still["chart_id"], own_chart.to_string());
}

#[tokio::test]
async fn create_refuses_a_schedule_targets_entry_naming_another_workspaces_chart() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    let foreign_chart = mk_chart(&fx.db, fx.other_ws, owner).await;

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        // `chart_id` is workspace 1's OWN, legitimate chart — a patch that
        // only gated `chart_id`/`dashboard_id` would wave this straight
        // through, which is exactly the probe's Surface 1 finding.
        Some(report_json(
            "multi target chart",
            "png",
            Some(own_chart),
            None,
            &[("chart", foreign_chart)],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("targets"),
        "refusal should name the field: {msg}"
    );
}

#[tokio::test]
async fn create_refuses_a_schedule_targets_entry_naming_another_workspaces_dashboard() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    let foreign_dashboard = mk_dashboard(&fx.db, fx.other_ws, owner).await;

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "multi target dashboard",
            "png",
            Some(own_chart),
            None,
            &[("dashboard", foreign_dashboard)],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("targets"),
        "refusal should name the field: {msg}"
    );
}

#[tokio::test]
async fn create_refuses_a_schedule_targets_entry_whose_object_id_exists_nowhere() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    // `schedule_targets.object_id` has no foreign key — this id was never
    // inserted anywhere, in any workspace. The probe proved it stores fine
    // with no fix; it must be refused with one.
    let nonexistent = Uuid::new_v4();

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "phantom target",
            "png",
            Some(own_chart),
            None,
            &[("chart", nonexistent)],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("targets"),
        "refusal should name the field: {msg}"
    );
}

/// The negative control. A fix that refuses the working case is not a fix.
#[tokio::test]
async fn create_still_saves_a_same_workspace_target() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    let own_dashboard = mk_dashboard(&fx.db, fx.ws, owner).await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "legit multi target",
            "png",
            Some(own_chart),
            None,
            &[("chart", own_chart), ("dashboard", own_dashboard)],
        )),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a same-workspace target must save: {made}"
    );
    assert_eq!(made["chart_id"], own_chart.to_string());
}

/// Context item 3: alerts, not just reports. `model.rs`'s alert branch never
/// required a target and never checked ownership — an alert carrying a
/// foreign `chart_id` reaches `render_alert_attachment` unchanged. The same
/// gate must cover it.
#[tokio::test]
async fn create_refuses_an_alert_naming_another_workspaces_chart() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let foreign_chart = mk_chart(&fx.db, fx.other_ws, owner).await;

    let body = serde_json::json!({
        "type": "alert",
        "name": "alert with foreign attachment",
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "database_id": fx.database,
        "sql": "SELECT count(*) FROM orders",
        "validator_type": "not_null",
        "chart_id": foreign_chart,
        "report_format": "png",
        "recipients": [{"kind": "email", "config": {"addresses": ["on-call@t.local"]}}],
    })
    .to_string();

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    assert!(err["error"]
        .as_str()
        .unwrap_or_default()
        .contains("chart_id"));
}

/// Fix-round-1 gap: the six new tests above prove `schedule_targets` is
/// checked on CREATE, but `update` rewrites `schedule_targets` too (the
/// DELETE-then-`write_children` in `ScheduleRepo::update`), and nothing
/// pinned that the same gate runs there. Mutating the targets check away
/// only failed the create-side tests — a refactor that moved the check into
/// `create` alone would have left every one of those green.
#[tokio::test]
async fn update_refuses_repointing_schedule_targets_at_another_workspaces_chart() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    let foreign_chart = mk_chart(&fx.db, fx.other_ws, owner).await;

    // Seeded legitimately: `chart_id` is workspace 1's own, and `targets` is
    // empty — nothing yet for the repoint to disturb.
    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "targets repointable",
            "png",
            Some(own_chart),
            None,
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    // Same `chart_id`, but now a `targets` entry naming the OTHER
    // workspace's chart — the door a `chart_id`-only check would miss.
    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(report_json(
            "targets repointable",
            "png",
            Some(own_chart),
            None,
            &[("chart", foreign_chart)],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("targets"),
        "refusal should name the field: {err}"
    );

    // And the rejected update did not partially apply: still no targets.
    let (_, still) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(
        still["targets"].as_array().unwrap().len(),
        0,
        "a rejected update must not have written the foreign target: {still}"
    );
}

/// The update-side negative control. A same-workspace `schedule_targets`
/// entry must still save on UPDATE, not just on create.
#[tokio::test]
async fn update_still_saves_a_same_workspace_schedule_targets_entry() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_chart = mk_chart(&fx.db, fx.ws, owner).await;
    let own_dashboard = mk_dashboard(&fx.db, fx.ws, owner).await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(report_json(
            "targets addable",
            "png",
            Some(own_chart),
            None,
            &[],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s, updated) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(report_json(
            "targets addable",
            "png",
            Some(own_chart),
            None,
            &[("chart", own_chart), ("dashboard", own_dashboard)],
        )),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a same-workspace update-side target must save: {updated}"
    );
    let targets = updated["targets"].as_array().unwrap();
    assert_eq!(
        targets.len(),
        2,
        "both targets should have been written: {updated}"
    );
}

// ── the database_id sibling (round 4, F1) ───────────────────────────────────

/// A `query.databases` row in `ws`, otherwise identical to the one `setup()`
/// creates for `fx.database` — used to build a SECOND, foreign database for
/// the cross-workspace tests below.
async fn mk_database(db: &Db, ws: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

/// An alert body with an explicit `database_id` and `owners` list — the two
/// fields `alert_json` leaves at their defaults (the fixture's own database,
/// and an empty `owners` that falls back to the creator).
fn alert_json_full(name: &str, database_id: Uuid, owners: &[Uuid]) -> String {
    serde_json::json!({
        "type": "alert",
        "name": name,
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "database_id": database_id,
        "sql": "SELECT count(*) FROM orders",
        "validator_type": "not_null",
        "owners": owners,
        "recipients": [{"kind": "email", "config": {"addresses": ["on-call@t.local"]}}],
    })
    .to_string()
}

#[tokio::test]
async fn create_refuses_an_alert_naming_another_workspaces_database() {
    let fx = fixture!();
    let foreign_database = mk_database(&fx.db, fx.other_ws).await;

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("foreign database", foreign_database, &[])),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("database_id"),
        "refusal should name the field: {msg}"
    );
}

#[tokio::test]
async fn create_refuses_an_alert_naming_a_database_that_exists_nowhere() {
    // Foreign and nonexistent must be literally indistinguishable — the same
    // rule the chart/dashboard/targets checks already follow — so this
    // endpoint cannot become an existence oracle over `query.databases`
    // either.
    let fx = fixture!();
    let foreign_database = mk_database(&fx.db, fx.other_ws).await;
    let ghost = Uuid::new_v4();

    let (s1, foreign_err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("foreign db oracle", foreign_database, &[])),
    )
    .await;
    let (s2, ghost_err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("ghost db oracle", ghost, &[])),
    )
    .await;
    assert_eq!(s1, StatusCode::BAD_REQUEST, "got {foreign_err}");
    assert_eq!(s2, StatusCode::BAD_REQUEST, "got {ghost_err}");
    assert_eq!(
        s1, s2,
        "status must be identical for foreign vs nonexistent"
    );
    assert_eq!(
        foreign_err["error"], ghost_err["error"],
        "body must be identical for foreign vs nonexistent — otherwise this \
         endpoint is an existence oracle over query.databases"
    );
}

#[tokio::test]
async fn create_still_saves_a_same_workspace_database() {
    // Negative control: the scoping check must not also refuse the
    // legitimate case.
    let fx = fixture!();

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("own database", fx.database, &[])),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {made}");
    assert_eq!(made["database_id"], fx.database.to_string());
}

#[tokio::test]
async fn update_refuses_repointing_an_alert_at_another_workspaces_database() {
    let fx = fixture!();
    let foreign_database = mk_database(&fx.db, fx.other_ws).await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("repointable db", fx.database, &[])),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(alert_json_full("repointable db", foreign_database, &[])),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    assert!(err["error"]
        .as_str()
        .unwrap_or_default()
        .contains("database_id"));

    let (_, still) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    assert_eq!(
        still["database_id"],
        fx.database.to_string(),
        "a rejected repoint must not have written the foreign database_id"
    );
}

// ── owners[] must name a member of this workspace ───────────────────────────

#[tokio::test]
async fn create_refuses_an_owner_who_is_not_a_member_of_this_workspace() {
    let fx = fixture!();
    // A user with NO grant anywhere — not even on `fx.other_ws` — so this is
    // unambiguously "not a member", the same shape the probe's "stranger" was.
    let stranger = mk_user(&fx.db, "stranger").await;

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("stranger owner", fx.database, &[stranger])),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("owners"),
        "refusal should name the field: {msg}"
    );
}

#[tokio::test]
async fn create_still_saves_a_member_owner() {
    // Negative control: a `ws_viewer` of `fx.ws` is a member — membership is
    // all `owners[]` requires, not `alert:manage` (that stays gated on the
    // ACTOR making the request, which is the editor token here).
    let fx = fixture!();
    let member = mk_user(&fx.db, "member_owner").await;
    grant_ws_role(&fx.db, member, fx.ws, "ws_viewer").await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("member owner", fx.database, &[member])),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {made}");
    let owners = made["owners"].as_array().unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0]["user_id"], member.to_string());
}

#[tokio::test]
async fn update_refuses_repointing_owners_at_a_non_member() {
    let fx = fixture!();
    let member = mk_user(&fx.db, "member_owner").await;
    grant_ws_role(&fx.db, member, fx.ws, "ws_viewer").await;
    let stranger = mk_user(&fx.db, "stranger").await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full("owner repointable", fx.database, &[member])),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(alert_json_full(
            "owner repointable",
            fx.database,
            &[stranger],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    assert!(err["error"].as_str().unwrap_or_default().contains("owners"));

    let (_, still) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        None,
    )
    .await;
    let owners = still["owners"].as_array().unwrap();
    assert_eq!(
        owners.len(),
        1,
        "a rejected owner repoint must not have replaced the existing owner"
    );
    assert_eq!(owners[0]["user_id"], member.to_string());
}

// ── owners[] must be checked against what the caller NAMED, not a fallback ──

/// Round-1 review regression #1: a `platform_admin` reaches every workspace
/// through `WsContext`'s bypass (no `access_grants` row on the workspace
/// itself), so an EMPTY `owners` body falling back to them as the creator
/// must never be run through the membership check — a fallback the server
/// chose for itself is not a naming.
#[tokio::test]
async fn a_platform_admin_who_is_not_a_workspace_member_can_create_a_schedule() {
    let fx = fixture!();
    let admin = mk_user(&fx.db, "platform_admin").await;
    grant_platform_role(&fx.db, admin, "platform_admin").await;
    let admin_tok = fx.token_for(admin).await;

    let (s, made) = req(
        &fx,
        &admin_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        // No `owners` in the body — falls back to the creator (`admin`),
        // who holds no `access_grants` row on `fx.ws` at all.
        Some(alert_json(&fx, "platform admin create")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {made}");
    let owners = made["owners"].as_array().unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0]["user_id"], admin.to_string());
}

#[tokio::test]
async fn a_platform_admin_who_is_not_a_workspace_member_can_update_a_schedule() {
    let fx = fixture!();
    let admin = mk_user(&fx.db, "platform_admin").await;
    grant_platform_role(&fx.db, admin, "platform_admin").await;
    let admin_tok = fx.token_for(admin).await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "platform admin update target")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s, updated) = req(
        &fx,
        &admin_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        // Still no `owners` — an update's empty body falls back to the
        // SCHEDULE's existing owners (the editor), never to the platform
        // admin doing the PUT, but `to_new` computes an intermediate
        // `stored.created_by` fallback first; that intermediate value must
        // not be checked either.
        Some(alert_json(&fx, "platform admin update target")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {updated}");
}

/// Round-1 review regression #2: `update`'s "empty owners means keep the
/// existing ones" branch replaces `new.owners` with the STORED owners
/// *after* validation used to run — so the check was validating a value
/// (`stored.created_by`) that was never written, while the value actually
/// written (the stored owners) went unchecked. Proven here the other
/// direction: an update that does not touch `owners` must keep succeeding
/// even once the stored owner has left the workspace, and must keep writing
/// that same (now-foreign) stored owner back — not silently reassign it to
/// whoever is calling.
#[tokio::test]
async fn an_update_that_does_not_touch_owners_still_succeeds_once_the_stored_owner_has_left() {
    let fx = fixture!();
    let member = mk_user(&fx.db, "departed_owner").await;
    grant_ws_role(&fx.db, member, fx.ws, "ws_viewer").await;

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json_full(
            "owner then departs",
            fx.database,
            &[member],
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    revoke_ws_role(&fx.db, member, fx.ws).await;

    let (s, updated) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        // No `owners` field — not touching them — on every other axis just a
        // routine edit (a changed crontab, say the same body is fine since
        // nothing else about `alert_json_full` changed).
        Some(alert_json_full("owner then departs", fx.database, &[])),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "an update that does not touch owners must not be refused just \
         because the EXISTING owner has since left the workspace: {updated}"
    );
    let owners = updated["owners"].as_array().unwrap();
    assert_eq!(
        owners.len(),
        1,
        "the stored owner must still be written back, unchanged: {updated}"
    );
    assert_eq!(
        owners[0]["user_id"],
        member.to_string(),
        "must keep the departed member as owner, not silently reassign to the caller"
    );
}

// ── set_folder: a foreign or nonexistent folder must be refused identically ─

/// `folder_id` rides on `ScheduleSummaryDto` (the list), not
/// `ScheduleDetailDto` (the single-schedule GET) — so verifying a `set_folder`
/// write means reading it back through the list, not the detail route.
async fn folder_of(fx: &Fx, id: &str) -> serde_json::Value {
    let (_, list) = req(
        fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    list.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("schedule {id} not in list: {list}"))["folder_id"]
        .clone()
}

#[tokio::test]
async fn moving_a_schedule_into_another_workspaces_folder_is_refused() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let foreign_folder = egret_bi::folder::FolderRepo::new(fx.db.clone())
        .create(fx.other_ws, "alert", "other ws folder", Some(owner))
        .await
        .expect("create foreign folder");

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "folder move target")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s, err) = req(
        &fx,
        &fx.editor_tok,
        "PATCH",
        &format!("/ws/{}/alerts/{id}/folder", fx.slug),
        Some(serde_json::json!({"folder_id": foreign_folder.id}).to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "got {err}");
    let msg = err["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("folder_id"),
        "refusal should name the field: {msg}"
    );

    assert!(
        folder_of(&fx, id).await.is_null(),
        "a rejected move must not have written the foreign folder_id"
    );
}

#[tokio::test]
async fn moving_a_schedule_into_a_nonexistent_folder_is_refused_identically() {
    // Before the fix, a foreign folder was a 200 and a nonexistent one was a
    // 500 from `folders_id_fkey` — two different status CLASSES, which made
    // this endpoint an existence oracle over ALL of `analytics.folders`
    // (charts, dashboards and datasets included), one call per guess, open to
    // any `ws_editor`. Both must now be refused the SAME way.
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let foreign_folder = egret_bi::folder::FolderRepo::new(fx.db.clone())
        .create(fx.other_ws, "alert", "other ws folder 2", Some(owner))
        .await
        .expect("create foreign folder");
    let ghost = Uuid::new_v4();

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "folder oracle target")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s1, foreign_err) = req(
        &fx,
        &fx.editor_tok,
        "PATCH",
        &format!("/ws/{}/alerts/{id}/folder", fx.slug),
        Some(serde_json::json!({"folder_id": foreign_folder.id}).to_string()),
    )
    .await;
    let (s2, ghost_err) = req(
        &fx,
        &fx.editor_tok,
        "PATCH",
        &format!("/ws/{}/alerts/{id}/folder", fx.slug),
        Some(serde_json::json!({"folder_id": ghost}).to_string()),
    )
    .await;
    assert_eq!(s1, StatusCode::BAD_REQUEST, "got {foreign_err}");
    assert_eq!(s2, StatusCode::BAD_REQUEST, "got {ghost_err}");
    assert_eq!(
        s1, s2,
        "status must be identical for foreign vs nonexistent"
    );
    assert_eq!(
        foreign_err["error"], ghost_err["error"],
        "body must be identical for foreign vs nonexistent — otherwise this \
         endpoint is an existence oracle over analytics.folders"
    );
    // And neither a foreign-folder guess (200 before the fix) nor a
    // nonexistent one (500 from the FK before the fix) may be distinguished
    // by status CLASS either.
    assert_eq!(s1, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn moving_a_schedule_into_its_own_workspaces_folder_still_works() {
    // Negative control: the scoping check must not also refuse the
    // legitimate case.
    let fx = fixture!();
    let owner = mk_user(&fx.db, "target_owner").await;
    let own_folder = egret_bi::folder::FolderRepo::new(fx.db.clone())
        .create(fx.ws, "alert", "my folder", Some(owner))
        .await
        .expect("create own folder");

    let (s, made) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(alert_json(&fx, "folder move own")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "seed create failed: {made}");
    let id = made["id"].as_str().unwrap();

    let (s, ack) = req(
        &fx,
        &fx.editor_tok,
        "PATCH",
        &format!("/ws/{}/alerts/{id}/folder", fx.slug),
        Some(serde_json::json!({"folder_id": own_folder.id}).to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "got {ack}");
    assert_eq!(folder_of(&fx, id).await, own_folder.id.to_string());
}

// ── Task 4: dataapi_alert authorization ─────────────────────────────────

/// A brand-new member holding the SEEDED `ws_admin` role (migration 0122
/// grants it both `data_api_endpoint:read` and `:manage`, alongside the
/// `alert:manage`/`alert:read` migration 0110/0113 already give it).
///
/// Deliberately NOT a freshly-`INSERT`-ed custom role: `CedarPdp` snapshots
/// role NAMES/templates at `build()` time (`egret-auth/src/pdp/mod.rs`) and
/// only reloads on a `RolesChanged` event this file's `setup()` has no way to
/// publish (its `EventBus` is a throwaway temporary — see `setup()`'s own
/// `CedarPdp::build` call). A role created by raw SQL after `fx` exists is
/// therefore invisible to `fx.app`'s PDP and every grant through it is
/// silently a no-op, which would make BOTH assertions below pass — the
/// "denied" one for the wrong reason (unrecognised role, not the missing
/// permission) and the "allowed" one not at all. `ws_admin`/`ws_editor` are
/// seeded by migrations, so they exist before `setup()` ever calls `build()`;
/// only the ACCESS GRANT (this function's `INSERT INTO access_grants`) is
/// new, and grants ARE read live per `authorize()` call
/// (`self.grants.grants_for(...)`), never cached. `egret-dataapi/tests/
/// lifecycle.rs`'s `principal_with_in` sidesteps the same trap the other
/// way, by building a FRESH `CedarPdp` after creating its custom role — not
/// available here without rebuilding `fx.app`'s whole `AutomationState`.
async fn admin_member(db: &Db, ws: Uuid) -> Uuid {
    let user = mk_user(db, "dataapi_alert_authz_admin").await;
    grant_ws_role(db, user, ws, "ws_admin").await;
    user
}

/// An OBJECT-scoped `data_api_endpoint:read` grant on exactly `endpoint_id`
/// — no workspace-level Data API permission at all. Links the SEEDED
/// `ws_admin` role's template (same PDP-staleness reasoning as
/// `admin_member` above: the ROLE must already be known, only the GRANT is
/// new) but binds it to the OBJECT, not the workspace: `templates.rs`
/// compiles every workspace-scope ROLE's template as `resource in
/// ?resource` regardless of what scope_type a later GRANT uses it at, and an
/// `?resource` bound to the endpoint's own entity makes `in` match only that
/// entity (an endpoint has no descendants) — an object-scoped-only grant in
/// every practical sense, using a role Cedar already has a template for.
///
/// This is what makes `an_endpoint_scoped_grant_with_no_workspace_read_is_
/// enough` below a REAL positive control for `OBJECT_KIND`: the grant's
/// `object_type` is the literal the review asked to pin, and the only way
/// this user's PUT succeeds is if the CHECK builds the exact same string
/// into the resource it asks the PDP about.
async fn grant_object_scoped_data_api_read(db: &Db, user: Uuid, endpoint_id: Uuid) {
    let ws_admin_role: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'")
            .fetch_one(&db.pool)
            .await
            .expect("ws_admin role must be seeded");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, object_type) \
         VALUES ('user', $1, $2, 'object', $3, 'data_api_endpoint')",
    )
    .bind(user)
    .bind(ws_admin_role)
    .bind(endpoint_id)
    .execute(&db.pool)
    .await
    .expect("grant object-scoped data_api_endpoint:read");
}

/// A WORKSPACE-scoped `dataapi_alert` (`dataapi_endpoint_id IS NULL` — it
/// watches every endpoint) — inserted directly, the same way
/// `dataapi_alert_tick.rs`'s fixture does, since `CreateSchedule` cannot
/// express this type yet (Task 5). `next_run_at` is left in the past so a
/// row exists to update; nothing here ticks it.
async fn insert_workspace_scoped_dataapi_alert(db: &Db, ws: Uuid, owner: Uuid, name: &str) -> Uuid {
    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, validator_type, validator_config, \
            dataapi_metric, dataapi_window_s) \
         VALUES ($1,'dataapi_alert',$2,'* * * * *',now() - interval '1 minute', \
                 'operator', $3, 'call_count', 3600) RETURNING id",
    )
    .bind(ws)
    .bind(name)
    .bind(serde_json::json!({"op": ">", "threshold": 0}))
    .fetch_one(&db.pool)
    .await
    .expect("insert dataapi_alert");
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&db.pool)
        .await
        .expect("insert owner");
    schedule
}

/// A minimal, validly-shaped `UpdateSchedule` body. Its CONTENT is
/// irrelevant to the tests below — the save-time gate runs on the STORED
/// row's type, before this body is ever validated against it — only its
/// SHAPE matters, so `Json<UpdateSchedule>` extracts successfully and the
/// handler runs at all.
fn minimal_update_body() -> String {
    serde_json::json!({
        "name": "renamed",
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
    })
    .to_string()
}

/// Same as [`minimal_update_body`], but naming a `dataapi_endpoint_id` —
/// fix-round-1's second check on `update()` gates the BODY's target endpoint
/// as well as the stored one, so a body that omits the field now (correctly)
/// asks to WIDEN the alert to the whole workspace rather than "leave it
/// alone". Tests that want to prove "an object-scoped grant on the CURRENT
/// endpoint is enough" have to say so explicitly, or they are actually
/// testing the widen-by-omission case instead.
///
/// A FULL, otherwise-valid `dataapi_alert` body (fix-round-3) — not the
/// bare `name`/`active`/`crontab`/`timezone` `minimal_update_body` gets away
/// with. A body missing `dataapi_metric` fails `to_new`'s validation before
/// ever reaching a write, which made every "no partial write" assertion
/// built on this helper vacuous: the write was never going to happen anyway,
/// mutated check or not. A caller sending this body would succeed if the
/// authz check under test let them through, so a "no partial write"
/// assertion built on it actually demonstrates the check stopped a write
/// that would otherwise have gone through — the same standard
/// `creating_a_dataapi_alert_needs_data_api_endpoint_read`'s ALLOWED branch
/// holds the create path to.
fn minimal_update_body_with_endpoint(endpoint_id: Uuid) -> String {
    serde_json::json!({
        "name": "renamed",
        "active": true,
        "crontab": "0 9 * * *",
        "timezone": "UTC",
        "validator_type": "operator",
        "validator_config": {"op": ">", "threshold": 0},
        "dataapi_metric": "call_count",
        "dataapi_window_s": 300,
        "dataapi_endpoint_id": endpoint_id,
    })
    .to_string()
}

/// The authorization test the brief asks for: a caller who can manage alerts
/// but cannot read the Data API endpoint(s) a `dataapi_alert` watches must be
/// refused — otherwise `alert:manage` is a side channel for observing an
/// endpoint's traffic and error rate without ever holding
/// `data_api_endpoint:read`.
///
/// An ADMIN would pass both checks and prove nothing on its own (the brief's
/// own warning) — which is why the FIRST assertion below is the one that
/// carries the test: `fx.editor_tok` (`ws_editor`) holds `alert:manage`
/// (migration 0110) but, unlike `ws_admin`, was never granted
/// `data_api_endpoint:read` (migration 0122 stops at `ws_admin` on purpose —
/// see that migration's own comment). `ws_admin` appears only in the SECOND
/// assertion, to show the SAME gate also lets a caller through, so the first
/// assertion is a real yes/no rather than a refusal that fires
/// unconditionally.
#[tokio::test]
async fn updating_a_dataapi_alert_needs_data_api_endpoint_read() {
    let fx = fixture!();
    // The schedule's OWNER is unrelated to who is calling PUT — no test here
    // runs the tick, so a fresh, roleless user is enough to satisfy the
    // owner FK.
    let owner = mk_user(&fx.db, "z2_authz_owner").await;
    let id = insert_workspace_scoped_dataapi_alert(&fx.db, fx.ws, owner, "z2-authz-denied").await;

    // `ws_editor`: `alert:manage`, no `data_api_endpoint:read`. Refused
    // before the update ever touches the row.
    let (status, body) = req_raw(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(minimal_update_body()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "alert:manage without data_api_endpoint:read must be refused: {body}"
    );

    // `ws_admin`: the same `alert:manage`, PLUS workspace-level
    // `data_api_endpoint:read` — gets past this gate. It still cannot reach
    // 200: `ScheduleType` has no `dataapi_alert` variant yet (Task 5), so
    // `update`'s own `ScheduleType::parse(&stored.r#type)` — which runs
    // strictly AFTER this task's check — fails closed with a named internal
    // error. That is the pre-existing, separately-tracked gap this task
    // inherits, not this check; what matters here is that the response is no
    // longer 403.
    let admin = admin_member(&fx.db, fx.ws).await;
    let admin_tok = fx.token_for(admin).await;
    let (status, body) = req_raw(
        &fx,
        &admin_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(minimal_update_body()),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "data_api_endpoint:read must get the caller PAST the new gate: {body}"
    );
    // Task 5 gave `ScheduleType` its `DataapiAlert` variant, so
    // `ScheduleType::parse(&stored.r#type)` — which runs strictly AFTER this
    // task's check — no longer fails closed with the internal-error gap this
    // test used to land on (that gap is gone, not merely hidden: see the
    // model.rs test suite for the replacement coverage). It now reaches
    // `to_new`'s own validation instead, which 400s naming a field
    // `minimal_update_body()` never set — still proof the request got PAST
    // this gate, just via a different, now-real code path rather than a
    // placeholder one.
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expected the caller to reach Task 5's dataapi_alert validation \
         (proving it passed THIS check), got {status}: {body}"
    );
    assert!(
        body.contains("dataapi_metric"),
        "expected the 400 to name the missing field: {body}"
    );
}

/// The object-scoped half: a `dataapi_alert` that names ONE endpoint
/// (`dataapi_endpoint_id`) is gated on OBJECT-scoped `data_api_endpoint:read`
/// on that row, not merely the workspace-level grant the previous test
/// covers. Workspace-level read must NOT be treated as sufficient on its
/// own — Cedar's own workspace→object cascade already makes a workspace
/// grant satisfy an object check, so this test's job is only to prove the
/// OTHER direction: manage-only, scoped to one endpoint, is still refused.
#[tokio::test]
async fn updating_an_endpoint_scoped_dataapi_alert_needs_data_api_endpoint_read() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_authz_object_owner").await;

    // `name`/`path`/`sql`/`database_id` are the columns migration 0121 makes
    // NOT NULL; `fx.database` is the `query.databases` row `setup` already
    // creates in this same workspace.
    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,'z2 authz test','/z2-authz-test','[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$2,$3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(fx.database)
    .bind(owner)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert dataapi.endpoints row");
    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, validator_type, validator_config, \
            dataapi_metric, dataapi_window_s, dataapi_endpoint_id) \
         VALUES ($1,'dataapi_alert','z2-authz-object','* * * * *', \
                 now() - interval '1 minute','operator',$2,'call_count',3600,$3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(serde_json::json!({"op": ">", "threshold": 0}))
    .bind(endpoint_id)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert endpoint-scoped dataapi_alert");
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&fx.db.pool)
        .await
        .expect("insert owner");

    // `ws_editor`: `alert:manage`, no `data_api_endpoint:read` on this
    // endpoint (nor at workspace level — same migration-0122 gap the
    // previous test relies on).
    let (status, body) = req_raw(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{schedule}", fx.slug),
        Some(minimal_update_body()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "alert:manage without data_api_endpoint:read on THIS endpoint must be refused: {body}"
    );

    // The positive control Finding 3 asked for: a caller whose ONLY Data API
    // permission is an OBJECT-scoped `data_api_endpoint:read` grant on THIS
    // endpoint — no `ws_admin`, no workspace-level grant at all — still gets
    // past the gate. Every prior test in this file (and the workspace-scoped
    // test above) passes `data_api_endpoint:read` in only at WORKSPACE scope,
    // where Cedar's own `resource in ?resource` cascade means an object check
    // would be satisfied even if `OBJECT_KIND` were built wrong (a mismatched
    // kind never gets compared — the workspace ancestor already matches).
    // This is the one case that actually exercises the kind string: if
    // `require_dataapi_endpoint_read`'s object branch built ANY kind other
    // than the literal this grant was made under, the object entity it asks
    // the PDP about would not be the one this grant names, and the request
    // would come back 403 same as `ws_editor` above.
    let object_reader = mk_user(&fx.db, "z2_authz_object_reader").await;
    grant_ws_role(&fx.db, object_reader, fx.ws, "ws_editor").await; // alert:manage only
    grant_object_scoped_data_api_read(&fx.db, object_reader, endpoint_id).await;
    let object_reader_tok = fx.token_for(object_reader).await;
    // The body names the SAME endpoint the row already has — fix-round-1's
    // second check (on the body's target, not just the stored one) means an
    // update that OMITTED `dataapi_endpoint_id` here would ask to widen to
    // the whole workspace instead, which this object-scoped-only grant
    // correctly cannot authorize (see the two dedicated tests below). This
    // body keeps the test isolated to what it says it proves: read on the
    // endpoint the row already names is enough to touch it.
    //
    // Fix-round-3: `minimal_update_body_with_endpoint` is now a FULL, valid
    // `dataapi_alert` body, so this caller — having passed the gate — reaches
    // a real `200`, not merely `to_new`'s 400. That also makes this the
    // route-level "readable caller successfully updates a dataapi_alert"
    // corner the re-review asked for: a `PUT` from an authorized caller with
    // a valid body persists, and the response echoes back what was stored.
    let (status, body) = req(
        &fx,
        &object_reader_tok,
        "PUT",
        &format!("/ws/{}/alerts/{schedule}", fx.slug),
        Some(minimal_update_body_with_endpoint(endpoint_id)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an object-scoped data_api_endpoint:read grant on THIS endpoint must be enough \
         to reach a real update, got {status}: {body}"
    );
    assert_eq!(body["name"], "renamed");
    assert_eq!(body["dataapi_endpoint_id"], endpoint_id.to_string());
    assert_eq!(body["dataapi_metric"], "call_count");
    assert_eq!(body["dataapi_window_s"], 300);

    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT dataapi_endpoint_id FROM automation.schedules WHERE id = $1")
            .bind(schedule)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        Some(endpoint_id),
        "the update must actually have persisted, not just returned 200"
    );
}

/// A second `dataapi.endpoints` row in the SAME workspace, distinct from
/// `endpoint_id` — for the two fix-round-1 tests below, which both need TWO
/// endpoints to distinguish "read on the row's CURRENT endpoint" from "read
/// on the endpoint the update is trying to set".
async fn a_second_endpoint(fx: &Fx, owner: Uuid, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,$2,$3,'[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$4,$5) RETURNING id",
    )
    .bind(fx.ws)
    .bind(format!("z2 {tag}"))
    .bind(format!("/z2-{tag}"))
    .bind(fx.database)
    .bind(owner)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert second dataapi.endpoints row")
}

/// Fix-round-1, Critical 1, exploit 1 ("Repoint"): before this fix, `update()`
/// checked only the STORED `dataapi_endpoint_id`. A caller who could read
/// endpoint A could PUT a body naming a DIFFERENT endpoint B and the row
/// would be repointed at B — measuring and (via the same body's `recipients`)
/// potentially MAILING B's traffic to a caller who never held
/// `data_api_endpoint:read` on B, object- or workspace-scoped. Proven live in
/// review against `egret_test` before this fix; this is the permanent
/// regression test for it.
#[tokio::test]
async fn updating_a_dataapi_alert_cannot_be_repointed_to_an_unreadable_endpoint() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_repoint_owner").await;
    let endpoint_a: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,'z2 repoint a','/z2-repoint-a','[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$2,$3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(fx.database)
    .bind(owner)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert endpoint A");
    let endpoint_b = a_second_endpoint(&fx, owner, "repoint-b").await;

    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, validator_type, validator_config, \
            dataapi_metric, dataapi_window_s, dataapi_endpoint_id) \
         VALUES ($1,'dataapi_alert','z2-repoint','* * * * *', \
                 now() - interval '1 minute','operator',$2,'call_count',3600,$3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(serde_json::json!({"op": ">", "threshold": 0}))
    .bind(endpoint_a)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert endpoint-a-scoped dataapi_alert");
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&fx.db.pool)
        .await
        .expect("insert owner");

    // ONLY object-scoped `data_api_endpoint:read` on A — nothing on B, no
    // workspace-level grant at all.
    let attacker = mk_user(&fx.db, "z2_repoint_attacker").await;
    grant_ws_role(&fx.db, attacker, fx.ws, "ws_editor").await; // alert:manage only
    grant_object_scoped_data_api_read(&fx.db, attacker, endpoint_a).await;
    let attacker_tok = fx.token_for(attacker).await;

    let (status, body) = req_raw(
        &fx,
        &attacker_tok,
        "PUT",
        &format!("/ws/{}/alerts/{schedule}", fx.slug),
        Some(minimal_update_body_with_endpoint(endpoint_b)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "repointing to an endpoint the caller cannot read must be refused: {body}"
    );

    // No partial write: the row must still watch A, not B and not NULL.
    let still_a: Option<Uuid> =
        sqlx::query_scalar("SELECT dataapi_endpoint_id FROM automation.schedules WHERE id = $1")
            .bind(schedule)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        still_a,
        Some(endpoint_a),
        "a refused repoint must not have moved the stored endpoint at all"
    );
}

/// Fix-round-1, Critical 1, exploit 2 ("widen by omission"): before this fix,
/// omitting `dataapi_endpoint_id` from the body deserialized to `None` and
/// was written straight through — turning an endpoint-scoped alert into a
/// workspace-wide one (this codebase's own definition of `NULL` here), with
/// no check that the caller could read at WORKSPACE scope. No malice
/// required: any client that simply does not send the field does this.
/// Proven live in review; this is the permanent regression test for it.
#[tokio::test]
async fn updating_a_dataapi_alert_cannot_be_widened_to_the_workspace_by_omitting_the_endpoint() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_widen_owner").await;
    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,'z2 widen','/z2-widen','[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$2,$3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(fx.database)
    .bind(owner)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert endpoint");
    let schedule: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, next_run_at, validator_type, validator_config, \
            dataapi_metric, dataapi_window_s, dataapi_endpoint_id) \
         VALUES ($1,'dataapi_alert','z2-widen','* * * * *', \
                 now() - interval '1 minute','operator',$2,'call_count',3600,$3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(serde_json::json!({"op": ">", "threshold": 0}))
    .bind(endpoint_id)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert endpoint-scoped dataapi_alert");
    sqlx::query("INSERT INTO automation.schedule_owners (schedule_id, user_id) VALUES ($1,$2)")
        .bind(schedule)
        .bind(owner)
        .execute(&fx.db.pool)
        .await
        .expect("insert owner");

    // ONLY object-scoped read on THIS endpoint — enough to touch the row
    // (proven by the positive-control test above) but not enough to ask for
    // "every endpoint in the workspace".
    let attacker = mk_user(&fx.db, "z2_widen_attacker").await;
    grant_ws_role(&fx.db, attacker, fx.ws, "ws_editor").await; // alert:manage only
    grant_object_scoped_data_api_read(&fx.db, attacker, endpoint_id).await;
    let attacker_tok = fx.token_for(attacker).await;

    // `minimal_update_body()`, not `_with_endpoint` — the body under test
    // OMITS `dataapi_endpoint_id` entirely.
    let (status, body) = req_raw(
        &fx,
        &attacker_tok,
        "PUT",
        &format!("/ws/{}/alerts/{schedule}", fx.slug),
        Some(minimal_update_body()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "omitting dataapi_endpoint_id must not silently widen to the whole workspace: {body}"
    );

    let still_scoped: Option<Uuid> =
        sqlx::query_scalar("SELECT dataapi_endpoint_id FROM automation.schedules WHERE id = $1")
            .bind(schedule)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        still_scoped,
        Some(endpoint_id),
        "a refused widen must not have nulled out the stored endpoint"
    );
}

/// A well-formed `dataapi_alert` create body — workspace-scoped
/// (`dataapi_endpoint_id` omitted).
fn dataapi_alert_json(name: &str) -> String {
    serde_json::json!({
        "type": "dataapi_alert",
        "name": name,
        "active": true,
        "crontab": "*/5 * * * *",
        "timezone": "UTC",
        "validator_type": "operator",
        "validator_config": {"op": ">", "threshold": 0},
        "dataapi_metric": "call_count",
        "dataapi_window_s": 300,
    })
    .to_string()
}

/// The hand-off from Task 4: `create()` could not gate `dataapi_alert`
/// because `CreateSchedule`/`ScheduleType` could not express one. Task 5 just
/// gave them that ability, which turns the comment `create()` carried
/// ("nothing this handler can write today names an endpoint") into a hole
/// unless the same `require_dataapi_endpoint_read` call `update` already uses
/// is wired in at create time too — this is that test.
///
/// `fx.editor_tok` (`ws_editor`) holds `alert:manage` but, unlike `ws_admin`,
/// was never granted `data_api_endpoint:read` (migration 0122 stops at
/// `ws_admin` on purpose). An admin alone would pass both checks and prove
/// nothing, which is why the FIRST assertion is the one that carries the
/// test.
#[tokio::test]
async fn creating_a_dataapi_alert_needs_data_api_endpoint_read() {
    let fx = fixture!();

    // `ws_editor`: `alert:manage`, no `data_api_endpoint:read`. Refused
    // before a row is ever written.
    let (status, body) = req_raw(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(dataapi_alert_json("z2-create-authz-denied")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "alert:manage without data_api_endpoint:read must be refused at create time: {body}"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM automation.schedules WHERE workspace_id = $1 AND name = $2",
    )
    .bind(fx.ws)
    .bind("z2-create-authz-denied")
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "a refused create must never write a row, not even one the caller cannot see"
    );

    // `ws_admin`: the same `alert:manage`, PLUS workspace-level
    // `data_api_endpoint:read` — gets past this gate AND all the way to a
    // real, well-formed row: this also proves the Task 5 write path
    // (`NewSchedule`'s four new fields, threaded through `create`/`update`'s
    // INSERT/UPDATE) actually persists a `dataapi_alert`, not merely that the
    // gate lets the request through.
    let admin = admin_member(&fx.db, fx.ws).await;
    let admin_tok = fx.token_for(admin).await;
    let (status, body) = req(
        &fx,
        &admin_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(dataapi_alert_json("z2-create-authz-allowed")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "data_api_endpoint:read must get the caller past the gate and all the way to 200: {body}"
    );
    assert_eq!(body["type"], "dataapi_alert");
    assert_eq!(body["dataapi_metric"], "call_count");
    assert_eq!(body["dataapi_window_s"], 300);
    assert!(
        body["dataapi_endpoint_id"].is_null(),
        "an omitted dataapi_endpoint_id is workspace-scoped, not an error: {body}"
    );
}

/// The object-scoped half of the create-time gate: a `dataapi_alert` create
/// body naming ONE `dataapi_endpoint_id` is checked against OBJECT-scoped
/// `data_api_endpoint:read` on THAT endpoint, not merely workspace-level
/// read — the same distinction `updating_an_endpoint_scoped_dataapi_alert_
/// needs_data_api_endpoint_read` proves for the update path.
#[tokio::test]
async fn creating_an_endpoint_scoped_dataapi_alert_needs_data_api_endpoint_read() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_create_object_owner").await;
    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,'z2 create authz test','/z2-create-authz-test','[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$2,$3) \
         RETURNING id",
    )
    .bind(fx.ws)
    .bind(fx.database)
    .bind(owner)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert dataapi.endpoints row");

    let mut body: serde_json::Value =
        serde_json::from_str(&dataapi_alert_json("z2-create-authz-object")).unwrap();
    body["dataapi_endpoint_id"] = serde_json::Value::String(endpoint_id.to_string());
    let body = body.to_string();

    // `ws_editor`: `alert:manage`, no `data_api_endpoint:read` on this
    // endpoint (nor at workspace level).
    let (status, resp) = req_raw(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "alert:manage without data_api_endpoint:read on THIS endpoint must be refused: {resp}"
    );

    // An OBJECT-scoped grant on THIS endpoint, and nothing at workspace
    // level, still gets past the gate — the positive control that pins
    // `OBJECT_KIND` the same way the update test's does.
    let object_reader = mk_user(&fx.db, "z2_create_object_reader").await;
    grant_ws_role(&fx.db, object_reader, fx.ws, "ws_editor").await;
    grant_object_scoped_data_api_read(&fx.db, object_reader, endpoint_id).await;
    let object_reader_tok = fx.token_for(object_reader).await;
    let (status, resp) = req(
        &fx,
        &object_reader_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an object-scoped data_api_endpoint:read grant on THIS endpoint must be enough: {resp}"
    );
    assert_eq!(resp["dataapi_endpoint_id"], endpoint_id.to_string());
}

/// A second, unrelated workspace holding one real `dataapi.endpoints` row —
/// for the two fix-round-2 tests below, which both need a REAL endpoint
/// outside `fx.ws` entirely (not merely one `fx`'s editor happens not to be
/// able to read, which `other_slug` already covers elsewhere in this file).
async fn a_foreign_workspace_endpoint(fx: &Fx, tag: &str) -> Uuid {
    let owner = mk_user(&fx.db, &format!("z2_{tag}_owner")).await;
    let ws = mk_ws(
        &fx.db,
        owner,
        &format!("z2-{tag}-{}", Uuid::new_v4().simple()),
    )
    .await;
    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,$2,$3,'[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$4,$5) RETURNING id",
    )
    .bind(ws)
    .bind(format!("z2 {tag}"))
    .bind(format!("/z2-{tag}"))
    .bind(database)
    .bind(owner)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap()
}

/// Fix-round-2, Critical 2: `dataapi_endpoint_id` naming a REAL endpoint that
/// belongs to a DIFFERENT workspace must be refused — the FK
/// (`schedules_dataapi_endpoint_id_fkey`) checks existence only, and the PDP
/// object check was told the CALLER's workspace, never the endpoint's, so a
/// workspace-cascaded grant (`ws_admin`) authorized ANY uuid before this fix.
///
/// Both corners the review asked for, on ONE caller: a REAL foreign endpoint
/// and a FICTIONAL uuid must refuse IDENTICALLY — same status, same body — or
/// the response is an oracle for "does this uuid name a live endpoint
/// anywhere in the deployment". The caller is `ws_admin` of `fx.ws`, not
/// `ws_editor`: an admin's workspace-cascaded `data_api_endpoint:read` is
/// EXACTLY the grant that used to satisfy the PDP for any uuid, so this is
/// the caller that actually exercises the new check — an editor with no
/// grant at all would be refused by the OLD code too and would prove
/// nothing.
#[tokio::test]
async fn creating_a_dataapi_alert_naming_a_foreign_or_nonexistent_endpoint_is_refused_identically()
{
    let fx = fixture!();
    let admin = admin_member(&fx.db, fx.ws).await;
    let admin_tok = fx.token_for(admin).await;
    let foreign_endpoint = a_foreign_workspace_endpoint(&fx, "create-foreign").await;

    let mut foreign_body: serde_json::Value =
        serde_json::from_str(&dataapi_alert_json("z2-create-foreign")).unwrap();
    foreign_body["dataapi_endpoint_id"] = serde_json::Value::String(foreign_endpoint.to_string());
    let (foreign_status, foreign_resp) = req_raw(
        &fx,
        &admin_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(foreign_body.to_string()),
    )
    .await;
    assert_eq!(
        foreign_status,
        StatusCode::NOT_FOUND,
        "a workspace-cascaded grant must not authorize a REAL endpoint in a \
         different workspace: {foreign_resp}"
    );

    let mut nonexistent_body: serde_json::Value =
        serde_json::from_str(&dataapi_alert_json("z2-create-nonexistent")).unwrap();
    nonexistent_body["dataapi_endpoint_id"] = serde_json::Value::String(Uuid::new_v4().to_string());
    let (nonexistent_status, nonexistent_resp) = req_raw(
        &fx,
        &admin_tok,
        "POST",
        &format!("/ws/{}/alerts", fx.slug),
        Some(nonexistent_body.to_string()),
    )
    .await;
    assert_eq!(
        nonexistent_status,
        StatusCode::NOT_FOUND,
        "{nonexistent_resp}"
    );

    assert_eq!(
        foreign_status, nonexistent_status,
        "a foreign endpoint and a nonexistent one must refuse with the SAME status"
    );
    assert_eq!(
        foreign_resp, nonexistent_resp,
        "and the SAME body, or the response is still an oracle for existence: \
         foreign={foreign_resp}, nonexistent={nonexistent_resp}"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM automation.schedules \
         WHERE workspace_id = $1 AND name LIKE 'z2-create-%'",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "a refused create must never write a row");
}

/// The update-path half of Critical 2's fix: repointing an EXISTING
/// `dataapi_alert` at a foreign or nonexistent endpoint must be refused the
/// same way create is, and the two must still be indistinguishable from each
/// other. Same `ws_admin` reasoning as the create test above — the whole
/// point is that this caller's grant used to be enough.
#[tokio::test]
async fn updating_a_dataapi_alert_to_a_foreign_or_nonexistent_endpoint_is_refused_identically() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_update_foreign_owner").await;
    let id = insert_workspace_scoped_dataapi_alert(&fx.db, fx.ws, owner, "z2-update-foreign").await;
    let admin = admin_member(&fx.db, fx.ws).await;
    let admin_tok = fx.token_for(admin).await;
    let foreign_endpoint = a_foreign_workspace_endpoint(&fx, "update-foreign").await;

    let (foreign_status, foreign_resp) = req_raw(
        &fx,
        &admin_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(minimal_update_body_with_endpoint(foreign_endpoint)),
    )
    .await;
    assert_eq!(
        foreign_status,
        StatusCode::NOT_FOUND,
        "repointing at a REAL foreign endpoint must be refused: {foreign_resp}"
    );

    let (nonexistent_status, nonexistent_resp) = req_raw(
        &fx,
        &admin_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}", fx.slug),
        Some(minimal_update_body_with_endpoint(Uuid::new_v4())),
    )
    .await;
    assert_eq!(
        nonexistent_status,
        StatusCode::NOT_FOUND,
        "{nonexistent_resp}"
    );

    assert_eq!(
        foreign_status, nonexistent_status,
        "a foreign endpoint and a nonexistent one must refuse with the SAME status"
    );
    assert_eq!(
        foreign_resp, nonexistent_resp,
        "and the SAME body: foreign={foreign_resp}, nonexistent={nonexistent_resp}"
    );

    let still_workspace_scoped: Option<Uuid> =
        sqlx::query_scalar("SELECT dataapi_endpoint_id FROM automation.schedules WHERE id = $1")
            .bind(id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(
        still_workspace_scoped.is_none(),
        "a refused repoint must not have moved the stored endpoint"
    );
}

/// Reading a `dataapi_alert`'s RUN-DERIVED state needs `data_api_endpoint:read`
/// too — Task 4 review, Finding 1. `alert:read`/`alert:manage` only ever said
/// something about the SCHEDULE; `RunDto::value` (`runs`) and
/// `ScheduleDetailDto::last_value` (`detail`) ARE the endpoint's measured
/// metric, and `run_now` is worse than either: it forces a FRESH measurement
/// before handing the result back, so a refusal has to land before the
/// runner ever runs, not merely before the response is built. `list` cannot
/// use the same all-or-nothing gate — a page mixes schedules of every kind —
/// so it redacts `last_state`/`last_eval_at` per row instead of failing the
/// whole request or hiding the row (ordinary alert-management visibility,
/// name/owners/id, is untouched).
///
/// `detail`/`runs` never depended on `ScheduleType::parse` recognising
/// `dataapi_alert` (`detail_dto`/`summary_dto` both fall back to `Alert` on an
/// unparseable type rather than erroring, and the tick's own dispatch in
/// `runner.rs` matches the raw `type` string), so the ALLOWED assertions below
/// reach a real `200` both before and after Task 5 — unlike `update`'s tests,
/// which used to land on an internal-error gap here and now land on a real
/// 400 from `to_new`'s validation instead.
#[tokio::test]
async fn reading_a_dataapi_alerts_run_state_needs_data_api_endpoint_read() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_authz_read_owner").await;
    let id = insert_workspace_scoped_dataapi_alert(&fx.db, fx.ws, owner, "z2-authz-read").await;
    // A schedule that has already run once, so `list`'s redaction has
    // something real to redact — an untouched row's `last_state` is NULL
    // regardless of the gate, which would make that assertion pass for the
    // wrong reason.
    sqlx::query(
        "UPDATE automation.schedules SET last_state = 'success', last_eval_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let runs_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM automation.schedule_runs WHERE schedule_id = $1")
            .bind(id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();

    // `ws_editor`: `alert:read`/`alert:manage`, no `data_api_endpoint:read`.
    for (method, path) in [
        ("GET", format!("/ws/{}/alerts/{id}", fx.slug)),
        ("GET", format!("/ws/{}/alerts/{id}/runs", fx.slug)),
    ] {
        let (status, body) = req_raw(&fx, &fx.editor_tok, method, &path, None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} must be refused: {body}"
        );
    }
    let (status, body) = req_raw(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("/ws/{}/alerts/{id}/run", fx.slug),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run_now must be refused: {body}"
    );
    let runs_after_denied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM automation.schedule_runs WHERE schedule_id = $1")
            .bind(id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(
        runs_after_denied, runs_before,
        "a refused run_now must never re-measure — no run-log row should be written"
    );

    // `list`: the row is still visible (`alert:read` covers that), but its
    // run-derived state is blanked, not the request failing.
    let (status, list_body) = req(
        &fx,
        &fx.editor_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = list_body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id.to_string())
        .expect("the schedule is still listed");
    assert!(
        row["last_state"].is_null() && row["last_eval_at"].is_null(),
        "list must redact last_state/last_eval_at for an endpoint the caller cannot read: {row}"
    );

    // `ws_admin`: the same `alert:manage`, PLUS `data_api_endpoint:read`.
    let admin = admin_member(&fx.db, fx.ws).await;
    let admin_tok = fx.token_for(admin).await;
    for (method, path) in [
        ("GET", format!("/ws/{}/alerts/{id}", fx.slug)),
        ("GET", format!("/ws/{}/alerts/{id}/runs", fx.slug)),
    ] {
        let (status, body) = req_raw(&fx, &admin_tok, method, &path, None).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} must NOT be refused for a caller who can read the endpoint: {body}"
        );
    }
    let (status, admin_list_body) = req(
        &fx,
        &admin_tok,
        "GET",
        &format!("/ws/{}/alerts", fx.slug),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let admin_row = admin_list_body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id.to_string())
        .expect("the schedule is still listed");
    assert_eq!(
        admin_row["last_state"], "success",
        "an admin who CAN read the endpoint must see the real last_state: {admin_row}"
    );
    let (status, body) = req_raw(
        &fx,
        &admin_tok,
        "POST",
        &format!("/ws/{}/alerts/{id}/run", fx.slug),
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "run_now must NOT be refused for a caller who can read the endpoint: {body}"
    );
}

/// `set_active` leaks exactly what `list` redacts (Task 4 review, Finding 4).
/// `PUT .../active` requires only `alert:manage` and returns
/// `summary_dto(&row, ...)` built straight from the `UPDATE ... RETURNING`
/// row — `last_state`/`last_eval_at`, unredacted, would tell a caller who
/// cannot read the endpoint whether it last fired and when, AND the same
/// request re-enables the schedule, so it is poll-able as an oracle rather
/// than a one-off leak.
///
/// The other MANAGE-gated mutations were checked too, not assumed clean:
/// `trash`/`restore`/`set_folder`/`remove` all return `egret_core::Ack`
/// (`{"ok":true}`) or nothing — no `summary_dto`, no `RETURNING` row reaches
/// the response, so there is no field for this leak to ride on and nothing
/// to redact.
#[tokio::test]
async fn toggling_active_on_a_dataapi_alert_redacts_run_state_for_a_caller_who_cannot_read_it() {
    let fx = fixture!();
    let owner = mk_user(&fx.db, "z2_authz_active_owner").await;
    let id = insert_workspace_scoped_dataapi_alert(&fx.db, fx.ws, owner, "z2-authz-active").await;
    // A schedule that has already run once, so redaction has something real
    // to redact — an untouched row's `last_state` is NULL regardless of the
    // gate, which would pass for the wrong reason.
    sqlx::query(
        "UPDATE automation.schedules SET last_state = 'success', last_eval_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    // `ws_editor`: `alert:manage`, no `data_api_endpoint:read`. The toggle
    // itself must still work (that is `alert:manage`'s job) — only the
    // run-derived fields in the RESPONSE are at stake.
    let (status, body) = req(
        &fx,
        &fx.editor_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}/active", fx.slug),
        Some(serde_json::json!({"active": false}).to_string()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the toggle itself must succeed: {body}"
    );
    assert_eq!(
        body["active"], false,
        "the field being toggled must still work: {body}"
    );
    assert!(
        body["last_state"].is_null() && body["last_eval_at"].is_null(),
        "set_active must redact last_state/last_eval_at for an endpoint the caller cannot read: {body}"
    );

    // `ws_admin`: the same `alert:manage`, PLUS `data_api_endpoint:read` —
    // sees the real value.
    let admin = admin_member(&fx.db, fx.ws).await;
    let admin_tok = fx.token_for(admin).await;
    let (status, body) = req(
        &fx,
        &admin_tok,
        "PUT",
        &format!("/ws/{}/alerts/{id}/active", fx.slug),
        Some(serde_json::json!({"active": true}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["last_state"], "success",
        "an admin who CAN read the endpoint must see the real last_state: {body}"
    );
}
