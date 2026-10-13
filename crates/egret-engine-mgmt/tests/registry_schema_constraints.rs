//! Migration 0147 (`registry` schema) exercised directly against a real
//! Postgres: every CHECK and FK the plan decisions describe is verified by a
//! test that tries to violate it, not just read back from `pg_constraint`.
//!
//! DB-gated: without `DATABASE_URL` this FAILS unless `EGRET_ALLOW_DB_SKIP` is
//! set, in which case every test prints SKIPPED and asserts nothing.

use egret_catalog_engine::CatalogStore;
use egret_engine_mgmt::EngineMgmtStore;
use egret_metastore::Db;
use uuid::Uuid;

async fn harness() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

macro_rules! harness_or_skip {
    () => {
        match harness().await {
            Some(db) => db,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

async fn seed_workspace(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("reg-{}", Uuid::new_v4().simple()))
    .bind(name)
    .fetch_one(&db.pool)
    .await
    .expect("seed workspace")
}

async fn seed_storage_connection(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("reg-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed storage connection")
}

/// A workspace + its own warehouse, as the registry's `warehouse_id` mirror expects.
async fn seed_warehouse(db: &Db) -> (Uuid, Uuid) {
    let catalog = CatalogStore::new(db.clone());
    let ws = seed_workspace(db, "Registry test workspace").await;
    let sc = seed_storage_connection(db).await;
    let wh = catalog
        .create_warehouse(&format!("reg-wh-{}", Uuid::new_v4().simple()), ws, sc)
        .await
        .expect("create warehouse");
    (ws, wh.id)
}

async fn seed_engine(store: &EngineMgmtStore) -> Uuid {
    store
        .create_engine(
            &format!("reg-eng-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .expect("seed engine")
        .id
}

fn is_check_violation(err: &sqlx::Error, constraint: &str) -> bool {
    matches!(err, sqlx::Error::Database(d) if d.constraint() == Some(constraint))
}

// -- catalogs_warehouse_mirrors_config --------------------------------------

/// An `iceberg`/`egret` catalog whose `warehouse_id` column disagrees with
/// `config.metastore.warehouse_id` is rejected; a matching pair is accepted.
#[tokio::test]
async fn catalog_warehouse_mirror_check_rejects_a_mismatch() {
    let db = harness_or_skip!();
    let (_ws, wh1) = seed_warehouse(&db).await;
    let (_ws2, wh2) = seed_warehouse(&db).await;

    // Mismatch: warehouse_id column names wh1, config names wh2.
    let mismatched = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
             jsonb_build_object('type','egret','warehouse_id', $2::text)), $3)",
    )
    .bind(format!("reg_mismatch_{}", Uuid::new_v4().simple()))
    .bind(wh2.to_string())
    .bind(wh1)
    .execute(&db.pool)
    .await;
    let err = mismatched.expect_err("mismatched warehouse_id must be rejected");
    assert!(
        is_check_violation(&err, "catalogs_warehouse_mirrors_config"),
        "{err:?}"
    );

    // warehouse_id set but kind is not iceberg/egret: also rejected.
    let wrong_kind = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'hive', '{}'::jsonb, $2)",
    )
    .bind(format!("reg_wrongkind_{}", Uuid::new_v4().simple()))
    .bind(wh1)
    .execute(&db.pool)
    .await;
    assert!(is_check_violation(
        &wrong_kind.expect_err("non-iceberg with warehouse_id must be rejected"),
        "catalogs_warehouse_mirrors_config"
    ));

    // Positive control: matching pair is accepted.
    let ok = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
             jsonb_build_object('type','egret','warehouse_id', $2::text)), $2)",
    )
    .bind(format!("reg_match_{}", Uuid::new_v4().simple()))
    .bind(wh1)
    .execute(&db.pool)
    .await;
    ok.expect("matching warehouse_id/config must be accepted");
}

// -- catalogs_storage_mirrors_config -----------------------------------------

