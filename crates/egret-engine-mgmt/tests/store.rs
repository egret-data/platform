//! `EngineMgmtStore` (`engine_mgmt.engines`) integration tests. Moved verbatim
//! from `egret-catalog/tests/embedded.rs`
//! (E1b) alongside the store methods themselves. `engines` has no
//! `warehouse_id` column any more (SP3c-2 Task 6); `seed_warehouse` still
//! crosses the seam through `egret_catalog_engine::CatalogStore` (a dev-only
//! dependency here) wherever a test needs a real `catalog.warehouses` row —
//! e.g. to recover its `workspace_id`. DB-gated; self-skips without
//! `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
use egret_catalog_engine::CatalogStore;
use egret_engine_mgmt::store::Engine;
use egret_engine_mgmt::EngineMgmtStore;
use egret_metastore::Db;
use uuid::Uuid;

/// Connect to the database named by `DATABASE_URL` and return a ready
/// `(CatalogStore, EngineMgmtStore)` pair. Returns `None` (self-skip) when the
/// env var is unset.
async fn stores() -> Option<(CatalogStore, EngineMgmtStore)> {
    let url = egret_test_db::url().await?;
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
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (_wh_id, _wh_name) = seed_warehouse(&catalog, "eng").await;

    let eng_name = format!("eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let created = engine_mgmt
        .create_engine(&eng_name, "trino", "egret-engines", &spec, None)
        .await
        .unwrap();
    assert_eq!(created.name, eng_name);
    assert_eq!(created.r#type, "trino");
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
        .create_engine(&eng_name, "trino", "egret-engines", &spec, None)
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
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
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
        spec: serde_json::json!({ "trinoConfig": { "auth": { "jwt": { "jwksUri": "http://kc/jwks" } } } }),
        status: "running".into(),
        authz_service_id: None,
        delegator_secret_id: None,
        root_secret_id: None,
        tls_secret_id: None,
        coordinator_cert: None,
        tls_cert_secret_ref: None,
        internal_secret_id: None,
        monitor_secret_id: None,
        ranger_secret_id: None,
        ranger_secret_prev_id: None,
        created_at: chrono::Utc::now(),
        origin: "provisioned".into(),
        external_host: None,
        external_port: None,
        ranger_plugin_version: None,
    };
    assert_eq!(e.coordinator_host(), "trino-prod-trino.engines.svc");
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

#[tokio::test]
async fn engine_status_is_constrained_and_transitions_only_from_the_expected_value() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (_wh_id, _) = seed_warehouse(&catalog, "status").await;
    let e = engine_mgmt
        .create_engine(
            &format!("status-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(e.status, egret_engine_mgmt::engine_status::PROVISIONING);

    let err = sqlx::query("UPDATE engine_mgmt.engines SET status = 'ready' WHERE id = $1")
        .bind(e.id)
        .execute(catalog.pool())
        .await
        .expect_err("'ready' is not a lifecycle status");
    assert_eq!(
        err.as_database_error().and_then(|d| d.constraint()),
        Some("engines_status_check")
    );

    assert!(engine_mgmt
        .transition_engine_status(e.id, "provisioning", "running")
        .await
        .unwrap());
    assert!(
        !engine_mgmt
            .transition_engine_status(e.id, "provisioning", "running")
            .await
            .unwrap(),
        "the row is no longer provisioning"
    );
    engine_mgmt
        .set_engine_status(e.id, "deleting")
        .await
        .unwrap();
    assert!(!engine_mgmt
        .transition_engine_status(e.id, "running", "running")
        .await
        .unwrap());
    assert_eq!(
        engine_mgmt.get_engine(e.id).await.unwrap().unwrap().status,
        "deleting"
    );
}

#[tokio::test]
async fn internal_secret_ids_round_trip_on_the_engine_row() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (_wh_id, _) = seed_warehouse(&catalog, "isec").await;
    let e = engine_mgmt
        .create_engine(
            &format!("isec-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    assert!(e.internal_secret_id.is_none() && e.monitor_secret_id.is_none());

    let (internal, monitor) = (Uuid::new_v4(), Uuid::new_v4());
    let committed = engine_mgmt
        .set_internal_secret_ids(e.id, None, None, internal, monitor)
        .await
        .unwrap();
    assert!(
        committed,
        "the row was still NULL, matching the expected previous pair"
    );
    let got = engine_mgmt.get_engine(e.id).await.unwrap().unwrap();
    assert_eq!(got.internal_secret_id, Some(internal));
    assert_eq!(got.monitor_secret_id, Some(monitor));
    let listed = engine_mgmt.list_engines().await.unwrap();
    let row = listed.iter().find(|x| x.id == e.id).expect("listed");
    assert_eq!(row.monitor_secret_id, Some(monitor));

    // A stale caller — still expecting the row to be NULL — loses the race and
    // writes nothing.
    let (internal2, monitor2) = (Uuid::new_v4(), Uuid::new_v4());
    let committed2 = engine_mgmt
        .set_internal_secret_ids(e.id, None, None, internal2, monitor2)
        .await
        .unwrap();
    assert!(
        !committed2,
        "the row no longer matches the stale expected pair"
    );
    let got = engine_mgmt.get_engine(e.id).await.unwrap().unwrap();
    assert_eq!(
        got.internal_secret_id,
        Some(internal),
        "the winner's pair must survive"
    );
    assert_eq!(got.monitor_secret_id, Some(monitor));

    // The caller that reads the CURRENT pair as its expectation succeeds.
    let (internal3, monitor3) = (Uuid::new_v4(), Uuid::new_v4());
    let committed3 = engine_mgmt
        .set_internal_secret_ids(e.id, Some(internal), Some(monitor), internal3, monitor3)
        .await
        .unwrap();
    assert!(committed3);
    let got = engine_mgmt.get_engine(e.id).await.unwrap().unwrap();
    assert_eq!(got.internal_secret_id, Some(internal3));
    assert_eq!(got.monitor_secret_id, Some(monitor3));
}
