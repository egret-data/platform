//! Per-user notebook-runtime provisioning (F1b).
//!
//! On every kernel start the runtime endpoint provisions the caller's private
//! catalog access, idempotently:
//!   1. ensure namespace `notebook_{username}` exists in the workspace's
//!      warehouse (already-exists = success), and
//!   2. mint a fresh **1-grant** catalog token scoped to exactly that namespace
//!      (`catalog:read` + `catalog:write`), revoking any previously minted token
//!      for this (user×workspace) first.
//!
//! Catalog tokens are stored hashed (the raw is unrecoverable after creation), so
//! the kernel always needs a *fresh* raw token. We therefore mint-and-revoke on
//! every call, keyed by a deterministic `label = notebook-runtime:{user_id}`,
//! guaranteeing **at most one active token** per (user×workspace).
//!
//! ## Namespace-create call path
//!
//! The live catalog namespaces are owned by the embedded lakekeeper engine in its
//! isolated `lakekeeper` Postgres schema and are created via
//! `NamespaceService::create_namespace` (see `egret-catalog-engine`). Driving that
//! service directly would pull the entire lakekeeper graph into `egret-notebook`
//! and is unreachable from a bare `#[sqlx::test]` scratch DB (no lakekeeper schema,
//! no provisioned warehouse). To keep the crate boundary clean and the logic
//! testable, namespace creation is injected as a [`NamespaceProvisioner`]: the
//! composition root (`egret-bin`, Task 7) wires an impl backed by lakekeeper's
//! `NamespaceService`, while tests use a trivial fake. The token mint/revoke logic
//! — the security-relevant part — is exercised against real Postgres here.

use egret_catalog::service::tokens::Grant;
use egret_catalog::service::CatalogStore;
use uuid::Uuid;

use crate::error::NotebookApiError;

/// The result of provisioning: the user's private namespace and the freshly
/// minted **raw** 1-grant token (shown once; never stored at rest).
#[derive(Debug)]
pub struct Provisioned {
    pub namespace: String,
    pub token: String,
}

/// Ensures a catalog namespace exists under a warehouse, treating
/// "already exists" as success.
///
/// Production wires an impl backed by lakekeeper's `NamespaceService`; tests use a
/// fake. Implementations MUST be idempotent: a second call for an existing
/// namespace returns `Ok(())`.
#[async_trait::async_trait]
pub trait NamespaceProvisioner: Send + Sync {
    /// Create `namespace` under `warehouse_id` if absent; already-exists = `Ok`.
    async fn ensure_namespace(
        &self,
        warehouse_id: Uuid,
        namespace: &str,
    ) -> Result<(), NotebookApiError>;
}

/// The deterministic catalog-token label for a user's notebook runtime token. The
/// stable label is what lets us mint-and-revoke: revoking every prior token with
/// this label before minting keeps at most one active per (user×workspace).
fn runtime_token_label(user_id: Uuid) -> String {
    format!("notebook-runtime:{user_id}")
}

/// The private notebook namespace for `username`.
fn notebook_namespace(username: &str) -> String {
    format!("notebook_{username}")
}

/// Idempotently provision the caller's private notebook catalog access.
///
/// 1. Ensure namespace `notebook_{username}` exists under `warehouse_id`.
/// 2. Revoke any previously minted runtime token for this (user×workspace), then
///    mint a fresh 1-grant token scoped to exactly that namespace (read+write).
///
/// `warehouse_id` is the egret-side `workspace_warehouses.id` (the value the
/// `catalog_tokens.warehouse_id` FK references), NOT the lakekeeper warehouse
/// UUID. The [`NamespaceProvisioner`] is given the same id and is responsible for
/// resolving the lakekeeper UUID it needs.
///
/// Returns the namespace and the fresh **raw** token. Safe to call on every kernel
/// start: the namespace step is a no-op when present, and the token step always
/// returns a usable fresh secret while leaving at most one token active.
pub async fn provision_notebook_access(
    catalog: &CatalogStore,
    provisioner: &dyn NamespaceProvisioner,
    workspace_id: Uuid,
    user_id: Uuid,
    warehouse_id: Uuid,
    username: &str,
) -> Result<Provisioned, NotebookApiError> {
    let ns = notebook_namespace(username);

    // 1. Ensure the private namespace exists (already-exists = ok).
    provisioner.ensure_namespace(warehouse_id, &ns).await?;

    // 2. Mint-and-revoke: drop any prior token for this (user×workspace), then
    //    mint a fresh 1-grant token. Revoke first so a mint failure never leaves
    //    two active tokens; a stale revoked token is harmless.
    let label = runtime_token_label(user_id);
    catalog
        .revoke_tokens_by_label(workspace_id, &label)
        .await
        .map_err(catalog_err)?;

    let grant = Grant {
        namespace: ns.clone(),
        table: None,
        permissions: vec!["catalog:read".into(), "catalog:write".into()],
    };
    let (_token, raw) = catalog
        .create_token(workspace_id, warehouse_id, &label, &[grant])
        .await
        .map_err(catalog_err)?;

    Ok(Provisioned {
        namespace: ns,
        token: raw,
    })
}

