//! `egret backfill catalog-client-sa`: give every catalog client that predates
//! SA2 an owning workspace-scoped service account.
//!
//! The catalog verifier is fail-closed: a client with a NULL
//! `service_account_id` cannot mint, so until this runs the existing clients
//! are dark. Idempotent: only clients still NULL are touched, one transaction
//! each, so a mid-run failure leaves the rest retryable.
//!
//! Not `ServiceAccountRepo::create`: that derives the email from the account
//! name (not the client id, so no deterministic key for a retry) and does not
//! allocate a username. This reuses the same primitives instead
//! (`UserRepo::insert_service_with`, `ensure_username_via` for a ledger-correct
//! `svc-` name, and the `service_accounts` insert).

use anyhow::Context;
use egret_auth::users::{ensure_username_via, UserRepo};
use sqlx::PgPool;
use uuid::Uuid;

/// Result of one backfill run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackfillReport {
    /// Clients that were given a new service account.
    pub created: usize,
    /// Clients found NULL at scan time but already owned by commit time
    /// (a concurrent run); left untouched.
    pub skipped: usize,
}

/// Backfill every `catalog.catalog_clients` row whose `service_account_id IS NULL`.
pub async fn backfill_catalog_client_sa(pool: &PgPool) -> anyhow::Result<BackfillReport> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM catalog.catalog_clients WHERE service_account_id IS NULL ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await
    .context("listing catalog clients without a service account")?;

    let mut report = BackfillReport::default();
    for id in ids {
        if backfill_one(pool, id)
            .await
            .with_context(|| format!("backfilling catalog client {id}"))?
        {
            report.created += 1;
        } else {
            report.skipped += 1;
        }
    }
    Ok(report)
}

/// One client, one transaction. `Ok(false)` when it was already owned.
async fn backfill_one(pool: &PgPool, client_id: Uuid) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    // Lock the row and re-check: a concurrent run must not mint a second SA.
    let row: Option<(String, Uuid)> = sqlx::query_as(
        "SELECT name, workspace_id FROM catalog.catalog_clients \
          WHERE id = $1 AND service_account_id IS NULL FOR UPDATE",
    )
    .bind(client_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((client_name, workspace_id)) = row else {
        return Ok(false);
    };

    // `(workspace_id, name)` is unique among workspace SAs and client names are
    // not unique, so a taken name gets the client id appended.
    let mut sa_name = format!("catalog:{client_name}");
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM service_accounts \
          WHERE scope = 'workspace' AND workspace_id = $1 AND name = $2)",
    )
    .bind(workspace_id)
    .bind(&sa_name)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        sa_name = format!("{sa_name}-{client_id}");
    }

    let email = format!("svc-catalog-{client_id}@service.egret.local");
    let user_id = UserRepo::insert_service_with(&mut *tx, &email, &sa_name).await?;
    ensure_username_via(&mut tx, user_id).await?;
    sqlx::query(
        "INSERT INTO service_accounts (user_id, scope, workspace_id, name, description) \
         VALUES ($1, 'workspace', $2, $3, $4)",
    )
    .bind(user_id.0)
    .bind(workspace_id)
    .bind(&sa_name)
    .bind("Owns the catalog client; created by `egret backfill catalog-client-sa`")
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE catalog.catalog_clients SET service_account_id = $1 WHERE id = $2")
        .bind(user_id.0)
        .bind(client_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}
