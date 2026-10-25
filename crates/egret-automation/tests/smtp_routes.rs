//! SMTP settings over HTTP.
//!
//! Two properties only exist at this layer:
//!
//! 1. **The password never appears in a response.** Asserted against the raw
//!    body bytes, not against the DTO's fields — what reaches the browser is a
//!    string, and that is what must be searched.
//! 2. **Settings are workspace administration, not alert authoring.** A
//!    `ws_editor` may create alerts all day and still must not be able to read
//!    or change the relay credential every alert sends through.

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
    admin_tok: String,
    editor_tok: String,
}

async fn mk_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn grant(db: &Db, user: Uuid, ws: Uuid, role: &str) {
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
    .unwrap()
    .rows_affected();
    // A typo'd role name grants nothing, which would make the denial assertions
    // below pass for the wrong reason.
    assert_eq!(n, 1, "role {role} must exist and have been granted");
}

async fn setup(pool: sqlx::PgPool) -> Fx {
    let db = Db { pool };
    let admin = mk_user(&db, "sm_admin").await;
    let editor = mk_user(&db, "sm_editor").await;
    let slug = format!("sm-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'SMTP Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    grant(&db, admin, ws, "ws_admin").await;
    grant(&db, editor, ws, "ws_editor").await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .unwrap(),
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
            Some(egret_secrets::Crypto::new(&[3u8; 32]).unwrap()),
        ),
    );
    // Not exercised by this file's tests; wired for the struct to construct
    // at all.
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
    for u in [admin, editor] {
        tok.push(
            sessions
                .create(egret_core::ids::UserId(u), "test", "127.0.0.1")
                .await
                .unwrap()
                .token,
        );
    }
    Fx {
        app: automation_router().with_state(st),
        slug,
        editor_tok: tok.pop().unwrap(),
        admin_tok: tok.pop().unwrap(),
    }
}

