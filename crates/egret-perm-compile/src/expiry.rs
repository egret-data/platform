//! The expiry sweep: closes narrowing 2 of [`crate::members`].
//!
//! An `access_grants` row carrying `expires_at` is read by every compile only
//! while it is live (`expires_at > now()`), so a lapse is not a write and
//! nothing recompiles the profiles that named the holder. Until something else
//! recompiles them, the lapsed holder stays in the compiled policy.
//!
//! [`recompile_lapsed_grants`] runs hourly (`egret-bin`). Each run finds the
//! rows that lapsed inside [`LAPSE_DETECTION_WINDOW`], derives the profiles
//! each one can change with the same `affected_services_for_*` functions the
//! write paths use, and recompiles that union in one transaction. The window is
//! two hours against an hourly tick, so every lapse is seen by at least one run
//! and a run that errored is retried by the next one. An unchanged profile
//! writes nothing, because the compile diffs before it writes.
//!
//! Only `access_grants` carries an expiry that reaches the compile. The
//! `perm.*` grant and rule tables have none, so they are not swept.

use std::time::Duration;

use egret_perm::CompileError;
use sqlx::PgPool;
use uuid::Uuid;

use crate::trigger::{
    affected_services_for_role_membership, affected_services_for_workspace_members,
    lock_services_in_tx, recompile_services_in_tx,
};

/// How far back a run looks for lapses. Wider than the hourly tick, so a lapse
/// is seen by two runs and a failed run is retried by the next.
pub const LAPSE_DETECTION_WINDOW: Duration = Duration::from_secs(2 * 3600);

/// What one run of the sweep did. Both profile sets are ascending and
/// deduplicated, as every compile entry point requires.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepSummary {
    /// Lapsed `access_grants` rows the run found.
    pub lapsed_rows: usize,
    /// Profiles whose compile was run (ascending service ids).
    pub services: Vec<i64>,
    /// Of `services`, the profiles whose compiled output changed (ascending
    /// service ids). Named rather than counted: the sweep reaches every
    /// profile with a lapse in the window, so a count cannot tell one
    /// profile's convergence from another profile's first rewrite.
    pub rewritten: Vec<i64>,
}

/// Recompile every profile that a grant lapsed inside [`LAPSE_DETECTION_WINDOW`]
/// can change, in one transaction. Idempotent: a run with nothing new, or a
/// profile already converged, writes nothing.
pub async fn recompile_lapsed_grants(pool: &PgPool) -> Result<SweepSummary, CompileError> {
    let db = |e: sqlx::Error| CompileError::Db(e.to_string());
    let window_secs = LAPSE_DETECTION_WINDOW.as_secs_f64();
    let mut tx = pool.begin().await.map_err(db)?;

    // One row per distinct (role, scope) that lapsed. The role name is what
    // both role consumers key on; a workspace-scoped row also changes the
    // workspace's member enumeration.
    let lapsed: Vec<(String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT DISTINCT r.name, ag.scope_type, ag.scope_id \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
          WHERE ag.expires_at IS NOT NULL \
            AND ag.expires_at <= now() \
            AND ag.expires_at > now() - make_interval(secs => $1)",
    )
    .bind(window_secs)
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;

    let lapsed_rows = lapsed.len();
    let mut services: Vec<i64> = Vec::new();
    let mut roles: Vec<&str> = lapsed.iter().map(|(name, _, _)| name.as_str()).collect();
    roles.sort_unstable();
    roles.dedup();
    for role in roles {
        services.extend(affected_services_for_role_membership(&mut tx, role).await?);
    }
    for (_, scope_type, scope_id) in &lapsed {
        if let ("workspace", Some(workspace_id)) = (scope_type.as_str(), scope_id) {
            services.extend(affected_services_for_workspace_members(&mut tx, *workspace_id).await?);
        }
    }
    services.sort_unstable();
    services.dedup();
    if services.is_empty() {
        return Ok(SweepSummary {
            lapsed_rows,
            ..SweepSummary::default()
        });
    }

    lock_services_in_tx(&mut tx, &services).await?;
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    tx.commit().await.map_err(db)?;

    let rewritten: Vec<i64> = services
        .iter()
        .zip(&summaries)
        .filter(|(_, s)| s.wrote_anything())
        .map(|(&id, _)| id)
        .collect();
    tracing::info!(
        lapsed_rows,
        profiles = services.len(),
        rewritten = rewritten.len(),
        "recompiled the profiles that named a grant which lapsed"
    );
    Ok(SweepSummary {
        lapsed_rows,
        services,
        rewritten,
    })
}