/// Map a `CatalogError` into the notebook HTTP error type.
fn catalog_err(e: egret_catalog::CatalogError) -> NotebookApiError {
    use egret_catalog::CatalogError as C;
    let inner = match e {
        C::NotFound(m) => egret_core::Error::NotFound(m),
        C::AlreadyExists(m) => egret_core::Error::Conflict(m),
        C::BadRequest(m) => egret_core::Error::InvalidInput(m),
        C::Db(e) => egret_core::Error::Internal(anyhow::anyhow!(e)),
    };
    NotebookApiError(inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A fake namespace provisioner that records every (warehouse, namespace) it
    /// was asked to ensure. Always succeeds (already-exists = ok) so the test can
    /// focus on token mint/revoke against real Postgres.
    #[derive(Default)]
    struct FakeProvisioner {
        calls: Arc<std::sync::Mutex<Vec<(Uuid, String)>>>,
        count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl NamespaceProvisioner for FakeProvisioner {
        async fn ensure_namespace(
            &self,
            warehouse_id: Uuid,
            namespace: &str,
        ) -> Result<(), NotebookApiError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.calls
                .lock()
                .unwrap()
                .push((warehouse_id, namespace.to_string()));
            Ok(())
        }
    }

    /// Seed a workspace + user + a `workspace_warehouses` row, returning
    /// `(workspace_id, user_id, warehouse_row_id)`. The returned warehouse id is
    /// the egret-side `workspace_warehouses.id` (the value `create_token`'s
    /// `warehouse_id` FK references), mirroring `seed_warehouse_http` in
    /// `tests/http.rs`.
    async fn seed(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid) {
        let db = Db { pool: pool.clone() };
        let users = egret_auth::users::UserRepo::new(db.clone());
        let workspaces = egret_workspace::workspaces::WorkspaceRepo::new(db.clone());

        let suffix = Uuid::new_v4().simple().to_string();
        let user = users
            .insert_local(&format!("alice-{suffix}@egret.test"), "Alice", "x")
            .await
            .expect("seed user");
        let ws = workspaces
            .create(&format!("prov{suffix}"), "Prov WS", user)
            .await
            .expect("seed ws");

        let warehouse_row_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(ws.id.0)
        .bind(Uuid::new_v4()) // the lakekeeper warehouse uuid (unused by the token FK)
        .bind(format!("wh-{suffix}"))
        .fetch_one(pool)
        .await
        .expect("seed warehouse");

        (ws.id.0, user.0, warehouse_row_id)
    }

    /// Count non-revoked catalog tokens for a (workspace, label).
    async fn active_token_count(pool: &sqlx::PgPool, ws: Uuid, label: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_tokens \
             WHERE workspace_id = $1 AND label = $2 AND revoked_at IS NULL",
        )
        .bind(ws)
        .bind(label)
        .fetch_one(pool)
        .await
        .expect("count tokens")
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn provision_is_idempotent_and_mints_fresh_token(pool: sqlx::PgPool) {
        let (ws, user, warehouse_id) = seed(&pool).await;
        let store = CatalogStore::new(Db { pool: pool.clone() });
        let prov = FakeProvisioner::default();

        let p1 = provision_notebook_access(&store, &prov, ws, user, warehouse_id, "alice")
            .await
            .expect("first provision");
        let p2 = provision_notebook_access(&store, &prov, ws, user, warehouse_id, "alice")
            .await
            .expect("second provision");

        // Namespace name is deterministic and idempotent.
        assert_eq!(p1.namespace, "notebook_alice");
        assert_eq!(p2.namespace, "notebook_alice");
        // The namespace-ensure ran on each call (idempotent on the provider side).
        assert_eq!(prov.count.load(Ordering::SeqCst), 2);

        // Each call mints a FRESH raw token.
        assert_ne!(p1.token, p2.token);

        // After the second call exactly ONE active token exists for the label —
        // the first was revoked.
        let label = runtime_token_label(user);
        assert_eq!(active_token_count(&pool, ws, &label).await, 1);

        // The first token no longer authenticates; the second does.
        assert!(
            store.authenticate_token(&p1.token).await.is_err(),
            "the previous token must be revoked"
        );
        let auth = store
            .authenticate_token(&p2.token)
            .await
            .expect("fresh token authenticates");

        // The minted token's scope is exactly one grant on notebook_alice rw.
        assert_eq!(auth.workspace_id, ws);
        assert_eq!(auth.warehouse_id, warehouse_id);
        assert_eq!(auth.grants.len(), 1, "exactly one grant");
        let g = &auth.grants[0];
        assert_eq!(g.namespace, "notebook_alice");
        assert_eq!(g.table, None);
        let mut perms = g.permissions.clone();
        perms.sort();
        assert_eq!(perms, vec!["catalog:read", "catalog:write"]);
    }
}