/// Returns the status AND the raw body text, because the password assertion has
/// to search what actually went over the wire.
async fn req(
    fx: &Fx,
    tok: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
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
            b.body(
                body.map(|s| Body::from(s.to_string()))
                    .unwrap_or(Body::empty()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

const SECRET: &str = "sup3r-s3cret-relay-pw";

fn config_body() -> String {
    serde_json::json!({
        "host": "smtp.example.com",
        "port": 587,
        "username": "mailer",
        "from_address": "alerts@example.com",
        "from_name": "Egret",
        "tls_mode": "starttls",
        "password": SECRET,
    })
    .to_string()
}

fn path(fx: &Fx) -> String {
    format!("/ws/{}/notifications/smtp", fx.slug)
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_password_never_appears_in_any_response(pool: sqlx::PgPool) {
    // THE assertion of this file, against raw bytes rather than a struct.
    let fx = setup(pool).await;

    let (s, put_body) = req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&config_body())).await;
    assert_eq!(s, StatusCode::OK, "{put_body}");
    assert!(
        !put_body.contains(SECRET),
        "the PUT response echoed the password back: {put_body}"
    );

    let (s, get_body) = req(&fx, &fx.admin_tok, "GET", &path(&fx), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        !get_body.contains(SECRET),
        "the GET response contains the password: {get_body}"
    );
    // It reports that one exists, which is what the editor needs.
    let v: serde_json::Value = serde_json::from_str(&get_body).unwrap();
    assert_eq!(v["has_password"], true);
    assert_eq!(v["host"], "smtp.example.com");
    assert_eq!(v["username"], "mailer");
    assert!(v.get("password").is_none());
    assert!(
        !get_body.contains("password_secret_id"),
        "the secret's id is not the browser's business either: {get_body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_editor_cannot_read_or_change_the_relay_credential(pool: sqlx::PgPool) {
    // A ws_editor holds `alert:manage` — enough to author alerts, deliberately
    // not enough to touch the credential they all send through.
    let fx = setup(pool).await;
    req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&config_body())).await;

    for (method, body) in [
        ("GET", None),
        ("PUT", Some(config_body())),
        ("DELETE", None),
    ] {
        let (s, _) = req(&fx, &fx.editor_tok, method, &path(&fx), body.as_deref()).await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "{method} on SMTP settings must require workspace administration"
        );
    }

    // And the test-send, which would otherwise let an editor probe the relay.
    let (s, _) = req(
        &fx,
        &fx.editor_tok,
        "POST",
        &format!("{}/test", path(&fx)),
        Some("{}"),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn saving_again_without_a_password_keeps_the_stored_one(pool: sqlx::PgPool) {
    // The editor renders a blank password field because it cannot show the
    // real one; sending that blank must not erase the credential.
    let fx = setup(pool).await;
    req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&config_body())).await;

    let without = serde_json::json!({
        "host": "smtp2.example.com",
        "port": 587,
        "username": "mailer",
        "from_address": "alerts@example.com",
        "tls_mode": "starttls",
    })
    .to_string();
    let (s, body) = req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&without)).await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["host"], "smtp2.example.com", "the edit applied");
    assert_eq!(
        v["has_password"], true,
        "...and the password survived it: {body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unconfigured_workspace_is_a_204_not_an_empty_config(pool: sqlx::PgPool) {
    // "Not configured" and "configured with blanks" must be distinguishable, or
    // the settings page cannot tell the user which one they are looking at.
    // It is 204, not 404 (KI-212): every workspace starts unconfigured, and a
    // 404 on each visit of the settings page logged a console error.
    let fx = setup(pool).await;
    let (s, body) = req(&fx, &fx.admin_tok, "GET", &path(&fx), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");
    assert!(body.is_empty(), "{body}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_test_send_without_a_config_says_so_rather_than_failing_obscurely(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    let (s, body) = req(
        &fx,
        &fx.admin_tok,
        "POST",
        &format!("{}/test", path(&fx)),
        Some("{}"),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_test_send_reports_the_relays_own_error(pool: sqlx::PgPool) {
    // "Configured" and "configured correctly" are different states, and this
    // endpoint exists to tell them apart. Pointed at a port nothing is
    // listening on, it must fail with a message that names the problem — not a
    // 500, and not a success.
    let fx = setup(pool).await;
    let unreachable = serde_json::json!({
        "host": "127.0.0.1",
        "port": 1,
        "from_address": "alerts@example.com",
        "tls_mode": "none",
    })
    .to_string();
    req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&unreachable)).await;

    let (s, body) = req(
        &fx,
        &fx.admin_tok,
        "POST",
        &format!("{}/test", path(&fx)),
        Some("{}"),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_lowercase().contains("smtp") || body.to_lowercase().contains("connect"),
        "the failure should say what went wrong: {body}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_malformed_from_address_is_refused_at_save_time(pool: sqlx::PgPool) {
    // Caught here rather than at 3am when the first alert fires.
    let fx = setup(pool).await;
    let bad = serde_json::json!({
        "host": "smtp.example.com",
        "port": 587,
        "from_address": "not-an-address",
        "tls_mode": "starttls",
    })
    .to_string();
    let (s, body) = req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.contains("from_address"), "{body}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_the_config_makes_it_unconfigured_again(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    req(&fx, &fx.admin_tok, "PUT", &path(&fx), Some(&config_body())).await;

    let (s, _) = req(&fx, &fx.admin_tok, "DELETE", &path(&fx), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = req(&fx, &fx.admin_tok, "GET", &path(&fx), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

fn slack_path(fx: &Fx) -> String {
    format!("/ws/{}/notifications/slack", fx.slug)
}

/// The workspace's Slack app: one token for every channel it posts to.
///
/// Before this, every slack recipient carried its own bot token — three
/// channels meant the same credential pasted three times, rotated in three
/// places and sitting in three vault rows.
#[sqlx::test(migrations = "../../migrations")]
async fn a_workspace_slack_token_is_stored_and_never_returned(pool: sqlx::PgPool) {
    const TOKEN: &str = "xoxb-workspace-app-token";
    let fx = setup(pool).await;

    let body = serde_json::json!({"token": TOKEN, "default_channel": "#general"}).to_string();
    let (s, put_body) = req(&fx, &fx.admin_tok, "PUT", &slack_path(&fx), Some(&body)).await;
    assert_eq!(s, StatusCode::OK, "{put_body}");
    // Asserted on the RAW body: a token nested in a sub-object still satisfies
    // a struct-field check.
    assert!(
        !put_body.contains(TOKEN),
        "the PUT echoed the token: {put_body}"
    );

    let (s, get_body) = req(&fx, &fx.admin_tok, "GET", &slack_path(&fx), None).await;
    assert_eq!(s, StatusCode::OK, "{get_body}");
    assert!(
        !get_body.contains(TOKEN),
        "the GET leaks the token: {get_body}"
    );
    assert!(
        !get_body.contains("token_secret_id"),
        "the GET hands out the vault reference: {get_body}"
    );
    let v: serde_json::Value = serde_json::from_str(&get_body).unwrap();
    assert_eq!(v["has_token"], true);
    assert_eq!(v["default_channel"], "#general");
}

/// An empty field must not wipe the credential.
///
/// The editor cannot render a stored token, so its blank field means "keep" —
/// the same three-way rule as the SMTP password. Saving the default channel
/// alone must not silently disable every Slack delivery in the workspace.
#[sqlx::test(migrations = "../../migrations")]
async fn saving_slack_without_a_token_keeps_the_stored_one(pool: sqlx::PgPool) {
    let fx = setup(pool).await;

    let first = serde_json::json!({"token": "xoxb-original"}).to_string();
    let (s, body) = req(&fx, &fx.admin_tok, "PUT", &slack_path(&fx), Some(&first)).await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let second = serde_json::json!({"default_channel": "#moved"}).to_string();
    let (s, body) = req(&fx, &fx.admin_tok, "PUT", &slack_path(&fx), Some(&second)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["has_token"], true, "the token was dropped: {body}");
    assert_eq!(v["default_channel"], "#moved");
}

/// A first write with no token is refused rather than stored.
///
/// A config row whose whole purpose is to hold a credential, holding none, is
/// a workspace that looks configured and silently cannot deliver.
/// KI-212: as SMTP's, an unconfigured Slack app is a 204, not a 404.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unconfigured_slack_app_is_a_204(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    let (s, body) = req(&fx, &fx.admin_tok, "GET", &slack_path(&fx), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");
    assert!(body.is_empty(), "{body}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_first_slack_save_without_a_token_is_refused(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    let body = serde_json::json!({"default_channel": "#general"}).to_string();
    let (s, out) = req(&fx, &fx.admin_tok, "PUT", &slack_path(&fx), Some(&body)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{out}");
    assert!(out.contains("token"), "{out}");
}

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