/// `storage_connection_id` must agree with `config.storage_connection_id`
/// (including both-NULL); a mismatch is rejected.
#[tokio::test]
async fn catalog_storage_mirror_check_rejects_a_mismatch() {
    let db = harness_or_skip!();
    let sc1 = seed_storage_connection(&db).await;
    let sc2 = seed_storage_connection(&db).await;

    let mismatched = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, storage_connection_id) \
         VALUES ($1, 'generic', jsonb_build_object('storage_connection_id', $2::text), $3)",
    )
    .bind(format!("reg_scmismatch_{}", Uuid::new_v4().simple()))
    .bind(sc2.to_string())
    .bind(sc1)
    .execute(&db.pool)
    .await;
    assert!(is_check_violation(
        &mismatched.expect_err("mismatched storage_connection_id must be rejected"),
        "catalogs_storage_mirrors_config"
    ));

    // column set, config missing the key entirely (NULL) -> mismatch.
    let missing_in_config = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, storage_connection_id) \
         VALUES ($1, 'generic', '{}'::jsonb, $2)",
    )
    .bind(format!("reg_scmissing_{}", Uuid::new_v4().simple()))
    .bind(sc1)
    .execute(&db.pool)
    .await;
    assert!(is_check_violation(
        &missing_in_config.expect_err("column set / config unset must be rejected"),
        "catalogs_storage_mirrors_config"
    ));

    // Positive control: matching pair, and both-NULL, are accepted.
    let ok = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, storage_connection_id) \
         VALUES ($1, 'generic', jsonb_build_object('storage_connection_id', $2::text), $2)",
    )
    .bind(format!("reg_scmatch_{}", Uuid::new_v4().simple()))
    .bind(sc1)
    .execute(&db.pool)
    .await;
    ok.expect("matching storage_connection_id/config must be accepted");

    let ok_null = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb)",
    )
    .bind(format!("reg_scnull_{}", Uuid::new_v4().simple()))
    .execute(&db.pool)
    .await;
    ok_null.expect("both-NULL storage_connection_id must be accepted");
}

// -- catalogs_warehouse_id_key (partial unique index) ------------------------

/// One catalog per warehouse: a second catalog on the same `warehouse_id` is
/// rejected, but many catalogs with a NULL `warehouse_id` coexist.
#[tokio::test]
async fn one_catalog_per_warehouse_but_many_with_no_warehouse() {
    let db = harness_or_skip!();
    let (_ws, wh) = seed_warehouse(&db).await;

    let config = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
             jsonb_build_object('type','egret','warehouse_id', $2::text)), $2)",
    )
    .bind(format!("reg_wh1_{}", Uuid::new_v4().simple()))
    .bind(wh)
    .execute(&db.pool)
    .await;
    config.expect("first catalog on this warehouse must succeed");

    let dupe = sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
             jsonb_build_object('type','egret','warehouse_id', $2::text)), $2)",
    )
    .bind(format!("reg_wh2_{}", Uuid::new_v4().simple()))
    .bind(wh)
    .execute(&db.pool)
    .await;
    let err = dupe.expect_err("a second catalog on the same warehouse must be rejected");
    assert!(
        matches!(&err, sqlx::Error::Database(d) if d.constraint() == Some("catalogs_warehouse_id_key")),
        "{err:?}"
    );

    // Two NULL-warehouse catalogs must both succeed (partial unique index).
    for _ in 0..2 {
        sqlx::query("INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb)")
            .bind(format!("reg_nowh_{}", Uuid::new_v4().simple()))
            .execute(&db.pool)
            .await
            .expect("NULL-warehouse catalogs do not collide");
    }
}

// -- attachments_client_has_credential ---------------------------------------

/// An attachment naming a minted `catalog_client_id` without a
/// `credential_secret_id` is rejected.
#[tokio::test]
async fn attachment_with_client_requires_a_credential_secret() {
    let db = harness_or_skip!();
    let store = EngineMgmtStore::new(db.clone());
    let (_ws, _wh) = seed_warehouse(&db).await;
    let engine = seed_engine(&store).await;
    let catalog: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg_att_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed catalog");

    let missing_secret = sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, catalog_client_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(engine)
    .bind(catalog)
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await;
    assert!(is_check_violation(
        &missing_secret
            .expect_err("catalog_client_id without credential_secret_id must be rejected"),
        "attachments_client_has_credential"
    ));

    // Positive control: both set is accepted.
    let ok = sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments \
           (engine_id, catalog_id, catalog_client_id, credential_secret_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(engine)
    .bind(catalog)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await;
    ok.expect("catalog_client_id with credential_secret_id must be accepted");

    // A never-revoked client (minted_client = false, no id at all) is fine too.
    let catalog2: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg_att2_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed second catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, minted_client) \
         VALUES ($1, $2, false)",
    )
    .bind(engine)
    .bind(catalog2)
    .execute(&db.pool)
    .await
    .expect("no client_id at all must be accepted");
}

// -- engine delete cascades the attachment -----------------------------------

