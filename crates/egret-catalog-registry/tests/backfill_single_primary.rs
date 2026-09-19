//! Backfill: one engine primary (the kind-cluster shape), name normalisation and
//! suffixing, typed vs generic, shared vs suffixed same-name rows, secret
//! extraction end-to-end (T2), the primary-processed-first name guard, and
//! idempotency down to attachment/secret/legacy-map counts. DB-gated; FAILS
//! without DATABASE_URL.

use egret_catalog_registry::{run_backfill, CatalogConfig, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

struct Seed {
    db: Db,
    ws: Uuid,
    sc: Uuid,
}

impl Seed {
    async fn warehouse(&self, name: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id")
            .bind(name).bind(self.ws).bind(self.sc).fetch_one(&self.db.pool).await.unwrap()
    }
    async fn engine(&self, name: &str, wh: Uuid) -> Uuid {
        sqlx::query_scalar("INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id")
            .bind(name).bind(wh).fetch_one(&self.db.pool).await.unwrap()
    }
    async fn legacy(
        &self,
        engine: Uuid,
        name: &str,
        kind: &str,
        config: serde_json::Value,
        primary: bool,
    ) -> Uuid {
        sqlx::query_scalar("INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) VALUES ($1, $2, $3, $4, $5) RETURNING id")
            .bind(engine).bind(name).bind(kind).bind(config).bind(primary).fetch_one(&self.db.pool).await.unwrap()
    }
}

async fn catalog_names(db: &Db) -> BTreeSet<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM registry.catalogs")
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

async fn codes(db: &Db, run: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT code FROM registry.migration_report WHERE run_id = $1 ORDER BY id")
        .bind(run)
        .fetch_all(&db.pool)
        .await
        .unwrap()
}

/// `(engine_id, catalog_id)` for every attachment, for a before/after idempotency
/// comparison that is not fooled by a count staying the same while the SET changes.
async fn attachment_pairs(db: &Db) -> BTreeSet<(Uuid, Uuid)> {
    sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT engine_id, catalog_id FROM registry.engine_catalog_attachments",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

async fn secret_count(db: &Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workspace_secrets")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn legacy_map_count(db: &Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM registry.legacy_engine_catalogs")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn single_primary_platform_migrates_like_the_kind_cluster() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let ws: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug, name, status) VALUES ('bf-one', 'BF', 'active') RETURNING id").fetch_one(&db.pool).await.unwrap();
    let sc: Uuid = sqlx::query_scalar("INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) VALUES ('bf-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id").fetch_one(&db.pool).await.unwrap();
    let s = Seed {
        db: db.clone(),
        ws,
        sc,
    };
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = RegistryStore::new(db.clone());

    let primary_wh = s.warehouse("e2e-wh-1784523876270").await;
    s.warehouse("e2e-wh-1784519491585").await;
    s.warehouse("Sales Data").await;
    s.warehouse("sales_data").await;
    s.warehouse("1st").await;
    // A warehouse literally NAMED "egret" but NOT the primary: proves the
    // primary-processed-first fix (I2 minor) — it must never steal the name.
    let impostor_wh = s.warehouse("egret").await;
    let demo = s.engine("demo-trino", primary_wh).await;
    let other = s.engine("trino-b", primary_wh).await;
    let client = Uuid::new_v4();
    s.legacy(
        demo,
        "egret",
        "egret-warehouse",
        json!({
            "warehouse": "e2e-wh-1784523876270", "s3_endpoint": "http://minio:9000",
            "credential": "cid:csecret", "clientRowId": client
        }),
        true,
    )
    .await;
    s.legacy(demo, "bench", "tpch", json!({}), false).await;
    s.legacy(other, "bench", "tpch", json!({}), false).await;
    s.legacy(demo, "pg_main", "postgresql", json!({ "properties": {
        "connection-url": "jdbc:postgresql://db:5432/x", "connection-user": "u", "connection-password": "hunter2" } }), false).await;
    s.legacy(
        other,
        "pg_main",
        "postgresql",
        json!({ "properties": {
        "connection-url": "jdbc:postgresql://other:5432/y", "connection-user": "u" } }),
        false,
    )
    .await;
    let pw = vault
        .create(None, "bf-pg-typed", None, "s3cr3t")
        .await
        .unwrap();
    s.legacy(demo, "Bad-Name", "postgresql", json!({
        "properties": { "connection-url": "jdbc:postgresql://db:5432/z?sslmode=require", "connection-user": "u" },
        "secretProperties": { "connection-password": pw.to_string() } }), false).await;
    // T2: an all-digit password under a sensitive key is still a secret, never a
    // plaintext value left in config/advanced, and its own secret round-trips.
    s.legacy(demo, "pg_num", "postgresql", json!({ "properties": {
        "connection-url": "jdbc:postgresql://numdb:5432/n", "connection-user": "u", "connection-password": "123456" } }), false).await;

    let first = run_backfill(&store, &vault).await.expect("backfill");
    assert_eq!(first.failed_rows, 0);

    // Warehouses: the single primary is `egret`; the impostor is suffixed; the
    // rest normalise, collisions suffix.
    assert_eq!(
        catalog_names(&db).await,
        [
            "egret",
            "egret_2",
            "e2e_wh_1784519491585",
            "sales_data",
            "sales_data_2",
            "w_1st",
            "bench",
            "pg_main",
            "pg_main_trino_b",
            "bad_name",
            "pg_num",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    );
    let egret = store.get_catalog_by_name("egret").await.unwrap().unwrap();
    assert_eq!(egret.warehouse_id, Some(primary_wh));
    assert_eq!(
        store.list_catalog_workspaces(egret.id).await.unwrap()[0].workspace_id,
        ws
    );
    let impostor = store.get_catalog_by_name("egret_2").await.unwrap().unwrap();
    assert_eq!(
        impostor.warehouse_id,
        Some(impostor_wh),
        "the literally-named warehouse never steals the primary's name"
    );

    // The primary attachment carries its client; the credential is a secret now.
    let a = store.get_attachment(demo, egret.id).await.unwrap().unwrap();
    assert_eq!(a.catalog_client_id, Some(client));
    assert!(a.minted_client);
    assert_eq!(
        vault
            .resolve_plaintext(None, a.credential_secret_id.unwrap())
            .await
            .unwrap(),
        "cid:csecret"
    );
    // trino-b's primary row was never there (it has no legacy primary), so it has no egret attachment.
    assert!(store
        .get_attachment(other, egret.id)
        .await
        .unwrap()
        .is_none());

    // Identical `bench` rows share one typed catalog; different `pg_main` rows do not.
    let bench = store.get_catalog_by_name("bench").await.unwrap().unwrap();
    assert!(matches!(
        bench.def().unwrap().config,
        CatalogConfig::Tpch(_)
    ));
    assert!(store
        .get_attachment(demo, bench.id)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_attachment(other, bench.id)
        .await
        .unwrap()
        .is_some());
    let pg = store.get_catalog_by_name("pg_main").await.unwrap().unwrap();
    assert_eq!(pg.kind, "generic");
    assert!(
        !pg.advanced.to_string().contains("hunter2"),
        "plaintext left the catalog's advanced properties"
    );
    assert!(
        !pg.config.to_string().contains("hunter2"),
        "plaintext left the catalog's config"
    );
    // T2: the secret is not just absent from the catalog — it resolves to the
    // exact original plaintext, so the credential was moved, never lost.
    let CatalogConfig::Generic(pg_cfg) = &pg.def().unwrap().config else {
        panic!("{:?}", pg.def())
    };
    let pg_pw_id = pg_cfg.secret_properties.trino["connection-password"];
    assert_eq!(
        vault.resolve_plaintext(None, pg_pw_id).await.unwrap(),
        "hunter2"
    );

    let typed = store
        .get_catalog_by_name("bad_name")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(typed.kind, "jdbc");

    // T2: an all-digit password is still a secret — never in config/advanced,
    // and it resolves to exactly the original digits.
    let pg_num = store.get_catalog_by_name("pg_num").await.unwrap().unwrap();
    assert_eq!(pg_num.kind, "generic");
    assert!(!pg_num.advanced.to_string().contains("123456"));
    assert!(!pg_num.config.to_string().contains("123456"));
    let CatalogConfig::Generic(pg_num_cfg) = &pg_num.def().unwrap().config else {
        panic!("{:?}", pg_num.def())
    };
    let pg_num_pw_id = pg_num_cfg.secret_properties.trino["connection-password"];
    assert_eq!(
        vault.resolve_plaintext(None, pg_num_pw_id).await.unwrap(),
        "123456"
    );

    let got = codes(&db, first.run_id).await;
    for code in [
        "warehouse_catalog_created",
        "primary_named_egret",
        "engine_catalog_typed",
        "engine_catalog_generic",
        "engine_catalog_shared",
        "engine_catalog_suffixed",
        "engine_catalog_renamed",
        "secret_extracted",
    ] {
        assert!(got.iter().any(|c| c == code), "missing {code} in {got:?}");
    }
    assert!(
        !got.iter().any(|c| c == "saved_sql_references_catalog"),
        "nothing references bad-name"
    );

    // Every legacy row is mapped; nothing blocks either engine.
    assert_eq!(
        store.unmigrated_legacy_catalog_count(demo).await.unwrap(),
        0
    );
    assert_eq!(
        store.unmigrated_legacy_catalog_count(other).await.unwrap(),
        0
    );

    // Idempotent down to the exact attachment set, secret count and legacy-map
    // count — not just "still 11 catalogs" — and an admin's removal is not undone.
    let attachments_before = attachment_pairs(&db).await;
    let secrets_before = secret_count(&db).await;
    let legacy_map_before = legacy_map_count(&db).await;
    store.set_catalog_workspaces(egret.id, &[]).await.unwrap();
    store.delete_attachment(other, bench.id).await.unwrap();

    let second = run_backfill(&store, &vault).await.expect("second backfill");
    assert_eq!((second.report_rows, second.failed_rows), (0, 0));
    assert!(store
        .list_catalog_workspaces(egret.id)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .get_attachment(other, bench.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(catalog_names(&db).await.len(), 11);

    let mut expected_attachments = attachments_before.clone();
    expected_attachments.remove(&(other, bench.id));
    assert_eq!(
        attachment_pairs(&db).await,
        expected_attachments,
        "the second run neither re-adds the removed attachment nor touches any other"
    );
    assert_eq!(
        secret_count(&db).await,
        secrets_before,
        "no secret is re-created or duplicated on a no-op boot"
    );
    assert_eq!(
        legacy_map_count(&db).await,
        legacy_map_before,
        "no legacy row is re-mapped on a no-op boot"
    );
}
