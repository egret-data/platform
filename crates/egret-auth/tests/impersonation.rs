//! DB-gated `ImpersonationRepo` tests. Self-skip (pass) without `DATABASE_URL`
//! so the suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::impersonation::{ImpersonationRepo, NewImpersonation};
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@test.local", uuid::Uuid::new_v4())
}

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(&unique_email("imp"), "Imp", &hash)
        .await
        .unwrap()
}

/// Seed a local user with a *specific* email (the overlay tests assert on it).
/// Cleans up any pre-existing user with that email first (cascades sessions +
/// impersonations) so the tests are re-runnable against a persistent DB.
async fn seed_user_named(db: &Db, email: &str) -> UserId {
    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(&db.pool)
        .await
        .unwrap();
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(email, "Named", &hash).await.unwrap()
}

/// The `sessions.id` for a raw token, resolved via the same `hash_token` the
/// service stores (`token_hash`).
async fn session_id_for_token(db: &Db, token: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM sessions WHERE token_hash = $1")
        .bind(egret_auth::token::hash_token(token))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// Seed a workspace owned by `owner`; returns its id.
async fn seed_workspace(db: &Db, owner: UserId) -> WorkspaceId {
    let slug = format!("imp-ws-{}", uuid::Uuid::new_v4());
    let id = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&slug)
    .bind("Imp WS")
    .bind(owner.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    WorkspaceId(id)
}

async fn seed_session(db: &Db, user: UserId) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
         VALUES ($1, $2, now() + interval '1 day', now() + interval '1 day') RETURNING id",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user.0)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn start_lookup_exit_roundtrip() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;

    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "debugging".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    let active = repo
        .active_for_session(sess)
        .await
        .unwrap()
        .expect("active");
    assert_eq!(active.target_user_id, target.0);
    assert_eq!(active.impersonator_id, admin.0);

    repo.exit(sess).await.unwrap();
    assert!(repo.active_for_session(sess).await.unwrap().is_none());
}

#[tokio::test]
async fn expired_overlay_is_not_active_and_is_cleaned() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;

    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE impersonations SET expires_at = now() - interval '1 minute' WHERE admin_session_id = $1",
    )
    .bind(sess)
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(
        repo.active_for_session(sess).await.unwrap().is_none(),
        "an expired overlay must not be active"
    );
    // And confirm it was reaped:
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM impersonations WHERE admin_session_id = $1")
            .bind(sess)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "expired overlay should be deleted by the lookup"
    );
}

#[tokio::test]
async fn start_twice_on_one_session_is_rejected() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let t1 = seed_user(&db).await;
    let t2 = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;
    let n = |t: UserId| NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: t,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    };
    repo.start(n(t1)).await.unwrap();
    assert!(
        repo.start(n(t2)).await.is_err(),
        "unique index blocks a second overlay"
    );
}

/// `start` must not depend on `active_for_session` having run first to reap a
/// dead overlay: a session whose overlay merely EXPIRED (never explicitly
/// exited) must accept a fresh `start` on its own.
#[tokio::test]
async fn start_reaps_expired_overlay_on_same_session() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let t1 = seed_user(&db).await;
    let t2 = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;

    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: t1,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE impersonations SET expires_at = now() - interval '1 minute' WHERE admin_session_id = $1",
    )
    .bind(sess)
    .execute(&db.pool)
    .await
    .unwrap();

    // No `active_for_session` call in between — `start` alone must reap the
    // dead row and succeed with a different target.
    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: t2,
        workspace_scope: None,
        reason: "y".into(),
        ttl_minutes: 30,
    })
    .await
    .expect("a fresh start must succeed once the old overlay has expired");

    let active = repo
        .active_for_session(sess)
        .await
        .unwrap()
        .expect("active");
    assert_eq!(active.target_user_id, t2.0);
}

#[tokio::test]
async fn validate_resolves_target_principal_under_overlay() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());

    let admin = seed_user_named(&db, "admin@x").await;
    let target = seed_user_named(&db, "target@x").await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;

    // No overlay yet → admin.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.email, "admin@x");
    assert!(p.impersonator.is_none());

    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    // Now → target, with impersonator = admin.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.email, "target@x");
    assert_eq!(p.user_id, target);
    let imper = p.impersonator.expect("impersonator set");
    assert_eq!(imper.user_id, admin);
    assert!(imper.workspace_scope.is_none());
}

#[tokio::test]
async fn validate_ignores_expired_overlay_and_returns_admin() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "adm2@x").await;
    let target = seed_user_named(&db, "tgt2@x").await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    sqlx::query("UPDATE impersonations SET expires_at = now() - interval '1 minute' WHERE admin_session_id = $1")
        .bind(sess_id)
        .execute(&db.pool)
        .await
        .unwrap();

    // Expired overlay → back to the admin.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.email, "adm2@x");
    assert!(p.impersonator.is_none());
}

#[tokio::test]
async fn validate_carries_workspace_scope() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "adm3@x").await;
    let target = seed_user_named(&db, "tgt3@x").await;
    let ws = seed_workspace(&db, admin).await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: Some(ws),
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.impersonator.unwrap().workspace_scope, Some(ws));
}

/// A disabled target must not resolve: the overlay is inert and validate returns
/// the admin principal (impersonating a suspended user is a no-op).
#[tokio::test]
async fn validate_disabled_target_falls_through_to_admin() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "adm4@x").await;
    let target = seed_user_named(&db, "tgt4@x").await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    // Disable the target after the overlay was started.
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(target.0)
        .execute(&db.pool)
        .await
        .unwrap();

    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(
        p.email, "adm4@x",
        "disabled target overlay must not resolve"
    );
    assert!(p.impersonator.is_none());
}
