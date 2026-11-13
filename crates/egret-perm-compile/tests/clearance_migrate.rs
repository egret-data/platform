//! SP3c-2: clearance migrates into `perm.rules` rows of the `withhold` kind.
//!
//! DB-gated; needs DATABASE_URL (see `egret_test_db`).

use egret_metastore::Db;
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db.pool)
}

async fn seed_tag(pool: &PgPool) -> String {
    let key = format!("wh_{}", &Uuid::new_v4().simple().to_string()[..12]);
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&key)
        .execute(pool)
        .await
        .expect("seed tag");
    key
}

async fn insert_withhold(
    pool: &PgPool,
    tag_key: &str,
    mask_type: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, tag_key) \
         VALUES ($1, 'withhold', $2, 'tag', $3)",
    )
    .bind(format!("withhold-{tag_key}"))
    .bind(mask_type)
    .bind(tag_key)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn a_withhold_rule_with_no_payload_is_accepted() {
    let Some(pool) = pool().await else { return };
    let tag = seed_tag(&pool).await;
    insert_withhold(&pool, &tag, None)
        .await
        .expect("a payload-free withhold rule is valid");
}

/// SQLSTATE 23514 = check_violation.
fn is_check_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "23514")
}

#[tokio::test]
async fn a_withhold_rule_with_a_payload_is_rejected() {
    let Some(pool) = pool().await else { return };
    let tag = seed_tag(&pool).await;
    let err = insert_withhold(&pool, &tag, Some("MASK_NULL"))
        .await
        .expect_err("a withhold rule with a mask_type must be refused");
    // The 0154 per-kind CHECKs and `rules_withhold_has_no_payload` overlap on
    // payloads; either refusal is the contract.
    assert!(
        is_check_violation(&err),
        "expected a CHECK violation: {err}"
    );
}

#[tokio::test]
async fn a_withhold_rule_must_be_tag_selected() {
    let Some(pool) = pool().await else { return };
    let catalog_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM registry.catalogs LIMIT 1")
        .fetch_optional(&pool)
        .await
        .expect("catalogs");
    let Some(catalog_id) = catalog_id else {
        // No catalog to anchor a path selector on in this database.
        return;
    };
    let err = sqlx::query(
        "INSERT INTO perm.rules (name, kind, selector_kind, catalog_id) \
         VALUES ($1, 'withhold', 'path', $2)",
    )
    .bind(format!("withhold-path-{}", Uuid::new_v4()))
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect_err("a path-selected withhold rule must be refused");
    assert!(
        err.to_string().contains("rules_withhold_has_no_payload"),
        "expected the withhold CHECK to fire: {err}"
    );
}
