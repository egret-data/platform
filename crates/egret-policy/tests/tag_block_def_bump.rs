//! Migration 0190: the `tagPolicies` block now carries the `tag` service-def
//! (`ranger/api/download.rs`), which changes the bytes served without changing
//! any `policy_version`. Plugins cache by version, so the migration bumps every
//! profile that names a `tagService` — the only downloads carrying the block —
//! and nothing else.
//!
//! The migration has already run on the test DB, so its statement is executed
//! again here, inside a transaction that is rolled back, against rows this test
//! made: the linked profiles (one whose tag service is `trino`-typed, the live
//! shape, and one whose tag service is `tag`-typed) move by exactly one, and a
//! profile with no `tagService`, a profile with `"tagService": null`, and the
//! tag services themselves do not move.
use egret_metastore::Db;
use egret_policy::{seed_tag, seed_trino, PolicyStore};
use serde_json::json;

const MIGRATION: &str = include_str!("../../../migrations/0190_tag_block_serves_the_tag_def.sql");

fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn the_migration_bumps_exactly_the_profiles_that_carry_a_tag_block() {
    let Some(url) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.unwrap();
    seed_trino(&store).await.unwrap();

    let trino_typed_tag = uniq("m190_ttag");
    let tag_typed_tag = uniq("m190_tag");
    let mut ids = Vec::new();
    for (name, typ, cfg) in [
        (trino_typed_tag.clone(), "trino", json!({})),
        (tag_typed_tag.clone(), "tag", json!({})),
        (
            uniq("m190_a"),
            "trino",
            json!({ "tagService": trino_typed_tag }),
        ),
        (
            uniq("m190_b"),
            "trino",
            json!({ "tagService": tag_typed_tag }),
        ),
        (uniq("m190_none"), "trino", json!({})),
        (uniq("m190_null"), "trino", json!({ "tagService": null })),
    ] {
        ids.push(store.create_service(&name, typ, &cfg).await.unwrap().id);
    }

    async fn versions(conn: &mut sqlx::PgConnection, ids: &[i64]) -> Vec<i64> {
        let mut out = Vec::new();
        for id in ids {
            out.push(
                sqlx::query_scalar::<_, i64>(
                    "SELECT policy_version FROM policy.authz_services WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
            );
        }
        out
    }

    let mut tx = db.pool.begin().await.unwrap();
    let before = versions(&mut tx, &ids).await;
    sqlx::raw_sql(MIGRATION).execute(&mut *tx).await.unwrap();
    let after = versions(&mut tx, &ids).await;
    tx.rollback().await.unwrap();

    let moved: Vec<i64> = before.iter().zip(&after).map(|(b, a)| a - b).collect();
    assert_eq!(
        moved,
        vec![0, 0, 1, 1, 0, 0],
        "[trino-typed tag svc, tag-typed tag svc, linked→trino-typed, linked→tag-typed, \
         no tagService, null tagService]"
    );
}
