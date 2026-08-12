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
    let st = AutomationState {
        db: db.clone(),
        schedules: ScheduleRepo::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
        render_tokens: RenderTokenRepo::new(db.clone()),
        charts: egret_bi::ChartRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        databases: egret_query::DatabaseRepo::new(db.clone(), secrets, ws_secrets),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
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
    })
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
    assert!(after["report_format"].is_null());
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
