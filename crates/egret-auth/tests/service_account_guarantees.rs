//! SA4: the standing guarantees of a service account (`users.provider =
//! 'service'`, NULL password), pinned. DB-gated; FAILS without a database
//! unless `EGRET_ALLOW_DB_SKIP` is set.
//!
//! 1. it cannot log in interactively;
//! 2. the per-login OIDC group sync never touches it;
//! 3. its `svc-…` username stays in `username_ledger` after it is deleted.

#[path = "support/recorder.rs"]
mod recorder;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use chrono::Duration;
use egret_auth::groups::GroupRepo;
use egret_auth::impersonation::ImpersonationRepo;
use egret_auth::oidc::{provision_and_sync, OidcClaims};
use egret_auth::routes::{auth_router, AuthState};
use egret_auth::service_accounts::{SaScope, ServiceAccountRepo};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_config::Config;
use egret_core::UserId;
use egret_metastore::Db;
use recorder::Recorder;
use tower::ServiceExt;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..10].to_string()
}

async fn creator(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'C') RETURNING id",
    )
    .bind(format!("c-{}@t.test", tag()))
    .fetch_one(&db.pool)
    .await
    .expect("creator")
}

fn actor() -> egret_core::Principal {
    egret_core::Principal {
        user_id: UserId(Uuid::nil()),
        email: "actor@test.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}

async fn make_sa(db: &Db, name: &str) -> egret_auth::service_accounts::ServiceAccount {
    ServiceAccountRepo::new(db.pool.clone())
        .create(SaScope::Platform, None, name, "d", creator(db).await)
        .await
        .expect("create sa")
}

fn login_app(db: &Db) -> axum::Router {
    let st = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
        rate_limiter: egret_core::ratelimit::RateLimiter::new(50, 300, 300),
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
    };
    axum::Router::new()
        .nest("/api/auth", auth_router())
        .with_state(st)
}

async fn post_login(app: &axum::Router, email: &str, password: &str) -> StatusCode {
    let body =
        serde_json::to_vec(&serde_json::json!({"email": email, "password": password})).unwrap();
    app.clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// `routes::login` admits only `status = 'active' AND password_hash IS NOT
/// NULL`; a service account has no password (`users_service_has_no_password`),
/// so no password, empty or guessed, can authenticate it, and no session row
/// is minted.
#[tokio::test]
async fn a_service_account_cannot_log_in() {
    let Some(db) = connect().await else { return };
    let sa = make_sa(&db, &format!("nologin-{}", tag())).await;
    let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(sa.user_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let app = login_app(&db);

    for pw in ["", "password", "svc", &email] {
        assert_eq!(
            post_login(&app, &email, pw).await,
            StatusCode::UNAUTHORIZED,
            "password {pw:?}"
        );
    }
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
        .bind(sa.user_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "a failed login mints no session");

    // The database refuses to give it a password at all (the root of the guarantee).
    let err = sqlx::query("UPDATE users SET password_hash = 'x' WHERE id = $1")
        .bind(sa.user_id)
        .execute(&db.pool)
        .await
        .expect_err("constraint");
    assert_eq!(
        err.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514"),
        "{err:?}"
    );
}

/// A service user is never a subject of the OIDC sync: `provision_and_sync`
/// resolves its user by `find_by_external_subject`, which only matches
/// `provider = 'oidc'`. Even a service row carrying the very subject the IdP
/// presents is not matched: the login provisions a distinct OIDC user and the
/// service account's group memberships are left exactly as they were.
#[tokio::test]
async fn oidc_sync_never_touches_a_service_account() {
    let Some(db) = connect().await else { return };
    let groups = GroupRepo::new(db.clone());
    let users = UserRepo::new(db.clone());
    let rc = Recorder::default();
    let t = tag();

    let sa = make_sa(&db, &format!("nosync-{t}")).await;
    let local = groups
        .create_local(&format!("local-{t}"), &rc)
        .await
        .expect("local group");
    groups
        .add_member(local, UserId(sa.user_id), &rc)
        .await
        .expect("manual membership");
    let oidc_name = format!("oidc-{t}");
    groups.upsert_oidc_group(&oidc_name).await.unwrap();

    let subject = format!("sub-{t}");
    sqlx::query("UPDATE users SET external_subject = $2 WHERE id = $1")
        .bind(sa.user_id)
        .bind(&subject)
        .execute(&db.pool)
        .await
        .expect("plant the subject on the service row");
    assert!(
        users
            .find_by_external_subject(&subject)
            .await
            .unwrap()
            .is_none(),
        "the subject lookup is keyed on provider = 'oidc'"
    );

    let claims = OidcClaims {
        subject,
        email: format!("h-{t}@t.test"),
        email_verified: true,
        name: "Human".into(),
        groups: vec![oidc_name.clone()],
    };
    let uid = provision_and_sync(&users, &groups, &claims, &rc)
        .await
        .expect("login");
    assert_ne!(uid.0, sa.user_id, "the login is a different user");

    let held: Vec<(String, String)> = sqlx::query_as(
        "SELECT g.name, g.source FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
          WHERE ug.user_id = $1",
    )
    .bind(sa.user_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        held,
        vec![(format!("local-{t}"), "local".to_string())],
        "the service account keeps its manual membership and gains no OIDC one"
    );
    let provider: String = sqlx::query_scalar("SELECT provider FROM users WHERE id = $1")
        .bind(uid.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(provider, "oidc");
}

/// Deleting a service account retires its `svc-…` name; it is never freed.
#[tokio::test]
async fn a_deleted_service_accounts_username_stays_retired() {
    let Some(db) = connect().await else { return };
    let sa = make_sa(&db, &format!("ledger-{}", tag())).await;
    let username = UserRepo::new(db.clone())
        .ensure_username(UserId(sa.user_id))
        .await
        .expect("username");
    assert!(username.starts_with("svc-"), "{username}");

    assert!(ServiceAccountRepo::new(db.pool.clone())
        .delete(sa.user_id, &actor(), &Recorder::default())
        .await
        .unwrap());

    let (holder, minted_for, retired): (Option<Uuid>, Option<Uuid>, bool) = sqlx::query_as(
        "SELECT user_id, minted_for, retired_at IS NOT NULL FROM username_ledger \
          WHERE username = $1",
    )
    .bind(&username)
    .fetch_one(&db.pool)
    .await
    .expect("the ledger row survives the delete");
    assert_eq!(holder, None, "no current holder");
    assert_eq!(minted_for, Some(sa.user_id), "still names who held it");
    assert!(retired);

    // Nobody can be given the freed name: the ledger refuses (23505).
    let err = sqlx::query(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, 'X', $2)",
    )
    .bind(format!("x-{}@t.test", tag()))
    .bind(&username)
    .execute(&db.pool)
    .await
    .expect_err("retired name is not reusable");
    assert_eq!(
        err.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23505"),
        "{err:?}"
    );
}
