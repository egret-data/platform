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

    /// Recompile ONE profile on the CALLER's own open transaction.
    ///
    /// The `PUT /engines/{id}` bind (Task 3) uses this: the engine row's
    /// `authz_service_id` write and the compile of the profile it now names
    /// commit together, so there is no window in which the engine claims a
    /// profile the compiler has never filled. Like
    /// [`recompile_catalog_assignment_in_tx`](Self::recompile_catalog_assignment_in_tx)
    /// — and unlike the fire-and-forget
    /// [`recompile_profile`](Self::recompile_profile), which runs after a
    /// cluster has already been provisioned — a failure here MUST fail and
    /// roll back the binding.
    ///
    /// The implementation takes the row locks through
    /// `egret_perm_compile::lock_services_in_tx`, which expands the set with
    /// `egret_policy::services_and_tag_services_via` (a profile's TAG service
    /// can have a LOWER id than the profile) and locks it ascending. That is
    /// the one ordering every recompile path shares; a second mechanism here
    /// would be a deadlock.
    async fn recompile_profile_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_id: i64,
    ) -> Result<(), egret_perm::CompileError>;

    /// The mask and row-filter pairs stored in every unit the profiles in
    /// `service_ids` belong to, read on the caller's transaction **before** a
    /// write that changes which catalogs — and so which tag assignments — a
    /// profile's unit resolves (a profile bind, an engine ↔ catalog
    /// attachment). Pair it with
    /// [`refuse_created_overlaps_in_tx`](Self::refuse_created_overlaps_in_tx)
    /// after the recompile (SP4d-4 Task 3 re-review M-2; the tag assignment and
    /// tag rename in `egret-bi` do the same through
    /// `egret_perm_compile::overlap`).
    async fn overlap_snapshot_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<OverlapSnapshot, egret_perm::CompileError>;

    /// Refuse, with `CompileError::AmbiguousMaskOrFilterArbitration`, any pair
    /// the units hold now that `snapshot` did not — only the pairs the write
    /// CREATED; a pair already stored is reported elsewhere and blocks nothing.
    async fn refuse_created_overlaps_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        snapshot: &OverlapSnapshot,
    ) -> Result<(), egret_perm::CompileError>;
}

/// What [`ProfileCompilerPort::overlap_snapshot_in_tx`] hands back. Opaque to
/// this crate: the pairs are `egret-policy` types, which this crate keeps out of
/// its production graph (see the module doc), so the composition root puts its
/// own snapshot type in and takes it out again.
pub struct OverlapSnapshot(Box<dyn std::any::Any + Send + Sync>);

impl OverlapSnapshot {
    pub fn new<T: std::any::Any + Send + Sync>(snapshot: T) -> Self {
        Self(Box::new(snapshot))
    }

    pub fn get<T: std::any::Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
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

/// The overlap snapshot of the profile `service_id`, or `None` when no compiler
/// is wired or the engine binds nothing.
pub(crate) async fn snapshot_overlaps_of(
    st: &crate::EngineMgmtState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    service_ids: &[i64],
) -> Result<Option<OverlapSnapshot>, egret_perm::CompileError> {
    let Some(compiler) = st.perm_compiler.clone() else {
        return Ok(None);
    };
    if service_ids.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        compiler.overlap_snapshot_in_tx(tx, service_ids).await?,
    ))
}

pub(crate) async fn snapshot_overlaps(
    st: &crate::EngineMgmtState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    service_id: Option<i64>,
) -> Result<Option<OverlapSnapshot>, egret_perm::CompileError> {
    let (Some(compiler), Some(service_id)) = (st.perm_compiler.clone(), service_id) else {
        return Ok(None);
    };
    Ok(Some(
        compiler.overlap_snapshot_in_tx(tx, &[service_id]).await?,
    ))
}

/// Recompile `service_id` on the caller's transaction and refuse the pairs the
/// write made since `before`. A no-op when no compiler is wired.
pub(crate) async fn recompile_refusing_created_overlaps(
    st: &crate::EngineMgmtState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    service_id: i64,
    before: Option<&OverlapSnapshot>,
) -> Result<(), egret_perm::CompileError> {
    let Some(compiler) = st.perm_compiler.clone() else {
        return Ok(());
    };
    compiler.recompile_profile_in_tx(tx, service_id).await?;
    if let Some(before) = before {
        compiler.refuse_created_overlaps_in_tx(tx, before).await?;
    }
    Ok(())
}