/// Deleting the engine row removes its attachments (`ON DELETE CASCADE`).
#[tokio::test]
async fn engine_delete_cascades_its_catalog_attachments() {
    let db = harness_or_skip!();
    let store = EngineMgmtStore::new(db.clone());
    let (_ws, _wh) = seed_warehouse(&db).await;
    let engine = seed_engine(&store).await;
    let catalog: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg_casc_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine)
    .bind(catalog)
    .execute(&db.pool)
    .await
    .expect("seed attachment");

    sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine)
        .execute(&db.pool)
        .await
        .expect("delete engine");

    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(engine)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(left, 0, "the attachment must cascade with the engine");
    // The catalog itself is untouched — only the attachment goes.
    let catalog_still_there: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registry.catalogs WHERE id = $1")
            .bind(catalog)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(catalog_still_there, 1);
}

// -- NO ACTION vs RESTRICT on catalog_id -------------------------------------

/// Deleting a workspace cascades workspace -> warehouse -> catalog -> the
/// workspace's own `catalog_workspaces` assignment row, all in the one DELETE
/// statement. A `RESTRICT` FK would fire immediately and block this; `NO
/// ACTION` defers the check to the end of the statement, by which time
/// nothing dangles.
#[tokio::test]
async fn workspace_delete_cascades_warehouse_catalog_and_its_own_assignment() {
    let db = harness_or_skip!();
    let (ws, wh) = seed_warehouse(&db).await;
    let catalog: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
             jsonb_build_object('type','egret','warehouse_id', $2::text)), $2) RETURNING id",
    )
    .bind(format!("reg_wsdel_{}", Uuid::new_v4().simple()))
    .bind(wh)
    .fetch_one(&db.pool)
    .await
    .expect("seed catalog");
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2)",
    )
    .bind(catalog)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("assign catalog to its own workspace");

    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(ws)
        .execute(&db.pool)
        .await
        .expect("workspace delete must cascade through warehouse -> catalog -> assignment in one statement");

    let wh_left: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog.warehouses WHERE id = $1")
        .bind(wh)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let cat_left: i64 = sqlx::query_scalar("SELECT count(*) FROM registry.catalogs WHERE id = $1")
        .bind(catalog)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(wh_left, 0, "warehouse must cascade with its workspace");
    assert_eq!(cat_left, 0, "catalog must cascade with its warehouse");
}

/// A direct `DELETE` of a catalog still referenced by a `catalog_workspaces`
/// assignment (in a workspace OTHER than any being deleted in the same
/// statement) is rejected — `NO ACTION` still blocks an out-of-band delete,
/// it just isn't checked until the statement's cascades have all run.
#[tokio::test]
async fn direct_catalog_delete_is_blocked_while_still_assigned() {
    let db = harness_or_skip!();
    let ws = seed_workspace(&db, "Assignee workspace").await;
    let catalog: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg_blocked_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed catalog");
    sqlx::query(
        "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2)",
    )
    .bind(catalog)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("assign catalog to a workspace");

    let err = sqlx::query("DELETE FROM registry.catalogs WHERE id = $1")
        .bind(catalog)
        .execute(&db.pool)
        .await
        .expect_err("a catalog still assigned to a workspace must not delete");
    assert!(
        matches!(&err, sqlx::Error::Database(d) if d.code().as_deref() == Some("23503")),
        "expected a foreign_key_violation (23503): {err:?}"
    );
}

// -- catalog_secret_refs cascades with the catalog ---------------------------

/// `catalog_secret_refs` rows go with their catalog (`ON DELETE CASCADE`), so
/// a deleted catalog cannot leave a secret permanently "in use".
#[tokio::test]
async fn catalog_secret_refs_cascade_with_the_catalog() {
    let db = harness_or_skip!();
    let catalog: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg_secref_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed catalog");
    let secret_id = Uuid::new_v4();
    sqlx::query("INSERT INTO registry.catalog_secret_refs (catalog_id, secret_id) VALUES ($1, $2)")
        .bind(catalog)
        .bind(secret_id)
        .execute(&db.pool)
        .await
        .expect("seed secret ref");

    sqlx::query("DELETE FROM registry.catalogs WHERE id = $1")
        .bind(catalog)
        .execute(&db.pool)
        .await
        .expect("catalog with no other referrers must delete");

    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.catalog_secret_refs WHERE catalog_id = $1",
    )
    .bind(catalog)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(left, 0);
}

// -- migration 0184: external engines and declared attachments ---------------

