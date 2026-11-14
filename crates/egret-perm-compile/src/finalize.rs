//! SP3d Package C: close the switch to the permission model, irreversibly.
//!
//! Every migration (operator policies, role principals, clearances, workspace
//! assignments) is reversible only because it leaves provenance behind and
//! keeps the switched-off original. Finalize is the one-way door: once EVERY
//! enforcement point reads `complete`, it deletes that provenance and the
//! switched-off originals, so `undo` has nothing to restore and the managed
//! model is the sole source.
//!
//! **The originals must go with the provenance.** The classifier recognises a
//! switched-off operator policy as "migrated" only through
//! `perm.migrated_from`; with the row gone and the policy kept, it would read
//! as a plain disabled policy, the EP would flip back to incomplete, and the
//! gate would stop meaning what finalize just asserted.
//!
//! One transaction, fail-closed:
//! 1. every EP's live gate must be `complete` (else 409 naming how many remain);
//! 2. capture the originals' ids (`perm.migrated_from.policy_id` is
//!    `ON DELETE NO ACTION`, migration 0181, so the policy cannot go first);
//! 3. delete the four provenance tables;
//! 4. delete the captured originals through `egret_policy::delete_policy_via`,
//!    the store's own path (compiler/infrastructure-ownership guards, and the
//!    service's `policy_version` bump). Any refusal rolls everything back.
//!
//! `perm.compiled` rows reference the policy `ON DELETE CASCADE`; a switched-off
//! original has none (the compiler never emitted it), and the cascade is the
//! behaviour that table documents for policy deletion anyway.
//!
//! Idempotent: with nothing left, every delete matches zero rows.

use serde::Serialize;
use sqlx::{PgConnection, Postgres, Transaction};

use crate::routes::PermHttpError;

/// What finalize removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct FinalizeSummary {
    /// Rows deleted from `perm.migrated_from`.
    pub policy_migrations_deleted: i64,
    /// Rows deleted from `perm.clearance_migration`.
    pub clearance_migrations_deleted: i64,
    /// Rows deleted from `perm.role_principal_migration`.
    pub role_principal_migrations_deleted: i64,
    /// Rows deleted from `perm.ws_assignment_migration`.
    pub ws_assignment_migrations_deleted: i64,
    /// Switched-off original operator policies deleted.
    pub original_policies_deleted: i64,
}

/// How many EPs are not migration-complete, and how many exist. Reads the live
/// gate per EP (fail-closed: a gate that cannot be read is an error, not a pass).
pub async fn incomplete_eps(conn: &mut PgConnection) -> Result<(usize, usize), PermHttpError> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM policy.authz_services ORDER BY id")
        .fetch_all(&mut *conn)
        .await
        .map_err(PermHttpError::db)?;
    let mut incomplete = 0;
    for id in &ids {
        if !crate::ep_gate::ep_gate_via(conn, *id).await?.complete {
            incomplete += 1;
        }
    }
    Ok((incomplete, ids.len()))
}

/// Which provenance a delete reaches. Production only ever uses
/// [`Scope::All`]; [`Scope::Services`] exists so a test on a shared database can
/// prove the delete logic without erasing its neighbours' provenance.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// Every provenance row, all four tables.
    All,
    /// Only `perm.migrated_from` rows of these profiles (and their originals);
    /// the three installation-wide tables are left alone.
    Services(&'a [i64]),
}

/// Delete the provenance and the switched-off originals. NO gate check — the
/// caller ([`finalize_in_tx`]) owns the guard.
#[doc(hidden)]
pub async fn delete_legacy_inputs_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    scope: Scope<'_>,
) -> Result<FinalizeSummary, PermHttpError> {
    let services: Option<&[i64]> = match scope {
        Scope::All => None,
        Scope::Services(s) => Some(s),
    };
    let original_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT policy_id FROM perm.migrated_from \
          WHERE $1::bigint[] IS NULL OR service_id = ANY($1) ORDER BY policy_id",
    )
    .bind(services)
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;

    let policy_migrations_deleted = sqlx::query(
        "DELETE FROM perm.migrated_from WHERE $1::bigint[] IS NULL OR service_id = ANY($1)",
    )
    .bind(services)
    .execute(&mut **tx)
    .await
    .map_err(PermHttpError::db)?
    .rows_affected() as i64;

    let (mut clearance, mut role_principal, mut ws_assignment) = (0, 0, 0);
    if services.is_none() {
        clearance = delete_all(tx, "DELETE FROM perm.clearance_migration").await?;
        role_principal = delete_all(tx, "DELETE FROM perm.role_principal_migration").await?;
        ws_assignment = delete_all(tx, "DELETE FROM perm.ws_assignment_migration").await?;
    }

    for id in &original_ids {
        egret_policy::delete_policy_via(tx, *id).await?;
    }

    Ok(FinalizeSummary {
        policy_migrations_deleted,
        clearance_migrations_deleted: clearance,
        role_principal_migrations_deleted: role_principal,
        ws_assignment_migrations_deleted: ws_assignment,
        original_policies_deleted: original_ids.len() as i64,
    })
}

async fn delete_all(
    tx: &mut Transaction<'_, Postgres>,
    sql: &'static str,
) -> Result<i64, PermHttpError> {
    let n = sqlx::query(sql)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?
        .rows_affected();
    Ok(n as i64)
}

/// The guarded finalize: refuse (409, nothing written) unless every EP is
/// complete, then [`delete_legacy_inputs_in_tx`].
pub async fn finalize_in_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<FinalizeSummary, PermHttpError> {
    let (incomplete, total) = incomplete_eps(tx).await?;
    if incomplete > 0 {
        return Err(PermHttpError::Conflict(format!(
            "{incomplete} of {total} enforcement points are not migration-complete yet; \
             finish migrating them before finalizing. Nothing was changed."
        )));
    }
    delete_legacy_inputs_in_tx(tx, Scope::All).await
}