/// **Bind `engine_id` to the profile `profile_id`, refusing a pair the bind
/// creates** (SP4d-4 Task 7 fix round 1, C1). Used by the two engine-create
/// binds (`POST /engines` for Trino, adopting or creating a profile, and for
/// StarRocks with `authzServiceId`) and by the late-bind inside an apply.
///
/// One transaction: snapshot the profile's unit, write the binding, recompile,
/// compare. An overlap the bind creates is `Err(RuleOverlap)` and NOTHING is
/// bound (the transaction is dropped); the create path undoes the rest with
/// `compensate_failed_create`, and the late-bind fails the apply with the same 409.
///
/// A guarded path that cannot complete for any OTHER reason (a snapshot, compile
/// or database failure) REFUSES too (SP4d-4 M-3): binding without the check is
/// the thing the guard exists to prevent, and `PUT /engines/{id}` and the
/// catalog attach already refuse the same way. Only the deployment with no
/// compiler wired (`perm_compiler` is `None`) binds plainly, as it always has.
pub(crate) async fn bind_profile_guarded(
    st: &crate::EngineMgmtState,
    engine_id: Uuid,
    engine_name: &str,
    profile_id: i64,
) -> Result<(), crate::EngineHttpError> {
    if st.perm_compiler.is_some() {
        let status = match st.engine_mgmt.get_engine(engine_id).await? {
            Some(e) => e.status,
            None => return Err(crate::EngineHttpError::NotFound),
        };
        let mut tx = st
            .engine_mgmt
            .pool()
            .begin()
            .await
            .map_err(crate::http_error::db_error)?;
        let outcome: Result<bool, egret_perm::CompileError> = async {
            let before = snapshot_overlaps(st, &mut tx, Some(profile_id)).await?;
            let bound = st
                .engine_mgmt
                .set_engine_authz_service_in_tx(&mut tx, engine_id, Some(profile_id), &status)
                .await
                .map_err(|e| match e {
                    crate::EngineMgmtError::Db(e) => egret_perm::CompileError::from_db(e),
                    other => egret_perm::CompileError::Db(other.to_string()),
                })?;
            if !bound {
                return Ok(false);
            }
            recompile_refusing_created_overlaps(st, &mut tx, profile_id, before.as_ref()).await?;
            Ok(true)
        }
        .await;
        match outcome {
            Ok(true) => {
                tx.commit().await.map_err(crate::http_error::db_error)?;
                return Ok(());
            }
            Ok(false) => {
                return Err(crate::EngineHttpError::Conflict(
                    "engine changed while the policy-profile binding was being applied; retry"
                        .into(),
                ))
            }
            Err(e) => {
                drop(tx);
                return Err(bind_refusal(engine_name, profile_id, e));
            }
        }
    }
    st.engine_mgmt
        .set_engine_authz_service(engine_id, Some(profile_id))
        .await?;
    Ok(())
}

/// The refusal for a guarded bind that did not complete. An overlap the bind
/// created is the caller's 409; a database failure is ours (5xx, logged); any
/// other compile failure is a fact about the profile the operator can act on, so
/// it is a 409 that names the engine, the profile and the cause.
fn bind_refusal(
    engine_name: &str,
    profile_id: i64,
    e: egret_perm::CompileError,
) -> crate::EngineHttpError {
    if let Some(body) = e.rule_overlap_body() {
        return crate::EngineHttpError::RuleOverlap(body);
    }
    tracing::error!(engine = %engine_name, service_id = profile_id, error = %e,
        "the guarded bind could not complete; the engine was not bound");
    match e {
        egret_perm::CompileError::Db(_) | egret_perm::CompileError::LockTimeout(_) => {
            crate::EngineHttpError::Internal
        }
        other => crate::EngineHttpError::Conflict(format!(
            "engine {engine_name} was not bound to policy profile {profile_id}: the profile \
             could not be checked for order-ambiguous pairs: {other}"
        )),
    }
}

#[cfg(test)]
mod bind_refusal_tests {
    use super::*;
    use crate::EngineHttpError;

    #[test]
    fn a_database_failure_is_internal_and_binds_nothing_to_say() {
        let e = egret_perm::CompileError::Db("boom".into());
        assert!(matches!(bind_refusal("e", 7, e), EngineHttpError::Internal));
    }

    #[test]
    fn a_compile_failure_is_a_409_naming_the_engine_the_profile_and_the_cause() {
        let e = egret_perm::CompileError::ProfileExpansionCeiling {
            profile: "p".into(),
            total: 10,
            ceiling: 5,
            largest: Vec::new(),
        };
        match bind_refusal("eng-1", 7, e) {
            EngineHttpError::Conflict(m) => {
                assert!(m.contains("eng-1") && m.contains('7'), "{m}");
                assert!(m.contains("ceiling of 5"), "{m}");
                assert!(m.contains("not bound"), "{m}");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }
}
