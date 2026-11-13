//! SP3c-1: the managed `role:<name> holders` group that mirrors a role's holders.

use crate::members;
use egret_perm::CompileError;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// The managed group that mirrors a role's holders. The naming is load-bearing:
/// principal-ref validation and the migration rely on it.
pub fn role_holder_group_name(role: &str) -> String {
    format!("role:{role} holders")
}

/// Upsert the `role:<name> holders` managed group and reconcile its
/// `user_groups` membership to exactly the role's holder user ids (direct user
/// holders plus members of groups holding the role). Returns the group id.
/// Idempotent; safe to call on every holder change.
pub async fn sync_role_holder_group_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    role: &str,
) -> Result<Uuid, CompileError> {
    let name = role_holder_group_name(role);
    let group_id: Uuid = sqlx::query_scalar(
        "INSERT INTO public.groups (id, name, source) VALUES (gen_random_uuid(), $1, 'managed') \
         ON CONFLICT (name) DO UPDATE SET source = 'managed' RETURNING id",
    )
    .bind(&name)
    .fetch_one(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;

    let roles = [role.to_string()];
    let mut want: std::collections::BTreeSet<Uuid> = std::collections::BTreeSet::new();
    for m in members::role_holders_via(&mut **tx, &roles).await? {
        want.insert(m.user_id);
    }
    for m in members::role_group_holders_via(&mut **tx, &roles).await? {
        want.insert(m.user_id);
    }
    let want: Vec<Uuid> = want.into_iter().collect();

    sqlx::query("DELETE FROM public.user_groups WHERE group_id = $1 AND user_id <> ALL($2)")
        .bind(group_id)
        .bind(&want)
        .execute(&mut **tx)
        .await
        .map_err(CompileError::from_db)?;
    sqlx::query(
        "INSERT INTO public.user_groups (user_id, group_id) \
         SELECT u, $1 FROM UNNEST($2::uuid[]) AS u ON CONFLICT (user_id, group_id) DO NOTHING",
    )
    .bind(group_id)
    .bind(&want)
    .execute(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    Ok(group_id)
}
