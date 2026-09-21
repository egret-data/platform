//! M5 (review fix round 2): if step 1 fails for the PRIMARY warehouse and step 2
//! then creates that warehouse's catalog on its own, step 2 must make the SAME
//! single-primary `egret` naming decision step 1 would have — and write the
//! SAME `warehouse_catalog_created`/`primary_named_egret` report step 1 would
//! have written, since step 1's own `backfill_failed` entry explains nothing
//! about how the warehouse actually got migrated a moment later in the same run.
//!
//! Forcing step 1 (and only step 1) to fail, deterministically, needs a trigger
//! that fires exactly once and then stays fired even though the transaction
//! carrying its first firing rolls back. A `SEQUENCE`'s `nextval()` is exactly
//! that: Postgres explicitly does NOT roll it back with its transaction, unlike
//! an ordinary table row. A `BEFORE INSERT` trigger on
//! `registry.catalog_workspaces` (the second of `ensure_warehouse_catalog`'s two
//! statements, so the also-inserted `registry.catalogs` row rolls back with it)
//! raises on `nextval() = 1` and passes on every call after — step 1's one
//! attempt (there is exactly one warehouse, so exactly one `catalog_workspaces`
//! insert attempt from step 1) hits the trap and fails; step 2's later, fresh
//! retry (nothing persisted from step 1's rolled-back attempt) does not.
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn step2_recovering_a_step1_failure_still_names_the_primary_egret() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = db.pool.clone();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ('bf-m5', 'BF', 'active') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ('bf-m5-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // Exactly one warehouse, so step 1's loop makes exactly one attempt at
    // `registry.catalog_workspaces` — the one the trigger below traps.
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-m5-wh', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('bf-m5-trino', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(wh)
    .fetch_one(&pool)
    .await
    .unwrap();
    let client = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, true)",
    )
    .bind(engine)
    .bind(json!({ "warehouse": "bf-m5-wh", "credential": "cid:csecret", "clientRowId": client }))
    .execute(&pool)
    .await
    .unwrap();

    // The fail-once trap: `nextval()` survives the rollback of the INSERT that
    // triggered it, so the SECOND `catalog_workspaces` insert attempt (step 2's,
    // moments later in the same `run_backfill` call) sails through.
    sqlx::query("CREATE SEQUENCE bf_m5_fail_once_seq START 1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION bf_m5_fail_once() RETURNS trigger AS $$ \
         BEGIN \
           IF nextval('bf_m5_fail_once_seq') = 1 THEN \
             RAISE EXCEPTION 'M5 test: step 1''s only attempt fails on purpose'; \
           END IF; \
           RETURN NEW; \
         END; $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER bf_m5_fail_once_trigger BEFORE INSERT ON registry.catalog_workspaces \
          FOR EACH ROW EXECUTE FUNCTION bf_m5_fail_once()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = RegistryStore::new(db.clone());
    let run = run_backfill(&store, &vault)
        .await
        .expect("step 2 recovers; the whole boot still succeeds");

    // Drop the trap: nothing after this point should ever fire it again anyway
    // (the warehouse is migrated), but leaving test-only DDL behind is untidy.
    sqlx::query("DROP TRIGGER bf_m5_fail_once_trigger ON registry.catalog_workspaces")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION bf_m5_fail_once()")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP SEQUENCE bf_m5_fail_once_seq")
        .execute(&pool)
        .await
        .unwrap();

    // Step 2 recovered it, named exactly `egret` — not a normalised fallback —
    // despite step 1 having failed for it moments earlier in the same run.
    let egret = store
        .get_catalog_by_name("egret")
        .await
        .unwrap()
        .expect("step 2 must still apply the single-primary egret exception");
    assert_eq!(egret.warehouse_id, Some(wh));
    assert_eq!(
        store.list_catalog_workspaces(egret.id).await.unwrap()[0].workspace_id,
        ws
    );
    let attachment = store
        .get_attachment(engine, egret.id)
        .await
        .unwrap()
        .expect("the primary row itself also migrated");
    assert_eq!(
        vault
            .resolve_plaintext(None, attachment.credential_secret_id.unwrap())
            .await
            .unwrap(),
        "cid:csecret"
    );
    assert_eq!(
        store.unmigrated_legacy_catalog_count(engine).await.unwrap(),
        0
    );

    // Report rows are consistent with what actually happened: step 1's failure
    // is on record, AND the creation report that failure entry's own wording
    // implies never happened is ALSO on record (written by step 2's recovery).
    let rows: Vec<(String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT code, warehouse_id, catalog_id FROM registry.migration_report \
          WHERE run_id = $1 ORDER BY id",
    )
    .bind(run.run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let codes: Vec<&str> = rows.iter().map(|(c, _, _)| c.as_str()).collect();
    assert_eq!(
        codes.iter().filter(|c| **c == "backfill_failed").count(),
        1,
        "{codes:?}"
    );
    assert!(
        rows.iter()
            .any(|(c, wh_id, _)| c == "backfill_failed" && *wh_id == Some(wh)),
        "step 1's failure names its own warehouse: {rows:?}"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|c| **c == "warehouse_catalog_created")
            .count(),
        1,
        "step 2's recovery writes the creation report step 1 never got to: {codes:?}"
    );
    assert!(
        rows.iter()
            .any(|(c, wh_id, cat_id)| c == "warehouse_catalog_created"
                && *wh_id == Some(wh)
                && *cat_id == Some(egret.id)),
        "{rows:?}"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|c| **c == "primary_named_egret")
            .count(),
        1,
        "{codes:?}"
    );
    assert!(
        !codes.contains(&"engine_catalog_renamed"),
        "the primary row's own name already matches the final one: {codes:?}"
    );
    assert_eq!(run.failed_rows, 1, "only step 1's one attempt failed");

    // Idempotent: a second boot changes nothing.
    let second = run_backfill(&store, &vault).await.expect("second boot");
    assert_eq!((second.report_rows, second.failed_rows), (0, 0));
}