const EXTERNAL_INSERT: &str = "INSERT INTO engine_mgmt.engines \
       (name, type, namespace, spec, status, origin, external_host, external_port, \
        ranger_plugin_version, root_secret_id) \
     VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8, $9)";

/// One INSERT into `engine_mgmt.engines`, every column the shape CHECK reads
/// spelled out by the caller.
#[allow(clippy::too_many_arguments)]
async fn insert_engine_row(
    db: &Db,
    name: &str,
    ty: &str,
    namespace: &str,
    spec: serde_json::Value,
    origin: &str,
    host: Option<&str>,
    port: Option<i32>,
    version: Option<&str>,
    root_secret: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(EXTERNAL_INSERT)
        .bind(name)
        .bind(ty)
        .bind(namespace)
        .bind(spec)
        .bind(origin)
        .bind(host)
        .bind(port)
        .bind(version)
        .bind(root_secret)
        .execute(&db.pool)
        .await
        .map(|_| ())
}

/// Every outcome is collected first and asserted after the rows are removed, so
/// a failed assertion cannot leave this test's engines behind.
#[tokio::test]
async fn the_external_shape_check_admits_one_shape_and_names_itself_refusing_the_rest() {
    let db = harness_or_skip!();
    let tag = Uuid::new_v4().simple().to_string();
    let n = |s: &str| format!("ext-shape-{s}-{tag}");
    let empty = serde_json::json!({});

    let missing_version = insert_engine_row(
        &db,
        &n("nover"),
        "starrocks",
        "",
        empty.clone(),
        "external",
        Some("sr.example.com"),
        Some(9030),
        None,
        None,
    )
    .await;
    let bad_version = insert_engine_row(
        &db,
        &n("badver"),
        "starrocks",
        "",
        empty.clone(),
        "external",
        Some("sr.example.com"),
        Some(9030),
        Some("2.8"),
        None,
    )
    .await;
    let with_root_secret = insert_engine_row(
        &db,
        &n("secret"),
        "starrocks",
        "",
        empty.clone(),
        "external",
        Some("sr.example.com"),
        Some(9030),
        Some("2.8.0"),
        Some(Uuid::new_v4()),
    )
    .await;
    let with_namespace = insert_engine_row(
        &db,
        &n("ns"),
        "starrocks",
        "egret-engines",
        empty.clone(),
        "external",
        Some("sr.example.com"),
        Some(9030),
        Some("2.8.0"),
        None,
    )
    .await;
    let with_spec = insert_engine_row(
        &db,
        &n("spec"),
        "starrocks",
        "",
        serde_json::json!({"a": 1}),
        "external",
        Some("sr.example.com"),
        Some(9030),
        Some("2.8.0"),
        None,
    )
    .await;
    let port_zero = insert_engine_row(
        &db,
        &n("port0"),
        "starrocks",
        "",
        empty.clone(),
        "external",
        Some("sr.example.com"),
        Some(0),
        Some("2.8.0"),
        None,
    )
    .await;
    let empty_host = insert_engine_row(
        &db,
        &n("nohost"),
        "starrocks",
        "",
        empty.clone(),
        "external",
        Some(""),
        Some(9030),
        Some("2.8.0"),
        None,
    )
    .await;
    let risingwave = insert_engine_row(
        &db,
        &n("rw"),
        "risingwave",
        "",
        empty.clone(),
        "external",
        Some("rw.example.com"),
        Some(4566),
        Some("2.8.0"),
        None,
    )
    .await;
    let provisioned_with_host = insert_engine_row(
        &db,
        &n("provhost"),
        "trino",
        "egret-engines",
        empty.clone(),
        "provisioned",
        Some("trino.example.com"),
        None,
        None,
        None,
    )
    .await;
    let provisioned_with_version = insert_engine_row(
        &db,
        &n("provver"),
        "trino",
        "egret-engines",
        empty.clone(),
        "provisioned",
        None,
        None,
        Some("2.5.0"),
        None,
    )
    .await;
    let unknown_origin = insert_engine_row(
        &db,
        &n("origin"),
        "trino",
        "egret-engines",
        empty.clone(),
        "managed",
        None,
        None,
        None,
        None,
    )
    .await;
    let well_formed = insert_engine_row(
        &db,
        &n("ok"),
        "starrocks",
        "",
        empty.clone(),
        "external",
        Some("sr.example.com"),
        Some(9030),
        Some("2.8.0"),
        None,
    )
    .await;
    let provisioned_ok = insert_engine_row(
        &db,
        &n("provok"),
        "trino",
        "egret-engines",
        empty,
        "provisioned",
        None,
        None,
        None,
        None,
    )
    .await;

    sqlx::query("DELETE FROM engine_mgmt.engines WHERE name LIKE $1")
        .bind(format!("%-{tag}"))
        .execute(&db.pool)
        .await
        .expect("cleanup");

    for (what, outcome) in [
        ("external without a plugin version", &missing_version),
        ("external with a malformed plugin version", &bad_version),
        ("external carrying a root secret", &with_root_secret),
        ("external with a namespace", &with_namespace),
        ("external with a spec", &with_spec),
        ("external with port 0", &port_zero),
        ("external with an empty host", &empty_host),
        ("external risingwave", &risingwave),
        (
            "provisioned carrying an external host",
            &provisioned_with_host,
        ),
        (
            "provisioned carrying a plugin version",
            &provisioned_with_version,
        ),
    ] {
        let err = outcome.as_ref().expect_err(what);
        assert!(
            is_check_violation(err, "engines_external_shape"),
            "{what}: {err:?}"
        );
    }
    let err = unknown_origin.expect_err("origin outside the closed set");
    assert!(is_check_violation(&err, "engines_origin_check"), "{err:?}");
    well_formed.expect("a well-formed external row is accepted");
    provisioned_ok.expect("a provisioned row with none of the external fields is accepted");
}

