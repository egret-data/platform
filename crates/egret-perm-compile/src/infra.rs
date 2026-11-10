//! Convergence of a profile's infrastructure policies (governance SP2a, D4).
//!
//! The IR owns three shapes of infrastructure row
//! ([`egret_policy::infra_names::is_ir_infra_name`]): the `egret-admin - …`
//! set, the fixed names (the four Trino ones and the StarRocks query-log
//! loader) and the StarRocks `egret-query-log-guard - …` set. [`converge_infra_in_tx`] derives every such
//! row a profile should hold from the store (the bound engines and the engine
//! admins, through [`egret_perm_ir::lower::infra::lower_infra`] and
//! [`crate::ranger::infra::render`]) and reconciles the stored rows by name.
//! The `all - …` default set is outside the IR: convergence never reads,
//! writes or deletes it.
//!
//! It runs before every compile (`trigger::recompile_locked_service_in_tx`,
//! the chokepoint the `egret-admin` sync held before it) and on its own: the
//! kill switch (`role_membership::set_user_status_and_recompile_in_tx`), the
//! boot pass ([`crate::role_membership::converge_infra_policies`]) and engine
//! apply ([`converge_infra_policy`]). None of those depends on the profile
//! compiling.

use std::collections::BTreeSet;

use egret_perm::CompileError;
use egret_policy::infra_names::{
    is_admin_all_access_policy_name, is_ir_infra_name, ADMIN_ALL_ACCESS_NAME_PREFIX,
    EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME, QUERY_LOG_GUARD_NAME_PREFIX,
    QUERY_LOG_LOADER_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME, SELF_IMPERSONATE_POLICY_NAME,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

/// What [`converge_infra_in_tx`] did to one profile's IR-owned infra rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfraSync {
    /// Whether any row was inserted, updated or deleted (and so
    /// `policy_version` moved, once).
    pub changed: bool,
    /// Names of the rows inserted, in render order.
    pub added: Vec<String>,
    /// Names of the rows deleted, sorted.
    pub removed: Vec<String>,
}

/// One stored IR-owned infra row.
struct Stored {
    id: i64,
    name: String,
    body: Value,
    managed_by: String,
}

/// Reconcile `service_id`'s IR-owned infrastructure rows with what the store
/// derives for it, on the caller's transaction (D4):
///
/// - a desired row that is present is updated in place, id kept, only when its
///   body differs or it is not `managed_by = 'infra'` (`version` moves then);
/// - a desired row that is absent is inserted `managed_by = 'infra'`, in
///   render order, after the same name guards the old writers ran;
/// - a present row that is not desired is deleted;
/// - `policy_version` is bumped once when any of the above wrote.
///
/// A profile this compiler does not target gets [`InfraSync::default`]. The
/// caller holds the profile row lock and then the compile-mint lock (an
/// engine admin's missing username is minted here).
pub async fn converge_infra_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<InfraSync, CompileError> {
    let Some(loaded) =
        crate::ir_load::load_infra_inputs(tx, service_id, crate::members::MissingUsername::Mint)
            .await?
    else {
        return Ok(InfraSync::default());
    };
    let engine = loaded.inputs.engine;
    let mut desired: Vec<(String, Value)> = Vec::new();
    for statement in egret_perm_ir::lower::infra::lower_infra(&loaded.inputs) {
        desired.extend(crate::ranger::infra::render(
            &statement,
            &loaded.service_name,
            engine,
        )?);
    }

    let stored = stored_infra_rows(tx, service_id).await?;
    let named_before = admin_users(stored.iter().map(|s| (&s.name, &s.body)));
    let named_now = admin_users(desired.iter().map(|(n, b)| (n, b)));

    let mut sync = InfraSync::default();
    let mut updated: Vec<String> = Vec::new();
    for (name, body) in &desired {
        match stored.iter().find(|s| &s.name == name) {
            Some(row) => {
                if row.body == *body && row.managed_by == "infra" {
                    continue;
                }
                let wrote: Option<i64> = sqlx::query_scalar(
                    "UPDATE policy.authz_policies \
                        SET policy_jsonb = $2, managed_by = 'infra', \
                            version = version + 1, updated_at = now() \
                      WHERE id = $1 \
                        AND (policy_jsonb IS DISTINCT FROM $2 OR managed_by <> 'infra') \
                     RETURNING id",
                )
                .bind(row.id)
                .bind(body)
                .fetch_optional(&mut **tx)
                .await
                .map_err(CompileError::from_db)?;
                if wrote.is_some() {
                    updated.push(name.clone());
                }
            }
            None => {
                egret_policy::store::refuse_reserved_name(name)
                    .map_err(crate::db_error::from_store)?;
                egret_policy::store::refuse_non_infrastructure_name(&loaded.service_type, name)
                    .map_err(crate::db_error::from_store)?;
                sqlx::query(
                    "INSERT INTO policy.authz_policies \
                       (service_id, name, policy_jsonb, managed_by) \
                     VALUES ($1, $2, $3, 'infra')",
                )
                .bind(service_id)
                .bind(name)
                .bind(body)
                .execute(&mut **tx)
                .await
                .map_err(CompileError::from_db)?;
                sync.added.push(name.clone());
            }
        }
    }
    for row in &stored {
        if desired.iter().any(|(n, _)| n == &row.name) {
            continue;
        }
        sqlx::query("DELETE FROM policy.authz_policies WHERE id = $1")
            .bind(row.id)
            .execute(&mut **tx)
            .await
            .map_err(CompileError::from_db)?;
        sync.removed.push(row.name.clone());
    }
    sync.removed.sort();

    sync.changed = !sync.added.is_empty() || !updated.is_empty() || !sync.removed.is_empty();
    if sync.changed {
        crate::trigger::bump_policy_version(tx, service_id).await?;
        tracing::info!(
            service_id,
            added = ?sync.added,
            updated = ?updated,
            removed = ?sync.removed,
            admins_removed = ?named_before.difference(&named_now).collect::<Vec<_>>(),
            admins_added = ?named_now.difference(&named_before).collect::<Vec<_>>(),
            "infrastructure policies converged"
        );
    }
    Ok(sync)
}

