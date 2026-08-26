//! P2a: time-bound grants. An expired `access_grants` row must be excluded from
//! every access decision — the PDP enforcement chokepoint AND the session role
//! read that populates `Principal.platform_roles`. Requires DATABASE_URL.

use egret_auth::pdp::CedarPdp;
use egret_auth::session::SessionService;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn seed_user(db: &Db) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    UserId(id)
}

async fn seed_workspace(db: &Db) -> WorkspaceId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    WorkspaceId(id)
}

/// Seed a workspace-scoped grant of role `ws_admin` on `ws` for `user`, with an
/// explicit `expires_at` (may be NULL). The SQL interval is supplied by the
/// caller as a literal expression string spliced only from test-controlled
/// values — never user input.
async fn seed_ws_grant(db: &Db, user: UserId, ws: WorkspaceId, expires_expr: &str) {
    let sql = format!(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, expires_at) \
         SELECT 'user', $2, r.id, 'workspace', $1, {expires_expr} FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_admin'"
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(ws.0)
        .bind(user.0)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn expired_ws_grant_denies() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };
    let action = "dashboard:read";

    // Expired one hour ago → must DENY.
    let u = seed_user(&db).await;
    let w = seed_workspace(&db).await;
    seed_ws_grant(&db, u, w, "now() - interval '1 hour'").await;

    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();
    let subject = Subject {
        user_id: u,
        group_ids: vec![],
    };
    let d = pdp
        .authorize(&subject, action, &Resource::Workspace(w))
        .await
        .unwrap();
    assert!(
        !d.allow,
        "an EXPIRED ws grant must NOT authorize `{action}`"
    );
}

#[tokio::test]
async fn future_and_null_ws_grants_allow() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };
    let action = "dashboard:read";
    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();

    // Expiry one hour in the FUTURE → still active → ALLOW.
    let u1 = seed_user(&db).await;
    let w1 = seed_workspace(&db).await;
    seed_ws_grant(&db, u1, w1, "now() + interval '1 hour'").await;
    let d1 = pdp
        .authorize(
            &Subject {
                user_id: u1,
                group_ids: vec![],
            },
            action,
            &Resource::Workspace(w1),
        )
        .await
        .unwrap();
    assert!(
        d1.allow,
        "a FUTURE-expiry ws grant must authorize `{action}`"
    );

    // NULL expiry (never expires) → ALLOW.
    let u2 = seed_user(&db).await;
    let w2 = seed_workspace(&db).await;
    seed_ws_grant(&db, u2, w2, "NULL").await;
    let d2 = pdp
        .authorize(
            &Subject {
                user_id: u2,
                group_ids: vec![],
            },
            action,
            &Resource::Workspace(w2),
        )
        .await
        .unwrap();
    assert!(d2.allow, "a NULL-expiry ws grant must authorize `{action}`");
}

#[tokio::test]
async fn expired_platform_admin_not_in_session_roles() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };
    let uid = seed_user(&db).await;
    let admin_role: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // Expired platform_admin grant.
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, expires_at) \
         VALUES ('user', $1, $2, 'platform', NULL, now() - interval '1 hour')",
    )
    .bind(uid.0)
    .bind(admin_role)
    .execute(&db.pool)
    .await
    .unwrap();

    let svc = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().expect("valid");
    assert!(
        !principal
            .platform_roles
            .iter()
            .any(|r| r == "platform_admin"),
        "an EXPIRED platform_admin grant must NOT appear in session platform_roles"
    );

    // Sanity: the PDP itself must also deny a platform action for this user.
    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();
    let d = pdp
        .authorize(
            &Subject {
                user_id: uid,
                group_ids: vec![],
            },
            "engine:create",
            &Resource::Platform,
        )
        .await
        .unwrap();
    assert!(
        !d.allow,
        "an EXPIRED platform_admin grant must NOT authorize a platform action"
    );
}
