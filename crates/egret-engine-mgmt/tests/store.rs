//! `EngineMgmtStore` (`engine_mgmt.engines` / `engine_mgmt.engine_catalogs`)
//! integration tests. Moved verbatim from `egret-catalog/tests/embedded.rs`
//! (E1b) alongside the store methods themselves; only the seeding of the
//! `warehouse_id` FK now crosses the seam through `egret_catalog_engine::CatalogStore`
//! (a dev-only dependency here), exactly like `create_engine`'s auto-wiring in
//! control-plane. DB-gated; self-skips without `DATABASE_URL`.
use egret_catalog_engine::CatalogStore;
use egret_engine_mgmt::store::Engine;
use egret_engine_mgmt::EngineMgmtStore;
use egret_metastore::Db;
use uuid::Uuid;

/// Connect to the database named by `DATABASE_URL` and return a ready
/// `(CatalogStore, EngineMgmtStore)` pair. Returns `None` (self-skip) when the
/// env var is unset.
async fn stores() -> Option<(CatalogStore, EngineMgmtStore)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some((
        CatalogStore::new(db.clone()),
        EngineMgmtStore::new(db.clone()),
    ))
}

/// Seed a fresh workspace + storage-conn + warehouse row via `CatalogStore`
/// (the catalog seam owns them); return the warehouse's `(id, name)`.
async fn seed_warehouse(catalog: &CatalogStore, prefix: &str) -> (Uuid, String) {
    let ws_id = Uuid::new_v4();
    let ws_slug = format!("ws_{}", ws_id.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(ws_id)
        .bind(&ws_slug)
        .bind(&ws_slug)
        .execute(catalog.pool())
        .await
        .expect("seed workspace");

    // The storage-connection registry moved to egret-storage (E3a); seed the row
    // the warehouse FK-references directly, so this crate needn't depend on it.
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("{prefix}-store-{}", Uuid::new_v4().simple()))
    .fetch_one(catalog.pool())
    .await
    .unwrap();

    let wh_name = format!("{prefix}-wh-{}", Uuid::new_v4().simple());
    let wh = catalog
        .create_warehouse(&wh_name, ws_id, sc_id)
        .await
        .unwrap();
    (wh.id, wh_name)
}

