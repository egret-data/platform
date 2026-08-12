//! Render-token lifecycle. The security properties live here: single use, short
//! life, and a cleartext token that is never stored.
use egret_automation::render::token::RenderTokenRepo;
use egret_automation::AutomationError;
use egret_metastore::Db;
use std::time::Duration;
use uuid::Uuid;

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
        .bind(format!("r_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_minted_token_redeems_once_and_only_once(pool: sqlx::PgPool) {
    // Single-use is the property that keeps a leaked token harmless. A second
    // redemption must find nothing at all, not a row flagged as used.
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let object = Uuid::new_v4();

    let token = repo
        .mint("dashboard", object, owner, Duration::from_secs(60))
        .await
        .unwrap();
    let grant = repo.redeem(&token).await.expect("first redemption works");
    assert_eq!(grant.kind, "dashboard");
    assert_eq!(grant.object_id, object);
    assert_eq!(grant.owner_id, owner);

    let second = repo.redeem(&token).await;
    assert!(
        matches!(second, Err(AutomationError::NotFound(_))),
        "got {second:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_cleartext_token_is_never_stored(pool: sqlx::PgPool) {
    // Asserted by searching the stored row, not by trusting the column name.
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let token = repo
        .mint("chart", Uuid::new_v4(), owner, Duration::from_secs(60))
        .await
        .unwrap();

    let rows: Vec<String> = sqlx::query_scalar("SELECT t::text FROM automation.render_tokens t")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].contains(&token),
        "cleartext token found in the row: {}",
        rows[0]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_token_is_refused(pool: sqlx::PgPool) {
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let token = repo
        .mint("chart", Uuid::new_v4(), owner, Duration::from_secs(60))
        .await
        .unwrap();
    sqlx::query("UPDATE automation.render_tokens SET expires_at = now() - interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();

    assert!(matches!(
        repo.redeem(&token).await,
        Err(AutomationError::NotFound(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_token_is_refused(pool: sqlx::PgPool) {
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    assert!(matches!(
        repo.redeem("not-a-real-token").await,
        Err(AutomationError::NotFound(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_tokens_for_the_same_object_are_distinct(pool: sqlx::PgPool) {
    // Otherwise the UNIQUE index on the hash would make a second render fail.
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let object = Uuid::new_v4();
    let a = repo
        .mint("chart", object, owner, Duration::from_secs(60))
        .await
        .unwrap();
    let b = repo
        .mint("chart", object, owner, Duration::from_secs(60))
        .await
        .unwrap();
    assert_ne!(a, b);
}

#[sqlx::test(migrations = "../../migrations")]
async fn pruning_removes_only_expired_tokens(pool: sqlx::PgPool) {
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let live = repo
        .mint("chart", Uuid::new_v4(), owner, Duration::from_secs(600))
        .await
        .unwrap();
    let dead_object = Uuid::new_v4();
    let dead = repo
        .mint("chart", dead_object, owner, Duration::from_secs(600))
        .await
        .unwrap();
    // Fallback for servers without pgcrypto: expire by the dead token's known
    // object_id rather than hashing the live token to exclude it.
    sqlx::query(
        "UPDATE automation.render_tokens SET expires_at = now() - interval '1 hour' \
         WHERE object_id = $1",
    )
    .bind(dead_object)
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(repo.prune_expired().await.unwrap() >= 1);
    assert!(repo.redeem(&live).await.is_ok(), "the live token survived");
    assert!(repo.redeem(&dead).await.is_err());
}
