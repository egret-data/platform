//! fix/ranger-table-mask-column: the boot pass that recompiles every profile a
//! path mask naming no column reaches, so a body the old compiler stored —
//! with no `column` resource, which masks nothing on Ranger — is replaced by
//! the fixed one (`column: *`) without waiting for an unrelated write.
//!
//! Its own binary: the pass is installation-wide over the private database
//! `egret-test-db` hands this process.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test path_mask_boot_pass

use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Scope, Selector};
use egret_perm_compile::{recompile_path_mask_profiles, recompile_service};
use egret_policy::{PolicyStore, COMPILER_RULE_PREFIX};
use serde_json::Value;
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

/// A StarRocks profile with one engine bound to it and one registry catalog
/// attached. Returns `(service_id, catalog_id)`.
async fn starrocks_profile(db: &Db, policy: &PolicyStore) -> (i64, Uuid) {
    let pool = &db.pool;
    egret_policy::seed_starrocks(policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_pathmask"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name) VALUES ($1,'Path mask') RETURNING id",
    )
    .bind(unique("pathmask-ws"))
    .fetch_one(pool)
    .await
    .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-pathmask-test','AKIATEST') RETURNING id",
    )
    .bind(unique("pathmask-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("pathmask_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("pathmask-eng"))
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
    .bind(unique("pathmask_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
    (service.id, catalog_id)
}

async fn path_mask(perm: &PermStore, catalog_id: Uuid, column: Option<&str>) -> Uuid {
    perm.create_rule(&Rule {
        id: Uuid::nil(),
        name: unique("pathmask"),
        kind: RuleKind::Mask {
            mask_type: "MASK_HASH".to_string(),
            value_expr: None,
        },
        selector: Selector::Path(Scope {
            catalog_id,
            schema: Some("sales".to_string()),
            table: Some("orders".to_string()),
            column: column.map(str::to_string),
        }),
        exempt: PrincipalSet::default(),
    })
    .await
    .expect("rule")
}

async fn body(db: &Db, service_id: i64, rule_id: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(format!("{COMPILER_RULE_PREFIX}{rule_id}"))
    .fetch_one(&db.pool)
    .await
    .expect("compiled mask")
}

/// Run `update` (an `UPDATE` keyed on `service_id = $1 AND name = $2`) on the
/// stored body, as an older compiler would have left it.
async fn age_body(db: &Db, service_id: i64, rule_id: Uuid, update: &'static str) {
    sqlx::query(update)
        .bind(service_id)
        .bind(format!("{COMPILER_RULE_PREFIX}{rule_id}"))
        .execute(&db.pool)
        .await
        .expect("age the stored body");
}

async fn policy_version(db: &Db, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(&db.pool)
        .await
        .expect("policy_version")
}

/// A table mask stored by the old compiler (no `column`) is rewritten to
/// `column: *` by the boot pass, with no other write, and the profile's
/// `policy_version` moves so the plugin downloads it; a second boot writes
/// nothing. A profile whose only mask NAMES its column is outside the pass.
#[tokio::test]
async fn the_boot_pass_replaces_a_table_mask_the_old_compiler_stored_without_a_column() {
    let Some(db) = test_db().await else { return };
    let _serial = SERIAL.lock().await;
    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());

    let (svc, catalog) = starrocks_profile(&db, &policy).await;
    let table_mask = path_mask(&perm, catalog, None).await;
    let (control_svc, control_catalog) = starrocks_profile(&db, &policy).await;
    let column_mask = path_mask(&perm, control_catalog, Some("email")).await;
    for s in [svc, control_svc] {
        recompile_service(&perm, &policy, s)
            .await
            .expect("recompile");
    }
    assert_eq!(
        body(&db, svc, table_mask).await["resources"]["column"]["values"],
        serde_json::json!(["*"]),
        "today's compiler renders a table mask as column `*`"
    );

    // The body the old compiler stored: no `column` resource at all.
    age_body(
        &db,
        svc,
        table_mask,
        "UPDATE policy.authz_policies SET policy_jsonb = policy_jsonb #- '{resources,column}' \
          WHERE service_id = $1 AND name = $2",
    )
    .await;
    assert!(body(&db, svc, table_mask).await["resources"]
        .get("column")
        .is_none());
    // A control the pass must NOT reach: a stale mask type on a column mask.
    age_body(
        &db,
        control_svc,
        column_mask,
        "UPDATE policy.authz_policies SET policy_jsonb = jsonb_set(policy_jsonb, \
         '{dataMaskPolicyItems,0,dataMaskInfo,dataMaskType}', '\"MASK_NULL\"'::jsonb) \
          WHERE service_id = $1 AND name = $2",
    )
    .await;
    let before = policy_version(&db, svc).await;
    let control_before = policy_version(&db, control_svc).await;

    let compiled = recompile_path_mask_profiles(&db.pool)
        .await
        .expect("boot pass");
    assert!(compiled >= 1, "the pass compiled the reached profile");

    let repaired = body(&db, svc, table_mask).await;
    assert_eq!(
        repaired["resources"]["column"]["values"],
        serde_json::json!(["*"]),
        "the boot pass replaces the column-less body: {repaired:#}"
    );
    let after_first = policy_version(&db, svc).await;
    assert!(
        after_first > before,
        "rewriting the body bumps the profile's policy_version ({before} -> {after_first})"
    );
    assert_eq!(
        policy_version(&db, control_svc).await,
        control_before,
        "a profile whose only mask names a column is not recompiled by the pass"
    );
    assert_eq!(
        body(&db, control_svc, column_mask).await["dataMaskPolicyItems"][0]["dataMaskInfo"]
            ["dataMaskType"],
        "MASK_NULL",
        "the control's stored body is untouched"
    );

    recompile_path_mask_profiles(&db.pool)
        .await
        .expect("second boot pass");
    assert_eq!(
        policy_version(&db, svc).await,
        after_first,
        "a second boot changes nothing, so it writes nothing"
    );
}
