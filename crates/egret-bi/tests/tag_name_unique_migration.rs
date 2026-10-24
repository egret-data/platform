//! Migration 0207: classification names become unique ignoring case, and the
//! duplicates stored before it are renamed — then the server's boot pass
//! recompiles what the renames left stale.
//!
//! The migration has already run on the test DB, so this re-runs its SQL
//! against duplicates seeded with the index (and the queue table it creates)
//! dropped first. That is DDL on `tags`, which is why this is its own test
//! BINARY: `egret-test-db` gives each process a private database, so nothing
//! else sees the index missing. One test, for the same reason.
//!
//! What it pins: the oldest classification of a group keeps its name, the
//! others become `<name> (<key>)` (suffixed when that is taken too), each
//! rename is queued and audited, the index then refuses a duplicate, the
//! migration bumps no `tag_version` (see the migration's header for why), and
//! `TagRepo::recompile_queued_renames` moves the compiled Trino tag policy to
//! the new name, bumps the tag versions, and empties the queue.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_bi::TagRepo;
use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Selector};
use egret_policy::PolicyStore;
use uuid::Uuid;

const MIGRATION: &str = include_str!("../../../migrations/0207_tags_name_unique.sql");

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    profile_id: i64,
    tag_service_id: i64,
    warehouse_id: Uuid,
}

/// A Trino profile + its tag service, bound through an engine and a registry
/// catalog to a warehouse (as in `tag_rename_recompiles.rs`).
async fn seed_trino_fixture(db: &Db) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_m207");
    let tag_service_name = format!("{profile_name}_tag");
    let tag_service = policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("create trino profile");

    let warehouse_id = egret_test_db::seed_warehouse(pool).await;
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("m207-eng"))
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("m207_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    Fixture {
        profile_id: profile.id,
        tag_service_id: tag_service.id,
        warehouse_id,
    }
}

async fn insert_tag(pool: &sqlx::PgPool, key: &str, name: &str, age_minutes: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO tags (key, name, created_at) \
         VALUES ($1, $2, now() - make_interval(mins => $3)) RETURNING id",
    )
    .bind(key)
    .bind(name)
    .bind(age_minutes)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

async fn name_of(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("tag name")
}

async fn compiled_tag_name(pool: &sqlx::PgPool, service_id: i64, policy_name: &str) -> String {
    let body: serde_json::Value = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(policy_name)
    .fetch_one(pool)
    .await
    .expect("the compiled tag policy must exist under the tag service");
    body["resources"]["tag"]["values"][0]
        .as_str()
        .expect("a tag policy names a tag")
        .to_string()
}

async fn tag_version(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT tag_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("tag_version")
}

