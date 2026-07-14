//! `EngineAssignmentService` integration tests (P1: assign-engine-to-workspace).
//!
//! Orchestrates two seams: the engine-provisioning store (this crate) and the
//! query-registry `DatabaseRepo` (egret-query). Assigning a JWT-capable engine
//! materializes a read-only, per-user-delegated connection in the workspace;
//! unassigning removes it. Non-JWT engines are rejected.
//!
//! DB-gated: self-skips without `DATABASE_URL`, same as `tests/store.rs`. Seeds a
//! warehouse (+ its workspace) via `egret_catalog_engine::CatalogStore` — the
//! catalog seam owns them — recovers the workspace id from the `warehouses` row,
//! and seeds a `users` row for `assigned_by`.
use egret_catalog_engine::CatalogStore;
use egret_engine_mgmt::{EngineAssignmentService, EngineMgmtError, EngineMgmtStore};
use egret_metastore::Db;
use egret_query::DatabaseRepo;
use egret_secrets::{SecretStore, WorkspaceSecretStore};
use uuid::Uuid;

/// Connect and build the full stack shared by both tests. `None` (self-skip)
/// when `DATABASE_URL` is unset. The `DatabaseRepo` binds no crypto key (the
/// materialized connection seals no secret — `secret_id` is NULL).
async fn harness() -> Option<(CatalogStore, EngineMgmtStore, EngineAssignmentService)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let catalog = CatalogStore::new(db.clone());
    let engine_mgmt = EngineMgmtStore::new(db.clone());
    let secrets = SecretStore::new(db.clone(), None);
    let workspace_secrets = WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = DatabaseRepo::new(db.clone(), secrets, workspace_secrets);
    let svc = EngineAssignmentService::new(engine_mgmt.clone(), databases);
    Some((catalog, engine_mgmt, svc))
}

/// Seed a warehouse (+ its workspace), an engine with `spec`, and a user to be
/// the `assigned_by`. Returns `(engine_id, workspace_id, user_id)`.
async fn seed_engine_and_ws(
    catalog: &CatalogStore,
    engine_mgmt: &EngineMgmtStore,
    prefix: &str,
    spec: serde_json::Value,
) -> (Uuid, Uuid, Uuid) {
    let ws_id = Uuid::new_v4();
    let ws_slug = format!("ws_{}", ws_id.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(ws_id)
        .bind(&ws_slug)
        .bind(&ws_slug)
        .execute(catalog.pool())
        .await
        .expect("seed workspace");

    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("{prefix}-store-{}", Uuid::new_v4().simple()))
    .fetch_one(catalog.pool())
    .await
    .expect("seed storage connection");

    let wh_name = format!("{prefix}-wh-{}", Uuid::new_v4().simple());
    let wh = catalog
        .create_warehouse(&wh_name, ws_id, sc_id)
        .await
        .expect("create warehouse");

    let eng_name = format!("{prefix}-eng-{}", Uuid::new_v4().simple());
    let engine = engine_mgmt
        .create_engine(&eng_name, "trino", "egret-engines", wh.id, &spec)
        .await
        .expect("create engine");

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
async fn assign_materializes_a_delegated_connection() {
    let Some((catalog, engine_mgmt, svc)) = harness().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    // JWT-capable engine: `spec.trinoConfig.auth.jwt` is an object.
    let jwt_spec = serde_json::json!({
        "trinoConfig": { "auth": { "jwt": { "jwks_uri": "http://kc/jwks" } } }
    });
    let (engine_id, ws, admin) =
        seed_engine_and_ws(&catalog, &engine_mgmt, "assign", jwt_spec).await;

    svc.assign(engine_id, ws, admin).await.unwrap();

    // The materialized row is delegated and points at the engine's coordinator.
    let engine = engine_mgmt.get_engine(engine_id).await.unwrap().unwrap();
    let dbs = svc_databases(&ws).await.unwrap();
    let mat = dbs
        .iter()
        .find(|d| d.engine_id == Some(engine_id))
        .expect("materialized row");
    assert!(mat.delegated);
    assert_eq!(mat.host, engine.coordinator_host());
    assert_eq!(mat.port, engine.coordinator_port());

    // Idempotent: re-assigning does not create a second row.
    svc.assign(engine_id, ws, admin).await.unwrap();
    let dbs = svc_databases(&ws).await.unwrap();
    assert_eq!(
        dbs.iter()
            .filter(|d| d.engine_id == Some(engine_id))
            .count(),
        1
    );

    // Unassign removes the materialized connection and the assignment.
    svc.unassign(engine_id, ws).await.unwrap();
    let dbs = svc_databases(&ws).await.unwrap();
    assert!(dbs.iter().all(|d| d.engine_id != Some(engine_id)));
    assert!(engine_mgmt
        .list_engines_for_workspace(ws)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn assign_rejects_non_jwt_engine() {
    let Some((catalog, engine_mgmt, svc)) = harness().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    // No `auth.jwt` block → not assignable.
    let non_jwt_spec = serde_json::json!({ "trinoConfig": {} });
    let (engine_id, ws, admin) =
        seed_engine_and_ws(&catalog, &engine_mgmt, "reject", non_jwt_spec).await;

    assert!(matches!(
        svc.assign(engine_id, ws, admin).await,
        Err(EngineMgmtError::NotAssignable(_))
    ));

    // No connection was materialized.
    let dbs = svc_databases(&ws).await.unwrap();
    assert!(dbs.iter().all(|d| d.engine_id != Some(engine_id)));
    // And no assignment recorded.
    assert!(engine_mgmt
        .list_engines_for_workspace(ws)
        .await
        .unwrap()
        .is_empty());
}

/// Fresh `DatabaseRepo` over the same DB, listing a workspace's databases.
/// (A standalone reader keeps the service opaque — it exposes no db handle.)
async fn svc_databases(
    ws: &Uuid,
) -> Result<Vec<egret_query::Database>, egret_query::DatabaseError> {
    let url = std::env::var("DATABASE_URL").unwrap();
    let db = Db::connect(&url, 5).await.expect("connect");
    let secrets = SecretStore::new(db.clone(), None);
    let workspace_secrets = WorkspaceSecretStore::new(db.clone(), secrets.clone());
    DatabaseRepo::new(db, secrets, workspace_secrets)
        .list_for_workspace(*ws)
        .await
}
