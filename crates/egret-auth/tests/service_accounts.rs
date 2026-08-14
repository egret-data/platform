//! A service account is a user row that can never log in.
//!
//! DB-gated integration tests. Skip (pass) when DATABASE_URL is unset so the
//! suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@acme.service.egret.local", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn service_account_cannot_have_a_password() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let users = UserRepo::new(db.clone());
    let id = users
        .insert_service(&unique_email("svc.reporting"), "reporting")
        .await
        .unwrap();

    // The DB, not the insert path, is what guarantees this.
    let err = sqlx::query("UPDATE users SET password_hash = 'x' WHERE id = $1")
        .bind(id.0)
        .execute(&db.pool)
        .await
        .expect_err("the CHECK must reject a password on a service account");
    assert!(
        err.to_string().contains("users_service_has_no_password"),
        "expected the named CHECK to fire, got: {err}"
    );
}

#[tokio::test]
async fn service_accounts_are_absent_from_the_user_list() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let users = UserRepo::new(db.clone());
    let human = users
        .insert_local(&unique_email("human"), "human", "hash")
        .await
        .unwrap();
    let svc = users
        .insert_service(&unique_email("svc.x"), "x")
        .await
        .unwrap();

    let listed: Vec<_> = users
        .list(10_000, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.id)
        .collect();
    assert!(listed.contains(&human), "the human must still be listed");
    assert!(
        !listed.contains(&svc),
        "a service account must not appear in the admin user list"
    );
}

#[tokio::test]
async fn service_account_is_still_findable_by_id() {
    // The filter belongs to ENUMERATION only. Resolving a service account by id
    // must keep working, or its name cannot be rendered next to its own grants
    // and audit rows.
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let users = UserRepo::new(db.clone());
    let svc = users
        .insert_service(&unique_email("svc.y"), "y")
        .await
        .unwrap();
    assert!(users.find_by_id(svc).await.unwrap().is_some());
}
