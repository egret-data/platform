//! Concrete [`WarehouseProvisioner`] implementation backed by the embedded
//! lakekeeper engine (CE-2 T5).
//!
//! `egret-workspace` defines the [`WarehouseProvisioner`] trait but must stay
//! free of the lakekeeper engine graph, so the workspace-create handler depends
//! only on the trait. This module — in `egret-catalog-engine`, where the engine
//! `ApiContext`, the egret-side `CatalogStore`, and the storage config already
//! live — supplies the impl. The composition root (`egret-bin`) constructs one
//! and sets it on `WorkspaceState.warehouse_provisioner`.
//!
//! `provision` (CE-5 Task 3) persists any supplied per-workspace [`StorageSpec`]
//! — sealing its `secret_key` through [`egret_secrets::SecretStore`] and the rest
//! into `workspace_storage` — then resolves the *effective* [`StorageConfig`] for
//! the workspace (the stored row, or the global default) and delegates to
//! [`crate::provision_workspace_warehouse`] (idempotent, live MinIO
//! `validate_access`), discarding the returned warehouse UUID — the mapping it
//! persists is the durable record the rest of egret reads.

use async_trait::async_trait;
use egret_catalog::service::CatalogStore;
use egret_catalog::WorkspaceStorageRow;
use egret_core::WorkspaceId;
use egret_secrets::SecretStore;
use egret_storage::StorageConfig;
use egret_workspace::provision::{StorageSpec, WarehouseProvisioner};
use lakekeeper::api::ApiContext;
use uuid::Uuid;

use crate::mount::EngineState;
use crate::provision_workspace_warehouse;

/// Engine-backed warehouse provisioner. Holds everything
/// [`provision_workspace_warehouse`] needs so the workspace-create handler can
/// invoke it through the trait without knowing about lakekeeper.
#[derive(Clone)]
pub struct EngineWarehouseProvisioner {
    engine_state: ApiContext<EngineState>,
    store: CatalogStore,
    storage_cfg: StorageConfig,
    system_user: String,
    secret_store: SecretStore,
}

impl EngineWarehouseProvisioner {
    /// Assemble the provisioner from the live engine state + egret-side store +
    /// runtime storage config + the configured `catalog.system_user` + the
    /// secret store used to seal/unseal per-workspace storage secrets.
    pub fn new(
        engine_state: ApiContext<EngineState>,
        store: CatalogStore,
        storage_cfg: StorageConfig,
        system_user: String,
        secret_store: SecretStore,
    ) -> Self {
        Self {
            engine_state,
            store,
            storage_cfg,
            system_user,
            secret_store,
        }
    }
}

/// Persist a per-workspace [`StorageSpec`]: seal `secret_key` through the secret
/// store and write the rest (with the resulting `secret_ref`) into
/// `workspace_storage`. Idempotent at the row level (upsert). Crate-internal:
/// downstream crates must go through the provisioner, never this seam directly.
// TODO(CE-5): re-provision orphans the prior secret_ref; delete-old-on-replace later
pub(crate) async fn persist_storage(
    store: &CatalogStore,
    secret_store: &SecretStore,
    workspace_id: Uuid,
    spec: &StorageSpec,
) -> Result<(), String> {
    let secret_ref = secret_store
        .put("workspace_s3_secret", &spec.secret_key)
        .await
        .map_err(|e| format!("storing workspace storage secret: {e}"))?;
    let row = WorkspaceStorageRow {
        workspace_id,
        provider: spec.provider.clone(),
        endpoint: spec.endpoint.clone(),
        region: spec.region.clone(),
        bucket: spec.bucket.clone(),
        key_prefix: spec.key_prefix.clone(),
        access_key: spec.access_key.clone(),
        secret_ref: Some(secret_ref),
        path_style: spec.path_style,
    };
    store
        .upsert_workspace_storage(&row)
        .await
        .map_err(|e| format!("persisting workspace storage: {e}"))
}

/// Resolve the effective [`StorageConfig`] for a workspace: the stored
/// `workspace_storage` row (with `secret_key` unsealed from the vault by its
/// `secret_ref`), or `default_cfg` when the workspace has no stored row.
/// Crate-internal (see [`persist_storage`]).
pub(crate) async fn resolve_storage(
    store: &CatalogStore,
    secret_store: &SecretStore,
    default_cfg: &StorageConfig,
    workspace_id: Uuid,
) -> Result<StorageConfig, String> {
    match store
        .workspace_storage_for(workspace_id)
        .await
        .map_err(|e| format!("reading workspace_storage for {workspace_id}: {e}"))?
    {
        Some(row) => {
            let secret_key = match row.secret_ref {
                Some(id) => secret_store
                    .get(id)
                    .await
                    .map_err(|e| format!("reading workspace storage secret: {e}"))?,
                // A stored row without a secret_ref can't yield S3 credentials;
                // fail loudly here rather than letting MinIO reject an empty key.
                None => {
                    return Err(format!(
                        "workspace_storage row for {workspace_id} has no secret_ref \
                         (re-provision required)"
                    ))
                }
            };
            Ok(StorageConfig {
                endpoint: row.endpoint,
                region: row.region,
                bucket: row.bucket,
                access_key: row.access_key,
                secret_key,
                path_style: row.path_style,
            })
        }
        None => Ok(default_cfg.clone()),
    }
}

