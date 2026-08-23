//! DB-gated ROUTE tests for Alerts & Reports. Self-skip without DATABASE_URL.
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
    let url = std::env::var("DATABASE_URL").ok()?;
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
async fn running_a_schedule_does_not_move_its_next_run_or_last_state() {
    // The manual-run contract from `runner.rs`: `run_now` must not call
    // `release`, so the schedule's own cadence is untouched by a click.
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

    req(
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
