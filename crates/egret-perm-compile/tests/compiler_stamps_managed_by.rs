//! The recompile driver's INSERT stamps `managed_by = 'compiler'`.
//!
//! The migration backfills existing rows; this covers the rows the compiler
//! writes AFTER it, which would silently become `'operator'` (the column
//! default) if the stamp in `trigger.rs` regressed. The scenario mirrors
//! `tag_service_routing`: one tag-selected mask rule compiled into a Trino
//! profile and its tag service. Reads are scoped to this fixture's two
//! `service_id`s.

use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Selector};
use egret_perm_compile::recompile_service;
use egret_policy::infra_names::is_egret_infrastructure_policy;
use egret_policy::{is_compiler_owned_policy_name, PolicyStore};
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

#[tokio::test]
async fn compiled_policies_are_stamped_compiler() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());

    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_stamp");
    let tag_service_name = format!("{profile_name}_tag");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("create profile");
    let tag_service = policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");

    let tag_key = unique("stamp-pii");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, 'Stamp PII')")
        .bind(&tag_key)
        .execute(&pool)
        .await
        .expect("insert tag");
    perm.create_rule(&Rule {
        id: Uuid::nil(),
        name: unique("stamp-rule"),
        kind: RuleKind::Mask {
            mask_type: "MASK".to_string(),
            value_expr: None,
        },
        selector: Selector::Tag { key: tag_key },
        exempt: PrincipalSet::default(),
    })
    .await
    .expect("create rule");

    recompile_service(&perm, &policy, profile.id)
        .await
        .expect("recompile");

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, managed_by FROM policy.authz_policies \
         WHERE service_id = ANY($1) ORDER BY name",
    )
    .bind(vec![profile.id, tag_service.id])
    .fetch_all(&pool)
    .await
    .expect("read back");

    // Control: the compile must have produced something, or the loop is vacuous.
    assert!(!rows.is_empty(), "recompile wrote no policy rows");
    for (name, managed_by) in &rows {
        assert_eq!(managed_by, "compiler", "{name:?} was not stamped compiler");
        assert!(is_compiler_owned_policy_name(name), "{name:?}");
        // "trino" is the type on which the predicate claims the most names.
        assert!(!is_egret_infrastructure_policy("trino", name), "{name:?}");
    }
}
