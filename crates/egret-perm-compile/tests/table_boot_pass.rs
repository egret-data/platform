//! D2 (spec §5.6, C13): the boot pass that compiles StarRocks whole-table
//! classifications written before the upgrade, one transaction per profile.
//!
//! Its own binary: the pass recompiles every StarRocks profile in the private
//! database `egret-test-db` hands this process that reaches a whole-table
//! classification.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test table_boot_pass

#[path = "support/lakekeeper_stub.rs"]
mod lakekeeper_stub;

use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Selector};
use egret_perm_compile::{recompile_service, recompile_table_classified_profiles};
use egret_policy::{PolicyStore, COMPILER_CLEARANCE_PREFIX, COMPILER_RULE_PREFIX};
use uuid::Uuid;

/// The pass is installation-wide: tests that assert on what it did run alone.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Profile {
    service_id: i64,
    warehouse_id: Uuid,
    lakekeeper: Uuid,
    catalog: String,
}

/// A StarRocks profile with one engine and one registry catalog over a fresh
/// warehouse linked to the stub catalog.
async fn starrocks_profile(db: &Db, policy: &PolicyStore) -> Profile {
    let pool = &db.pool;
    egret_policy::seed_starrocks(policy)
        .await
        .expect("seed starrocks");
    let service = policy
        .create_service(&unique("sr_boot"), "starrocks", &serde_json::json!({}))
        .await
        .expect("service");
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'Boot') RETURNING id")
            .bind(unique("boot-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-boot-test','AKIATEST') RETURNING id",
    )
    .bind(unique("boot-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("boot_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret',$3,$2) RETURNING id",
    )
    .bind(unique("boot-eng"))
    .bind(service.id)
    .bind(serde_json::json!({
        "starrocksConfig": {"feImage": egret_perm::DEFAULT_ENGINE_IMAGES.starrocks_fe_image}
    }))
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("boot_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    let lakekeeper = lakekeeper_stub::link_warehouse(db, warehouse_id).await;
    Profile {
        service_id: service.id,
        warehouse_id,
        lakekeeper,
        catalog,
    }
}

async fn new_tag(db: &Db) -> (Uuid, String) {
    let key = unique("boot-tag");
    let id = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(&key)
        .bind(unique("BootTag"))
        .fetch_one(&db.pool)
        .await
        .expect("tag");
    (id, key)
}

/// Attach `tag` to `sales.{table}`; `column` `""` is the whole table. Written
/// directly, with no recompile: the state an upgrade finds.
async fn attach(db: &Db, tag: Uuid, p: &Profile, table: &str, column: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales',$3,$4)",
    )
    .bind(tag)
    .bind(p.warehouse_id)
    .bind(table)
    .bind(column)
    .execute(&db.pool)
    .await
    .expect("attach");
}

async fn gate(db: &Db, tag: Uuid) {
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}')",
    )
    .bind(tag)
    .bind(vec![unique("boot-role")])
    .execute(&db.pool)
    .await
    .expect("gate");
}

/// A classification row filter on `key`, stored with no recompile.
async fn row_filter(perm: &PermStore, key: &str, expr: &str) -> Uuid {
    perm.create_rule(&Rule {
        id: Uuid::nil(),
        name: unique("boot-rowfilter"),
        kind: RuleKind::RowFilter {
            filter_expr: expr.to_string(),
        },
        selector: Selector::Tag {
            key: key.to_string(),
        },
        exempt: PrincipalSet::default(),
    })
    .await
    .expect("rule")
}

async fn names(db: &Db, service_id: i64) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM policy.authz_policies WHERE service_id = $1 ORDER BY name")
        .bind(service_id)
        .fetch_all(&db.pool)
        .await
        .expect("names")
}

async fn policy_version(db: &Db, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(&db.pool)
        .await
        .expect("version")
}

async fn last_outcome(db: &Db, service_id: i64) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT outcome, error FROM perm.compile_runs WHERE service_id = $1")
        .bind(service_id)
        .fetch_optional(&db.pool)
        .await
        .expect("compile run")
}

