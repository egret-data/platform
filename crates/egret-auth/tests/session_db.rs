//! DB-gated integration tests. Skip (pass) when DATABASE_URL is unset so the
//! suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
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

#[tokio::test]
async fn insert_and_find_user() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let repo = UserRepo::new(db);
    let email = unique_email("user");
    let hash = hash_password("pw").unwrap();

    let id = repo.insert_local(&email, "Tester", &hash).await.unwrap();
    let found = repo
        .find_by_email(&email)
        .await
        .unwrap()
        .expect("user exists");
    assert_eq!(found.id, id);
    assert_eq!(found.provider, "local");
    assert!(found.password_hash.is_some());

    let by_id = repo.find_by_id(id).await.unwrap().expect("by id");
    assert_eq!(by_id.email.to_lowercase(), email.to_lowercase());
}

use chrono::Duration;
use egret_auth::session::SessionService;
use egret_core::UserId;

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(&unique_email("sess"), "S", &hash)
        .await
        .unwrap()
}

#[tokio::test]
async fn create_then_validate_slides_idle() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));

    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().expect("valid");
    assert_eq!(principal.user_id, uid);
}

#[tokio::test]
async fn expired_session_does_not_validate() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    // idle_ttl in the past → immediately expired.
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(-1));
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}

#[tokio::test]
async fn revoke_invalidates() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "a", "127.0.0.1").await.unwrap();
    svc.revoke(&issued.token).await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}