/// `service_id`'s stored rows whose name the IR owns. The SQL narrows by the
/// shapes' spellings; [`is_ir_infra_name`] is the predicate.
async fn stored_infra_rows(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<Vec<Stored>, CompileError> {
    let rows: Vec<(i64, String, Value, String)> = sqlx::query_as(
        "SELECT id, name, policy_jsonb, managed_by FROM policy.authz_policies \
          WHERE service_id = $1 \
            AND (starts_with(name, $2) OR starts_with(name, $4) OR name = ANY($3)) \
          ORDER BY id",
    )
    .bind(service_id)
    .bind(ADMIN_ALL_ACCESS_NAME_PREFIX)
    .bind(fixed_names())
    .bind(QUERY_LOG_GUARD_NAME_PREFIX)
    .fetch_all(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    Ok(rows
        .into_iter()
        .filter(|(_, name, _, _)| is_ir_infra_name(name))
        .map(|(id, name, body, managed_by)| Stored {
            id,
            name,
            body,
            managed_by,
        })
        .collect())
}

fn fixed_names() -> Vec<String> {
    [
        IMPERSONATE_POLICY_NAME,
        SELF_IMPERSONATE_POLICY_NAME,
        EXECUTE_QUERY_POLICY_NAME,
        QUERY_TIMEOUT_POLICY_NAME,
        QUERY_LOG_LOADER_POLICY_NAME,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The users the `egret-admin - …` bodies among `rows` name, for the log.
fn admin_users<'a>(rows: impl Iterator<Item = (&'a String, &'a Value)>) -> BTreeSet<String> {
    rows.filter(|(name, _)| is_admin_all_access_policy_name(name))
        .flat_map(|(_, body)| {
            body["policyItems"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|item| item["users"].as_array().into_iter().flatten())
                .filter_map(|u| u.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every service convergence has something to do on, ascending and deduped:
/// those holding an IR-owned infra row (to keep or delete), those a
/// qualifying engine is bound to (`egret_core::engine_delegation::
/// qualifies_for_infra`) and those a query-log-writing StarRocks engine is
/// bound to ([`crate::ir_load::writes_query_log`]). Callers lock them in this
/// order.
pub(crate) async fn infra_services_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<i64>, CompileError> {
    use egret_core::engine_delegation::qualifies_for_infra;

    let holding: Vec<(i64, String)> = sqlx::query_as(
        "SELECT service_id, name FROM policy.authz_policies \
          WHERE starts_with(name, $1) OR starts_with(name, $3) OR name = ANY($2)",
    )
    .bind(ADMIN_ALL_ACCESS_NAME_PREFIX)
    .bind(fixed_names())
    .bind(QUERY_LOG_GUARD_NAME_PREFIX)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    let bound: Vec<(i64, String, String, Value)> = sqlx::query_as(
        "SELECT authz_service_id, type, origin, spec FROM engine_mgmt.engines \
          WHERE authz_service_id IS NOT NULL",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    let ids: BTreeSet<i64> = holding
        .into_iter()
        .filter(|(_, name)| is_ir_infra_name(name))
        .map(|(id, _)| id)
        .chain(
            bound
                .into_iter()
                .filter(|(_, t, origin, spec)| {
                    qualifies_for_infra(t, origin, spec)
                        || crate::ir_load::writes_query_log(t, origin)
                })
                .map(|(id, _, _, _)| id),
        )
        .collect();
    Ok(ids.into_iter().collect())
}

/// [`infra_services_via`] on a pooled connection: a test hook.
#[doc(hidden)]
pub async fn infra_services(pool: &PgPool) -> Result<Vec<i64>, CompileError> {
    let mut conn = pool.acquire().await.map_err(CompileError::from_db)?;
    infra_services_via(&mut conn).await
}

/// Converge `service_id`'s infrastructure policies in a transaction of its
/// own, under the profile row lock and then the compile-mint lock (the order
/// every compile takes them), without compiling the profile. Engine apply's
/// path (`ProfileCompilerPort::converge_infra`).
pub async fn converge_infra_policy(
    pool: &PgPool,
    service_id: i64,
) -> Result<InfraSync, CompileError> {
    let db = CompileError::from_db;
    let mut tx = pool.begin().await.map_err(db)?;
    crate::role_membership::lock_for_admin_sync_in_tx(&mut tx, &[service_id]).await?;
    let sync = converge_infra_in_tx(&mut tx, service_id).await?;
    tx.commit().await.map_err(db)?;
    Ok(sync)
}