#[tokio::test]
async fn a_declared_attachment_holds_nothing_and_the_check_names_itself() {
    let db = harness_or_skip!();
    let tag = Uuid::new_v4().simple().to_string();
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines \
           (name, type, namespace, spec, status, origin, external_host, external_port, \
            ranger_plugin_version) \
         VALUES ($1, 'trino', '', '{}'::jsonb, 'running', 'external', 'trino.example.com', \
                 8080, '2.5.0') RETURNING id",
    )
    .bind(format!("ext-decl-{tag}"))
    .fetch_one(&db.pool)
    .await
    .expect("external engine");
    let mut catalogs = Vec::new();
    for i in 0..4 {
        catalogs.push(
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO registry.catalogs (name, kind, config) \
                 VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
            )
            .bind(format!("ext_decl_{i}_{tag}"))
            .fetch_one(&db.pool)
            .await
            .expect("catalog"),
        );
    }
    let insert = |catalog: Uuid, minted: bool, primary: bool, status: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO registry.engine_catalog_attachments \
                   (engine_id, catalog_id, minted_client, is_primary, apply_status) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(engine_id)
            .bind(catalog)
            .bind(minted)
            .bind(primary)
            .bind(status)
            .execute(&pool)
            .await
        }
    };
    let minted = insert(catalogs[0], true, false, "declared").await;
    let primary = insert(catalogs[1], false, true, "declared").await;
    let ok = insert(catalogs[2], false, false, "declared").await;
    let unknown_status = insert(catalogs[3], false, false, "guessed").await;

    sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine_id)
        .execute(&db.pool)
        .await
        .expect("cleanup engine (cascades attachments)");
    sqlx::query("DELETE FROM registry.catalogs WHERE id = ANY($1)")
        .bind(&catalogs)
        .execute(&db.pool)
        .await
        .expect("cleanup catalogs");

    let err = minted.expect_err("a declared attachment that minted a client");
    assert!(
        is_check_violation(&err, "attachments_declared_holds_nothing"),
        "{err:?}"
    );
    let err = primary.expect_err("a declared attachment that is primary");
    assert!(
        is_check_violation(&err, "attachments_declared_holds_nothing"),
        "{err:?}"
    );
    let err = unknown_status.expect_err("a status outside the closed set");
    assert!(
        is_check_violation(&err, "engine_catalog_attachments_apply_status_check"),
        "{err:?}"
    );
    ok.expect("a well-formed declared attachment is accepted");
}

// -- migration 0185: every provisioned engine's image becomes explicit --------

/// Migration 0185's UPDATEs, comments dropped, each `WHERE` narrowed to the ids
/// this test inserted so a concurrent test's engines are never touched. The
/// count of narrowed statements is asserted: a rewrite that matched nothing
/// would otherwise run nothing and pass.
fn migration_0185_scoped_statements() -> Vec<String> {
    let sql: String = include_str!("../../../migrations/0185_engine_images_explicit.sql")
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    let statements: Vec<String> = sql
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replacen("WHERE ", "WHERE id = ANY($1) AND ", 1))
        .collect();
    assert_eq!(statements.len(), 6, "six UPDATEs: {statements:?}");
    for s in &statements {
        assert!(s.contains("id = ANY($1)"), "scoped: {s}");
    }
    statements
}

