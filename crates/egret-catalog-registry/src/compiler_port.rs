//! The seam this crate reaches the SP4a permission compiler through, for the
//! two write paths that need it: [`crate::store::RegistryStore::ensure_warehouse_catalog`]
//! here (an implicit assignment grant, created once per warehouse) and
//! `egret-catalog-engine::store::embedded::delete_warehouse_releasing_catalog`
//! (that grant's removal on teardown), which reuses this same trait.
//!
//! **Why a port and not a direct call (SP4a Task 16, D3 defects 2 and 3).**
//! This crate's own `Cargo.toml` says so explicitly: no `egret-engine-mgmt`
//! dependency, because engine-mgmt (and `egret-catalog-engine`) depend on
//! THIS crate — a reverse dependency on either would be a cycle. The compiler
//! itself (`egret-perm-compile`, which depends on `egret-policy`) is heavier
//! still and belongs only in the composition root, per the same reasoning
//! `egret-engine-mgmt::profile_compiler::ProfileCompilerPort` documents. So
//! this trait declares exactly what the two callers here need — "bring this
//! catalog's compiled policies up to date, in MY open transaction" — and
//! `egret-bin` supplies the implementation, reusing the exact derivation
//! `ProfileCompilerPort::recompile_catalog_assignment_in_tx` already uses
//! (`egret_policy::services_for_catalog_via`, then
//! `egret_perm_compile::{lock_services_in_tx, recompile_services_in_tx}`).
//! `egret_perm::CompileError` is already a safe type to name here: this crate
//! depends on `egret-perm` directly (for `sync_assignment_grants_in_tx`), and
//! that dependency is the one the Cargo.toml comment says is fine to carry —
//! it is `egret-perm-compile`/`egret-policy` (the compiler and the Ranger
//! seam) that must stay out.

use uuid::Uuid;

/// Recompile, on the CALLER's own open transaction, every profile a change to
/// `catalog_id` can affect.
///
/// `Err` MUST fail (and roll back) the caller's write: a grant stored/removed
/// but never (re)compiled is worse than a rejected write — the same reasoning
/// `egret_engine_mgmt::profile_compiler::ProfileCompilerPort::recompile_catalog_assignment_in_tx`
/// documents for its own callers.
#[async_trait::async_trait]
pub trait CatalogAssignmentCompiler: Send + Sync {
    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: Uuid,
    ) -> Result<(), egret_perm::CompileError>;

    /// Recompile exactly `service_ids` (ascending, deduplicated — the
    /// caller's job, same contract `egret_perm_compile::recompile_services_in_tx`
    /// itself documents) on the CALLER's own open transaction.
    ///
    /// **Why this exists alongside [`Self::recompile_catalog_assignment_in_tx`]
    /// (SP4a Task 16, D3 defect 3).** That method derives its target set from
    /// CURRENT catalog reachability — correct for a grant being written, but
    /// useless for a catalog being TORN DOWN: `delete_warehouse_releasing_catalog`
    /// only ever runs once every engine attachment is already gone (its own
    /// guard refuses otherwise), so "services this catalog reaches now" is
    /// always empty right there, by construction — recompiling that set would
    /// find nothing wrong. What CAN be wrong is a profile that reached this
    /// catalog in the PAST, compiled a policy for it, and never got told to
    /// drop that policy when it was later detached (`recompile_bound_profile`
    /// is fire-and-forget by contract and can silently fail). This method lets
    /// the caller name that set directly — sourced from
    /// `egret_perm::compiled_service_ids_for_catalog_via`'s own provenance
    /// join, which this crate can read without depending on the compiler.
    async fn recompile_services_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<(), egret_perm::CompileError>;
}