#[tokio::test]
async fn create_list_get_delete_engine() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let (wh_id, _wh_name) = seed_warehouse(&catalog, "eng").await;

    let eng_name = format!("eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let created = engine_mgmt
        .create_engine(&eng_name, "trino", "egret-engines", wh_id, &spec)
        .await
        .unwrap();
    assert_eq!(created.name, eng_name);
    assert_eq!(created.r#type, "trino");
    assert_eq!(created.warehouse_id, wh_id);
    assert_eq!(created.status, "provisioning");

    // list includes it
    let list = engine_mgmt.list_engines().await.unwrap();
    assert!(list.iter().any(|e| e.id == created.id));

    // get by id
    let got = engine_mgmt.get_engine(created.id).await.unwrap().unwrap();
    assert_eq!(got.id, created.id);
    assert_eq!(got.spec, spec);

    // delete → gone
    engine_mgmt.delete_engine(created.id).await.unwrap();
    assert!(engine_mgmt.get_engine(created.id).await.unwrap().is_none());
}

#[tokio::test]
async fn create_list_delete_engine_catalogs_cascade() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let (wh_id, wh_name) = seed_warehouse(&catalog, "cat").await;

    let eng_name = format!("cat-eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let engine = engine_mgmt
        .create_engine(&eng_name, "trino", "egret-engines", wh_id, &spec)
        .await
        .unwrap();

    // Insert the extra (non-primary) catalog first, then the primary, to prove the
    // ordering comes from `is_primary DESC` and not insertion order.
    let extra = engine_mgmt
        .create_engine_catalog(
            engine.id,
            "tpch",
            "tpch",
            &serde_json::json!({}),
            None,
            false,
        )
        .await
        .unwrap();
    let primary = engine_mgmt
        .create_engine_catalog(
            engine.id,
            "egret",
            "egret-warehouse",
            &serde_json::json!({ "warehouse": wh_name }),
            None,
            true,
        )
        .await
        .unwrap();

    // list → primary first
    let list = engine_mgmt.list_engine_catalogs(engine.id).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, primary.id);
    assert!(list[0].is_primary);
    assert_eq!(list[1].id, extra.id);

    // get by id
    let got = engine_mgmt
        .get_engine_catalog(extra.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.name, "tpch");
    assert_eq!(got.kind, "tpch");

    // delete the extra → only primary left
    engine_mgmt.delete_engine_catalog(extra.id).await.unwrap();
    let list = engine_mgmt.list_engine_catalogs(engine.id).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, primary.id);

    // deleting the engine cascades to its catalogs
    engine_mgmt.delete_engine(engine.id).await.unwrap();
    assert!(engine_mgmt
        .list_engine_catalogs(engine.id)
        .await
        .unwrap()
        .is_empty());
    assert!(engine_mgmt
        .get_engine_catalog(primary.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn update_engine_catalog_reflects_in_get() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let (wh_id, _wh_name) = seed_warehouse(&catalog, "upd").await;

    let eng_name = format!("upd-eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let engine = engine_mgmt
        .create_engine(&eng_name, "trino", "egret-engines", wh_id, &spec)
        .await
        .unwrap();

    let eng_catalog = engine_mgmt
        .create_engine_catalog(
            engine.id,
            "tpch",
            "tpch",
            &serde_json::json!({}),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(eng_catalog.config, serde_json::json!({}));
    assert_eq!(eng_catalog.secret_ref, None);

    let new_config = serde_json::json!({ "connector.name": "tpch" });
    let new_secret_ref = Uuid::new_v4();
    let updated = engine_mgmt
        .update_engine_catalog(eng_catalog.id, &new_config, Some(new_secret_ref))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.config, new_config);
    assert_eq!(updated.secret_ref, Some(new_secret_ref));

    let got = engine_mgmt
        .get_engine_catalog(eng_catalog.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.config, new_config);
    assert_eq!(got.secret_ref, Some(new_secret_ref));

    // updating a non-existent id returns None rather than erroring
    assert!(engine_mgmt
        .update_engine_catalog(Uuid::new_v4(), &new_config, None)
        .await
        .unwrap()
        .is_none());
}

/// Seed a warehouse (+ its workspace), an engine on it, and a user to be the
/// `assigned_by`. Returns `(engine_id, workspace_id, user_id)`. The warehouse's
/// workspace id is recovered from the `warehouses` row (the catalog seam owns it).
async fn seed_engine_and_ws(
    catalog: &CatalogStore,
    engine_mgmt: &EngineMgmtStore,
    prefix: &str,
) -> (Uuid, Uuid, Uuid) {
    let (wh_id, _wh_name) = seed_warehouse(catalog, prefix).await;
    let ws_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM catalog.warehouses WHERE id = $1")
            .bind(wh_id)
            .fetch_one(catalog.pool())
            .await
            .expect("warehouse workspace_id");

    let eng_name = format!("{prefix}-eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let engine = engine_mgmt
        .create_engine(&eng_name, "trino", "egret-engines", wh_id, &spec)
        .await
        .unwrap();

    let email = format!("{prefix}-{}@egret.local", Uuid::new_v4().simple());
    let user_id: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(&email)
            .fetch_one(catalog.pool())
            .await
            .expect("seed user");

    (engine.id, ws_id, user_id)
}

#[tokio::test]
async fn assign_list_unassign_engine_workspace() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let (engine_id, ws_id, admin) = seed_engine_and_ws(&catalog, &engine_mgmt, "assign").await;

    engine_mgmt
        .assign_workspace(engine_id, ws_id, admin)
        .await
        .unwrap();

    // re-assign is idempotent (ON CONFLICT DO NOTHING)
    engine_mgmt
        .assign_workspace(engine_id, ws_id, admin)
        .await
        .unwrap();

    let ws = engine_mgmt
        .list_workspaces_for_engine(engine_id)
        .await
        .unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].workspace_id, ws_id);

    let engines = engine_mgmt.list_engines_for_workspace(ws_id).await.unwrap();
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0].id, engine_id);

    engine_mgmt
        .unassign_workspace(engine_id, ws_id)
        .await
        .unwrap();
    assert!(engine_mgmt
        .list_workspaces_for_engine(engine_id)
        .await
        .unwrap()
        .is_empty());
    assert!(engine_mgmt
        .list_engines_for_workspace(ws_id)
        .await
        .unwrap()
        .is_empty());
}

#[test]
fn coordinator_host_and_jwt_check() {
    let e = Engine {
        id: Uuid::nil(),
        name: "trino-prod".into(),
        r#type: "trino".into(),
        namespace: "engines".into(),
        warehouse_id: Uuid::nil(),
        spec: serde_json::json!({ "trinoConfig": { "auth": { "jwt": { "jwksUri": "http://kc/jwks" } } } }),
        status: "running".into(),
        authz_service_id: None,
        created_at: chrono::Utc::now(),
    };
    assert_eq!(e.coordinator_host(), "trino-prod-coordinator.engines.svc");
    assert_eq!(e.coordinator_port(), 8080);
    assert!(e.supports_jwt_delegation());

    // auth present but no jwt block → not assignable
    let e2 = Engine {
        spec: serde_json::json!({ "trinoConfig": { "auth": {} } }),
        ..e.clone()
    };
    assert!(!e2.supports_jwt_delegation());

    // no auth block at all → not assignable
    let e3 = Engine {
        spec: serde_json::json!({ "trinoConfig": {} }),
        ..e.clone()
    };
    assert!(!e3.supports_jwt_delegation());
}