async fn insert_engine_spec(
    db: &Db,
    name: &str,
    ty: &str,
    spec: serde_json::Value,
    origin: &str,
) -> Uuid {
    let external = origin == "external";
    let sql = "INSERT INTO engine_mgmt.engines \
         (name, type, namespace, spec, status, origin, external_host, external_port, \
          ranger_plugin_version) \
       VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8) RETURNING id";
    sqlx::query_scalar(sql)
        .bind(name)
        .bind(ty)
        .bind(if external { "" } else { "egret-engines" })
        .bind(spec)
        .bind(origin)
        .bind(external.then_some("sr.example.com"))
        .bind(external.then_some(9030_i32))
        .bind(external.then_some("2.8.0"))
        .fetch_one(&db.pool)
        .await
        .expect("insert engine")
}

#[tokio::test]
async fn migration_0185_writes_todays_implicit_images_and_touches_nothing_explicit() {
    let db = harness_or_skip!();
    let tag = Uuid::new_v4().simple().to_string();
    let trino_empty = insert_engine_spec(
        &db,
        &format!("img-trino-empty-{tag}"),
        "trino",
        serde_json::json!({}),
        "provisioned",
    )
    .await;
    let trino_explicit = insert_engine_spec(
        &db,
        &format!("img-trino-explicit-{tag}"),
        "trino",
        serde_json::json!({"imageVersion": "483"}),
        "provisioned",
    )
    .await;
    let sr_implicit = insert_engine_spec(
        &db,
        &format!("img-sr-implicit-{tag}"),
        "starrocks",
        serde_json::json!({"starrocksConfig": {"feReplicas": 1}}),
        "provisioned",
    )
    .await;
    let sr_external = insert_engine_spec(
        &db,
        &format!("img-sr-external-{tag}"),
        "starrocks",
        serde_json::json!({}),
        "external",
    )
    .await;
    let trino_number = insert_engine_spec(
        &db,
        &format!("img-trino-number-{tag}"),
        "trino",
        serde_json::json!({"imageVersion": 470}),
        "provisioned",
    )
    .await;
    let sr_number = insert_engine_spec(
        &db,
        &format!("img-sr-number-{tag}"),
        "starrocks",
        serde_json::json!({"starrocksConfig": {"feImage": 41, "cnImage": 42}}),
        "provisioned",
    )
    .await;
    let ids = vec![
        trino_empty,
        trino_explicit,
        sr_implicit,
        sr_external,
        trino_number,
        sr_number,
    ];

    let mut run_error = None;
    for statement in migration_0185_scoped_statements() {
        // `AssertSqlSafe`: the text is the checked-in migration file, narrowed by a
        // fixed literal replacement; nothing user-supplied is interpolated.
        if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(statement))
            .bind(&ids)
            .execute(&db.pool)
            .await
        {
            run_error = Some(e.to_string());
            break;
        }
    }
    let mut specs = Vec::new();
    for id in &ids {
        let spec: serde_json::Value =
            sqlx::query_scalar("SELECT spec FROM engine_mgmt.engines WHERE id = $1")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .expect("read spec");
        specs.push(spec);
    }
    sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&db.pool)
        .await
        .expect("cleanup");

    assert_eq!(run_error, None);
    assert_eq!(specs[0]["imageVersion"], "470", "empty Trino spec");
    assert_eq!(specs[1], serde_json::json!({"imageVersion": "483"}));
    let sr = &specs[2]["starrocksConfig"];
    assert_eq!(sr["feImage"], "starrocks/fe-ubuntu:4.1-latest");
    assert_eq!(sr["cnImage"], "starrocks/cn-ubuntu:4.1-latest");
    assert_eq!(sr["feReplicas"], 1, "other config kept");
    assert_eq!(specs[3], serde_json::json!({}), "external untouched");
    // A non-string image becomes the string it spells, so Rust's `as_str()` sees it.
    assert_eq!(specs[4]["imageVersion"], serde_json::json!("470"));
    assert_eq!(
        specs[5]["starrocksConfig"]["feImage"],
        serde_json::json!("41")
    );
    assert_eq!(
        specs[5]["starrocksConfig"]["cnImage"],
        serde_json::json!("42")
    );
}
