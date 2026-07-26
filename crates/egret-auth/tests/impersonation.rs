//! DB-gated `ImpersonationRepo` tests. Self-skip (pass) without `DATABASE_URL`
//! so the suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::impersonation::{ImpersonationRepo, NewImpersonation};
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::UserId;
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