#[tokio::test]
async fn duplicates_are_renamed_queued_audited_then_recompiled_at_boot() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());
    let f = seed_trino_fixture(&db).await;

    // The state before 0207: no index, no queue.
    sqlx::raw_sql("DROP INDEX tags_name_lower_key; DROP TABLE tag_rename_recompiles")
        .execute(&pool)
        .await
        .expect("undo 0207's DDL");

    // `tags.key` allows only lowercase ascii alphanumerics, ':' or '-'.
    let s = Uuid::new_v4().simple().to_string()[..12].to_string();
    let (keeper_key, dup_key, third_key) = (format!("k-{s}"), format!("d-{s}"), format!("t-{s}"));
    let name = format!("Dup {s}");
    let keeper = insert_tag(&pool, &keeper_key, &name, 60).await;
    // Already holds the dup's first-choice new name, so the dup gets ` 2`.
    let squatter_name = format!("DUP {s} ({dup_key})");
    let squatter = insert_tag(&pool, &format!("q-{s}"), &squatter_name, 50).await;
    let dup = insert_tag(&pool, &dup_key, &format!("DUP {s}"), 40).await;
    let third = insert_tag(&pool, &third_key, &format!("dup {s}"), 30).await;

    // The dup carries a mask rule and a column, so a policy names it.
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(dup)
    .bind(f.warehouse_id)
    .execute(&pool)
    .await
    .expect("assign the dup to a column");
    let rule_id = perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("m207-rule"),
            kind: RuleKind::Mask {
                mask_type: "MASK".to_string(),
                value_expr: None,
            },
            selector: Selector::Tag {
                key: dup_key.clone(),
            },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("create rule");
    egret_perm_compile::recompile_service(&perm, &policy, f.profile_id)
        .await
        .expect("first compile");
    let policy_name = format!("perm-rule-{rule_id}");
    assert_eq!(
        compiled_tag_name(&pool, f.tag_service_id, &policy_name).await,
        format!("DUP {s}")
    );
    let profile_tv = tag_version(&pool, f.profile_id).await;
    let tag_service_tv = tag_version(&pool, f.tag_service_id).await;

    sqlx::raw_sql(MIGRATION)
        .execute(&pool)
        .await
        .expect("migration 0207");

    let dup_new = format!("DUP {s} ({dup_key}) 2");
    let third_new = format!("dup {s} ({third_key})");
    assert_eq!(
        name_of(&pool, keeper).await,
        name,
        "the oldest keeps its name"
    );
    assert_eq!(name_of(&pool, squatter).await, squatter_name);
    assert_eq!(name_of(&pool, dup).await, dup_new);
    assert_eq!(name_of(&pool, third).await, third_new);

    let queued: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT q.tag_id, q.old_name FROM tag_rename_recompiles q \
             JOIN tags t ON t.id = q.tag_id ORDER BY t.created_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        queued,
        vec![(dup, format!("DUP {s}")), (third, format!("dup {s}"))]
    );

    let audited: Vec<(String, Option<Uuid>, serde_json::Value)> = sqlx::query_as(
        "SELECT resource_id, actor_user_id, detail FROM audit.audit_log \
          WHERE kind = 'mutation' AND action = 'tag:rename' AND resource_type = 'tag' \
            AND resource_id = ANY($1) ORDER BY id",
    )
    .bind(vec![dup.to_string(), third.to_string()])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(audited.len(), 2, "{audited:?}");
    assert_eq!(audited[0].0, dup.to_string());
    assert_eq!(audited[0].1, None, "the system made it");
    assert_eq!(audited[0].2["before"]["name"], format!("DUP {s}"));
    assert_eq!(audited[0].2["after"]["name"], dup_new);

    let refused = sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $2)")
        .bind(format!("x-{s}"))
        .bind(name.to_uppercase())
        .execute(&pool)
        .await
        .expect_err("the index refuses a duplicate");
    assert!(
        refused
            .as_database_error()
            .is_some_and(|e| e.constraint() == Some("tags_name_lower_key")),
        "{refused:?}"
    );

    // SQL cannot recompile: the policy is stale until the boot pass, and no
    // plugin was told to refetch the new tag map meanwhile.
    assert_eq!(
        compiled_tag_name(&pool, f.tag_service_id, &policy_name).await,
        format!("DUP {s}")
    );
    assert_eq!(tag_version(&pool, f.profile_id).await, profile_tv);

    let tags = TagRepo::new(db.clone());
    assert_eq!(tags.recompile_queued_renames().await.expect("boot pass"), 2);
    assert_eq!(
        compiled_tag_name(&pool, f.tag_service_id, &policy_name).await,
        dup_new,
        "the boot pass recompiles the policy to the new name"
    );
    assert_eq!(tag_version(&pool, f.profile_id).await, profile_tv + 1);
    assert_eq!(
        tag_version(&pool, f.tag_service_id).await,
        tag_service_tv + 1
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tag_rename_recompiles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "the queue is emptied in the same transaction");

    assert_eq!(tags.recompile_queued_renames().await.unwrap(), 0);
    assert_eq!(
        tag_version(&pool, f.profile_id).await,
        profile_tv + 1,
        "a later boot with nothing queued changes nothing"
    );
}