/// C13: a whole-table clearance and a classification row filter written before
/// the upgrade compile at the first boot, with no other write; a second boot
/// writes nothing.
#[tokio::test]
async fn the_boot_pass_compiles_table_level_classifications_written_before_upgrade() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(db.pool.clone());
    let p = starrocks_profile(&db, &policy).await;
    lakekeeper_stub::add_table(&db, p.lakekeeper, &["sales"], "orders", &["id", "email"]).await;
    let (tag, key) = new_tag(&db).await;
    attach(&db, tag, &p, "orders", "").await;
    gate(&db, tag).await;
    let rule = row_filter(&perm, &key, "region = 'us'").await;
    assert!(
        names(&db, p.service_id).await.is_empty(),
        "nothing compiled yet"
    );

    let compiled = recompile_table_classified_profiles(&db.pool)
        .await
        .expect("boot pass");
    assert!(compiled >= 1, "{compiled}");

    let c = &p.catalog;
    let names = names(&db, p.service_id).await;
    for want in [
        format!("{COMPILER_CLEARANCE_PREFIX}{tag}-{c}.sales.orders.*"),
        format!("{COMPILER_CLEARANCE_PREFIX}{tag}-mask-{c}.sales.orders.id"),
        format!("{COMPILER_CLEARANCE_PREFIX}{tag}-mask-{c}.sales.orders.email"),
        format!("{COMPILER_RULE_PREFIX}{rule}-{c}.sales.orders"),
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }

    let version = policy_version(&db, p.service_id).await;
    recompile_table_classified_profiles(&db.pool)
        .await
        .expect("second boot");
    assert_eq!(
        policy_version(&db, p.service_id).await,
        version,
        "an unchanged recompile writes nothing"
    );
}

/// C13/C14: one transaction per profile. A profile whose first recompile
/// creates a differing row-filter tie (a classification row filter that now
/// reaches a whole table where another one already filters) keeps its
/// previous compile and is recorded as failed; the other profile compiles.
#[tokio::test]
async fn one_refusing_profile_does_not_stop_the_others() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(db.pool.clone());

    // Profile 1: rule X filters `orders` through a column classification and
    // is compiled; rule Y, on a whole-table classification of the same table,
    // compiled nothing before the upgrade.
    let p1 = starrocks_profile(&db, &policy).await;
    let (tag_x, key_x) = new_tag(&db).await;
    attach(&db, tag_x, &p1, "orders", "email").await;
    row_filter(&perm, &key_x, "region = 'us'").await;
    recompile_service(&perm, &policy, p1.service_id)
        .await
        .expect("compile rule X");
    let before = names(&db, p1.service_id).await;
    let (tag_y, key_y) = new_tag(&db).await;
    attach(&db, tag_y, &p1, "orders", "").await;
    row_filter(&perm, &key_y, "region = 'eu'").await;

    // Profile 2: a whole-table clearance, nothing compiled.
    let p2 = starrocks_profile(&db, &policy).await;
    let (tag_z, _) = new_tag(&db).await;
    attach(&db, tag_z, &p2, "ghost", "").await;
    gate(&db, tag_z).await;

    recompile_table_classified_profiles(&db.pool)
        .await
        .expect("boot pass");

    assert_eq!(
        names(&db, p1.service_id).await,
        before,
        "the refused profile keeps its previous compile"
    );
    let (outcome, error) = last_outcome(&db, p1.service_id).await.expect("recorded");
    assert_eq!(outcome, "error");
    assert!(
        error.as_deref().unwrap_or("").contains("order-ambiguous"),
        "{error:?}"
    );
    assert!(
        names(&db, p2.service_id).await.contains(&format!(
            "{COMPILER_CLEARANCE_PREFIX}{tag_z}-{}.sales.ghost.*",
            p2.catalog
        )),
        "the other profile compiled"
    );
    assert_eq!(
        last_outcome(&db, p2.service_id).await.map(|(o, _)| o),
        Some("ok".to_string())
    );
}
