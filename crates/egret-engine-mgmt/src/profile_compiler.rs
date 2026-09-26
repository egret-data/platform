//! The seam this crate reaches the SP4a permission compiler through
//! (SP4a Task 9 review).
//!
//! **Why a port and not a direct call.** The compiler lives in
//! `egret-perm-compile`, which depends on `egret-policy`. This crate must not:
//! `egret-policy-api::PolicyPort` exists precisely so the engine seam never
//! carries `egret-policy`'s internals in its production dependency graph
//! (`egret-policy` is a *dev*-dependency here), and `egret-perm-compile` was
//! split out of `egret-perm` in Task 6 for the same reason. So the engine seam
//! declares what it needs — "bring this profile's compiled policies up to date"
//! — and the composition root (`egret-bin`) supplies the implementation, the
//! same shape `egret-policy::EngineBindingReader` uses in the other direction.
//!
//! **Why this exists at all.** `egret_perm_compile`'s affected-service sets are
//! derived from the MUTATED ROW: a grant names a catalog, a rule names a tag or
//! a catalog. Binding an engine to a profile mutates no grant and no rule, yet
//! it changes the answer to "which profiles does this catalog reach" — so a
//! newly bound profile starts out behind the model and stays there until
//! somebody calls `POST /api/control-plane/perm/compile/{service_id}` by hand.
//! Nobody knows to. This is how the bind calls it for them.

use uuid::Uuid;

/// Recompile one policy profile's `perm`-derived Ranger policies.
///
/// `Err` is informational: every caller in this crate logs and carries on (see
/// `engines::recompile_bound_profile` for why a stale profile must not fail an
/// engine create that has already provisioned a cluster).
#[async_trait::async_trait]
pub trait ProfileCompilerPort: Send + Sync {
    /// `service_id` is a `policy.authz_services` id.
    async fn recompile_profile(&self, service_id: i64) -> Result<(), egret_perm::CompileError>;

    /// Recompile, on the CALLER's own open transaction, every profile
    /// `catalog_id` currently reaches — the same affected-service set a
    /// grant scoped to that catalog would use
    /// (`egret_perm_compile::affected_services_for_grant`).
    ///
    /// **SP4a Task 15 (D3 defect 1).** `catalogs::set_catalog_workspaces`
    /// composes `RegistryStore::set_catalog_workspaces_in_tx` and
    /// `egret_perm::sync_assignment_grants_in_tx` in one transaction, but
    /// neither of those writes a compiled policy — the live gate measured
    /// that an assignment's member was DENIED immediately after the write
    /// committed, and only started passing once an UNRELATED membership
    /// write recompiled the profile and swept the stale assignment in. This
    /// method is what closes that: called in the SAME transaction as the two
    /// writes above, so the compiled effect commits atomically with the
    /// assignment itself, never leaving a window where the grant row exists
    /// but the engine has not been told.
    ///
    /// Unlike [`recompile_profile`](Self::recompile_profile) — which is
    /// fire-and-forget, logs, and never fails its caller because a bind has
    /// already happened by the time it runs — a failure here MUST fail (and
    /// roll back) the assignment write: a grant stored but never compiled is
    /// worse than a rejected one (the same reasoning
    /// `egret_workspace::members::MemberRepo` documents for its own
    /// recompile call).
    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: Uuid,
    ) -> Result<(), egret_perm::CompileError>;
}

/// Recompile whatever profile `engine_id` is currently bound to, if any and if
/// a compiler is wired.
///
/// **Never fails its caller.** By the time a bind happens the engine row exists
/// and, on the create path, a cluster has been provisioned; refusing that over
/// a policy compile would destroy far more than it protects, and the profile's
/// compiled set was already stale before the bind — failing the create does not
/// make it less so. Every failure is logged at `error` naming the manual repair
/// trigger, so the staleness is visible rather than silent, which is the whole
/// improvement over doing nothing.
pub(crate) async fn recompile_bound_profile(
    st: &crate::EngineMgmtState,
    engine_id: Uuid,
    engine_name: &str,
) {
    let Some(compiler) = st.perm_compiler.clone() else {
        return;
    };
    let service_id = match st.engine_mgmt.get_engine(engine_id).await {
        Ok(Some(e)) => e.authz_service_id,
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(engine = %engine_name, error = %e,
                "could not re-read the engine to recompile its policy profile");
            return;
        }
    };
    let Some(service_id) = service_id else {
        return; // unbound: nothing to compile into
    };
    match compiler.recompile_profile(service_id).await {
        Ok(()) => tracing::debug!(
            engine = %engine_name,
            service_id,
            "recompiled the newly bound policy profile from the perm model"
        ),
        Err(e) => tracing::error!(
            engine = %engine_name,
            service_id,
            error = %e,
            "the newly bound policy profile could not be compiled from the perm model; \
             it will serve a stale policy set until POST \
             /api/control-plane/perm/compile/{service_id} succeeds"
        ),
    }
}