#[async_trait]
impl WarehouseProvisioner for EngineWarehouseProvisioner {
    async fn provision(
        &self,
        workspace_id: WorkspaceId,
        slug: &str,
        storage: Option<StorageSpec>,
    ) -> Result<(), String> {
        if let Some(spec) = &storage {
            persist_storage(&self.store, &self.secret_store, workspace_id.0, spec).await?;
        }
        let cfg = resolve_storage(
            &self.store,
            &self.secret_store,
            &self.storage_cfg,
            workspace_id.0,
        )
        .await?;
        provision_workspace_warehouse(
            self.engine_state.clone(),
            &self.store,
            workspace_id.0,
            slug,
            &cfg,
            &self.system_user,
        )
        .await
        .map(|_warehouse_id| ())
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    //! DB-backed test for the per-workspace storage persist/resolve seam (CE-5
    //! Task 3). In-crate so it can reach the `pub(crate)` resolver without
    //! exposing it. Exercises ONLY `CatalogStore` (workspace_storage) +
    //! `SecretStore` (secrets) — neither the lakekeeper engine nor MinIO — and
    //! proves the `secret_key` round-trips THROUGH egret-secrets (the row carries
    //! a `secret_ref`, not the plaintext) and that an unconfigured workspace
    //! falls back to the global default. Self-skips when `DATABASE_URL` is unset.
    //!
    //!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
    //!   cargo test -p egret-catalog-engine --lib

    use super::{persist_storage, resolve_storage};
    use egret_auth::users::UserRepo;
    use egret_catalog::service::CatalogStore;
    use egret_metastore::Db;
    use egret_secrets::{Crypto, SecretStore};
    use egret_storage::StorageConfig;
    use egret_workspace::provision::StorageSpec;
    use egret_workspace::workspaces::WorkspaceRepo;
    use uuid::Uuid;

    /// A recognizable global default so the "no stored row" branch can assert
    /// equality.
    fn default_storage_config() -> StorageConfig {
        StorageConfig {
            endpoint: Some("http://default:9000".to_string()),
            region: "default-region".to_string(),
            bucket: "default-bucket".to_string(),
            access_key: "default-ak".to_string(),
            secret_key: "default-sk".to_string(),
            path_style: false,
        }
    }

    #[tokio::test]
    async fn persist_and_resolve_round_trips_secret_through_vault() {
        let Ok(dsn) = std::env::var("DATABASE_URL") else {
            eprintln!("DATABASE_URL not set — skipping provision_storage test");
            return;
        };

        let db = Db::connect(&dsn, 5).await.expect("connect egret db");
        db.migrate().await.expect("egret migrations apply");

        let store = CatalogStore::new(db.clone());
        // A real Crypto (32-byte test key) so `put`/`get` actually seal/open — a
        // `None` crypto would fail `put` with `NotConfigured`, defeating the
        // round-trip assertion.
        let crypto = Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
        let secret_store = SecretStore::new(db.clone(), Some(crypto));

        // Seed an owner + workspace (workspace_storage FKs workspaces → users).
        let suffix = Uuid::now_v7().simple().to_string();
        let owner = UserRepo::new(db.clone())
            .insert_local(&format!("ce5-{suffix}@egret.test"), "CE5 Owner", "x")
            .await
            .expect("seed owner");
        let ws = WorkspaceRepo::new(db.clone())
            .create(&format!("ce5ws{suffix}"), "CE5 Storage Test", owner)
            .await
            .expect("seed workspace row");
        let workspace_id: Uuid = ws.id.0;

        let spec = StorageSpec {
            provider: "s3".to_string(),
            endpoint: Some("http://localhost:9000".to_string()),
            region: "us-east-1".to_string(),
            bucket: "wsbucket".to_string(),
            key_prefix: None,
            access_key: "ak".to_string(),
            secret_key: "sk-123".to_string(),
            path_style: true,
        };

        persist_storage(&store, &secret_store, workspace_id, &spec)
            .await
            .expect("persist storage spec");

        let cfg = resolve_storage(
            &store,
            &secret_store,
            &default_storage_config(),
            workspace_id,
        )
        .await
        .expect("resolve stored storage");

        assert_eq!(cfg.bucket, "wsbucket");
        assert_eq!(cfg.endpoint.as_deref(), Some("http://localhost:9000"));
        assert_eq!(cfg.region, "us-east-1");
        assert_eq!(cfg.access_key, "ak");
        assert!(cfg.path_style);
        // The secret round-tripped THROUGH egret-secrets, not the row.
        assert_eq!(cfg.secret_key, "sk-123");

        // The persisted row stores only a `secret_ref`, never the plaintext; the
        // plaintext is fetchable via the vault by that ref.
        let row = store
            .workspace_storage_for(workspace_id)
            .await
            .expect("read back row")
            .expect("row exists after persist");
        let secret_ref = row.secret_ref.expect("secret_ref is set");
        assert_eq!(
            secret_store.get(secret_ref).await.expect("fetch secret"),
            "sk-123",
            "secret is stored in the vault, fetched by secret_ref"
        );

        // Resolving a workspace with NO stored row falls back to the global default.
        let unstored = Uuid::now_v7();
        let cfg_default =
            resolve_storage(&store, &secret_store, &default_storage_config(), unstored)
                .await
                .expect("resolve unconfigured workspace");
        assert_eq!(
            cfg_default,
            default_storage_config(),
            "unconfigured workspace resolves to the global default config"
        );
    }
}
