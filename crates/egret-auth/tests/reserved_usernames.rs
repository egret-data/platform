//! View governance Task 7: no human may hold a username egret runs as on an
//! engine. `egret-lineage` reads every governed table (spec R5), and an engine
//! JWT carries `users.username` as `preferred_username`, so a human given that
//! name would hold its grants.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_auth::users::UserRepo;
use egret_core::engine_principals::LINEAGE_PRINCIPAL;
use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn user_with_email(db: &Db, email: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(email)
    .fetch_one(&db.pool)
    .await
    .expect("insert user");
    UserId(id)
}

/// Both spellings slug to `egret-lineage`; neither may receive it.
#[tokio::test]
async fn an_email_that_slugs_to_the_lineage_principal_gets_another_username() {
    let Some(db) = connect().await else { return };
    let repo = UserRepo::new(db.clone());
    for local in ["egret-lineage", "Egret.Lineage"] {
        let uid = user_with_email(&db, &format!("{local}@{}.test", Uuid::new_v4().simple())).await;
        let got = repo.ensure_username(uid).await.expect("username");
        assert_ne!(got, LINEAGE_PRINCIPAL, "{local}");
        assert!(got.starts_with("egret-lineage-"), "{local}: {got}");
    }
}

/// The schema refuses the name outright, whatever path writes it.
#[tokio::test]
async fn the_lineage_principal_is_unrepresentable_as_a_username() {
    let Some(db) = connect().await else { return };
    let uid = user_with_email(&db, &format!("{}@test.local", Uuid::new_v4())).await;
    for name in [LINEAGE_PRINCIPAL, "EGRET-LINEAGE"] {
        let err = sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
            .bind(uid.0)
            .bind(name)
            .execute(&db.pool)
            .await
            .expect_err(name);
        assert!(
            err.to_string().contains("users_username_not_reserved"),
            "{name}: {err}"
        );
    }
}
