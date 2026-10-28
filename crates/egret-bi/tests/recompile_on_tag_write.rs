//! SP4a Task 7: proves the WIRING, not just the pure driver — that
//! `CatalogTagRepo::assign_catalog`/`unassign_catalog` (the real
//! HTTP-adjacent write path egret's tag UI goes through) actually calls
//! `egret_perm_compile::recompile_service` for every StarRocks profile bound
//! to the warehouse, alongside the `tag_version` bump
//! `catalog_tag_version_bump.rs` already covers. `crates/egret-perm-compile/tests/recompile.rs`
//! covers `recompile_service`'s own diff correctness (value-level, not just
//! counts) directly; this file only has to show the trigger is actually
//! HOOKED to the tag-write path, so it asserts scoped counts.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::TagRepo;
use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Selector};
use egret_policy::PolicyStore;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    warehouse_id: Uuid,
    service_id: i64,
}

/// A StarRocks profile + engine + registry catalog + one `perm.rules` masking
/// rule selecting tag `pii` — no column tagged yet (unlike
/// `egret-perm-compile/tests/recompile.rs`'s fixture, which pre-tags one so
/// its own no-op test has something to hold steady; this file's whole point
/// is watching the FIRST tag write happen through the real repo).
async fn seed_starrocks_fixture(db: &Db, tag_key: &str) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_wire"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("wire-ws"))
            .bind("Wire WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-wire-test','AKIATEST') RETURNING id",
    )
    .bind(unique("wire-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("wire_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("wire-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("wire_cat"))
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

    perm.create_rule(&Rule {
        id: Uuid::nil(),
        name: unique("wire-pii-hash"),
        kind: RuleKind::Mask {
            mask_type: "MASK_HASH".to_string(),
            value_expr: None,
        },
        selector: Selector::Tag {
            key: tag_key.to_string(),
        },
        exempt: PrincipalSet::default(),
    })
    .await
    .expect("create rule");

    Fixture {
        warehouse_id,
        service_id: service.id,
    }
}

async fn policy_count(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("count policies")
}

/// The `column` resource names of every StarRocks masking policy under
/// `service_id`, sorted — a value-level view of "which columns are
/// currently masked," not just how many. The `perm-hidden-` deny per masked
/// table also lives under `service_id`, so only the `perm-rule-` masks count.
async fn masked_columns(pool: &sqlx::PgPool, service_id: i64) -> Vec<String> {
    let bodies: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 AND name LIKE 'perm-rule-%'",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .expect("policy bodies");
    let mut cols: Vec<String> = bodies
        .into_iter()
        .map(|(b,)| {
            b["resources"]["column"]["values"][0]
                .as_str()
                .expect("every StarRocks masking policy here names a column")
                .to_string()
        })
        .collect();
    cols.sort();
    cols
}

#[tokio::test]
async fn assign_and_unassign_through_the_real_repo_recompile_the_bound_starrocks_profile() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();

    // `perm.rules.tag_key` FK-references `tags.key` (migration 0154) — the
    // tag row must exist before the fixture's rule can reference it.
    let by: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("wire_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .expect("insert user");
    let tags = TagRepo::new(db.clone());
    let repo = CatalogTagRepo::new(db.clone());
    // `tags.key` allows only lowercase ascii alphanumerics, ':' or '-' — no
    // underscore, unlike `unique()`'s usual `_`-joined names.
    let tag_key = format!("wire-pii-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let tag = tags
        .create(&tag_key, "PII", "Personal data", "#ff0000", Some(by))
        .await
        .expect("create tag");

    let f = seed_starrocks_fixture(&db, &tag_key).await;
    assert_eq!(
        policy_count(&pool, f.service_id).await,
        0,
        "no column is tagged yet — nothing should be compiled"
    );

    // Assign through the REAL repo method (the one the HTTP handler calls) —
    // this is what proves the trigger is wired into the write path, not just
    // that `recompile_service` itself works when called directly.
    repo.assign_catalog(tag.id, f.warehouse_id, "sales", "orders", "email", Some(by))
        .await
        .expect("assign (and recompile) email");
    assert_eq!(
        policy_count(&pool, f.service_id).await,
        2,
        "assign_catalog must recompile the bound StarRocks profile — one masking policy for the tagged column, \
         plus the hidden-column deny for its table"
    );
    assert_eq!(
        masked_columns(&pool, f.service_id).await,
        vec!["email".to_string()],
        "the one compiled policy must name the column actually tagged"
    );

    repo.assign_catalog(tag.id, f.warehouse_id, "sales", "orders", "phone", Some(by))
        .await
        .expect("assign (and recompile) phone");
    assert_eq!(
        policy_count(&pool, f.service_id).await,
        3,
        "a second tagged column must recompile to a second masking policy (the table's one hidden deny is shared)"
    );
    assert_eq!(
        masked_columns(&pool, f.service_id).await,
        vec!["email".to_string(), "phone".to_string()],
        "both tagged columns must be masked, and only those two"
    );

    repo.unassign_catalog(tag.id, f.warehouse_id, "sales", "orders", "phone")
        .await
        .expect("unassign (and recompile) phone");
    assert_eq!(
        policy_count(&pool, f.service_id).await,
        2,
        "untagging a column must recompile it OUT of the StarRocks profile — the enumeration \
         D5 says would otherwise stay stale"
    );
    // Value-level, not just a count: the SURVIVING policy must be the one
    // for "email", never "phone" left behind under a different id and
    // "email" dropped by mistake — a count-only assertion cannot tell those
    // two outcomes apart, and this plan has already shipped two Criticals of
    // exactly that "right count, wrong value" shape.
    assert_eq!(
        masked_columns(&pool, f.service_id).await,
        vec!["email".to_string()],
        "the remaining policy must be for the still-tagged column, not the untagged one"
    );
}
