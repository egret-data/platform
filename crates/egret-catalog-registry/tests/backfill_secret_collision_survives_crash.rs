//! M2 (review fix round 2): a crash after `create_or_reuse_secret` creates the
//! suffixed secret but before the row's own transaction commits leaves that
//! suffixed secret sitting there, unexplained, with the row still unmapped. The
//! next boot's retry must not silently reuse it (same value, so the REUSE path
//! fires, not the CREATE path) without ALSO writing `secret_name_collision` —
//! otherwise the credential is correctly stored but the report never says why
//! it landed under a name nobody asked for.
//!
//! Simulated directly rather than via a real interrupted boot (the base-named
//! secret already colliding with something else, and the suffixed one already
//! holding this row's own value, IS exactly the state such a crash leaves
//! behind — constructing it directly is equivalent and deterministic).
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn a_quiet_reuse_of_an_already_suffixed_secret_still_reports_the_collision() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = db.pool.clone();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ('bf-m2', 'BF', 'active') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ('bf-m2-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-m2-wh', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('bf-m2-trino', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(wh)
    .fetch_one(&pool)
    .await
    .unwrap();
    let client = Uuid::new_v4();
    let my_credential = "cid:my-real-credential";
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, true)",
    )
    .bind(engine)
    .bind(json!({ "warehouse": "bf-m2-wh", "credential": my_credential, "clientRowId": client }))
    .execute(&pool)
    .await
    .unwrap();

    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let base_name = format!("catalog-client-{client}");
    let suffixed_name = format!("{base_name}-2");

    // The exact state a crash between "boot N creates the suffixed secret" and
    // "boot N's row transaction commits" leaves behind: the base name is taken
    // by something else, and the suffixed name already holds THIS row's own
    // credential (boot N created it, then never got to report or map the row).
    vault
        .create(None, &base_name, None, "cid:someone-elses-value")
        .await
        .unwrap();
    vault
        .create(None, &suffixed_name, None, my_credential)
        .await
        .unwrap();

    let store = RegistryStore::new(db.clone());
    let run = run_backfill(&store, &vault)
        .await
        .expect("the retry succeeds by quietly reusing the already-suffixed secret");
    assert_eq!(run.failed_rows, 0);

    let egret = store
        .get_catalog_by_name("egret")
        .await
        .unwrap()
        .expect("the single primary still gets its egret name");
    let attachment = store
        .get_attachment(engine, egret.id)
        .await
        .unwrap()
        .expect("the row migrated");
    assert_eq!(
        vault
            .resolve_plaintext(None, attachment.credential_secret_id.unwrap())
            .await
            .unwrap(),
        my_credential,
        "the reused secret is the row's own, never the unrelated one under the base name"
    );

    // No THIRD secret was created — this run reused what was already there.
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM workspace_secrets WHERE name LIKE $1 ORDER BY name")
            .bind(format!("catalog-client-{client}%"))
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(names, vec![base_name.clone(), suffixed_name.clone()]);

    // M2: even though this boot only REUSED the secret (never created one), the
    // collision is still reported — this is the whole point of the fix.
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT code, detail FROM registry.migration_report WHERE run_id = $1 ORDER BY id",
    )
    .bind(run.run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let collisions: Vec<&(String, serde_json::Value)> = rows
        .iter()
        .filter(|(c, _)| c == "secret_name_collision")
        .collect();
    assert_eq!(
        collisions.len(),
        1,
        "exactly one collision entry, for this row's one credential: {rows:?}"
    );
    assert_eq!(
        collisions[0].1["usedName"].as_str(),
        Some(suffixed_name.as_str())
    );
    assert_eq!(
        collisions[0].1["wantedName"].as_str(),
        Some(base_name.as_str())
    );

    // Idempotent: a second, no-op boot writes nothing new — the collision entry
    // is not duplicated (it was written exactly once, inside this row's own
    // one-time migration transaction; the row is never revisited).
    let second = run_backfill(&store, &vault).await.expect("second boot");
    assert_eq!((second.report_rows, second.failed_rows), (0, 0));
    let total_collisions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.migration_report WHERE code = 'secret_name_collision'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        total_collisions, 1,
        "the collision entry is never duplicated across boots"
    );
}
