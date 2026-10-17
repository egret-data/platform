//! **J-F3 Task 1b (design R8): who belongs to a group.** The compiler's half of
//! `egret_auth::groups::GroupMembershipRecompiler` — the port every writer of
//! `user_groups` calls, on its own transaction, so a membership change and the
//! engine policies it determines commit together or not at all.
//!
//! # Why a group-membership write needs a recompile at all
//!
//! A group NAME is unresolvable in every engine egret provisions: D7 measured no
//! `group-provider.properties` and no usersync, so `getGroups(user)` is empty
//! everywhere and a Ranger item naming a group matches nobody. Since R8 the
//! compiler therefore enumerates a non-`public` group's MEMBERS at compile time
//! ([`crate::members`]), exactly as SP4c enumerates a role's holders — and that
//! enumeration is stale the instant a row of `user_groups` changes. A missed
//! REMOVE keeps the ex-member granted by an allow, exempt from a mask, or
//! subtracted from a deny; that is a breach, and invisible, because egret's UI
//! reads the `user_groups` row, which was always written correctly.
//!
//! # Why this is a port and `role_membership` is not
//!
//! [`crate::role_membership`] owns the transaction and calls
//! `egret_auth::roles::RoleRepo`'s `_in_tx` SQL, which works because every role
//! writer is an HTTP handler in a crate above this one. The group writers are
//! not: the per-login OIDC sync lives in `egret-auth` itself, and
//! `UserRepo::hard_delete` is also called from `egret-dataapi`, which does not
//! depend on this crate. So the transaction stays with the writer and this crate
//! fills the seam, the inversion `egret_policy::ProfileRecompiler` already uses
//! for the policy editor (`crate::trigger::PermRecompiler`). The ORDER is the
//! same one `role_membership::with_recompile` documents — affected set, locks
//! ascending, write, recompile — and the writer, not this module, holds it;
//! `egret_auth::groups`' module doc says so at every writer's site.
//!
//! # Every writer, and how it reaches this
//!
//! - `GroupRepo::add_member` / `remove_member` — the admin console's
//!   `POST`/`DELETE /api/control-plane/groups/{id}/members`;
//! - `GroupRepo::sync_user_oidc_groups` — every OIDC login
//!   (`egret_auth::oidc::provision_and_sync`, from the callback route), only
//!   for the groups the login actually moved;
//! - `GroupRepo::delete_local` — `user_groups.group_id` is `ON DELETE CASCADE`;
//! - `GroupRepo::rename` — principals name a group by NAME, so a rename moves
//!   every member from the old name to the new one;
//! - `UserRepo::hard_delete` — the control plane's user delete and the Data
//!   API's service-account delete (`user_groups.user_id` cascades too).
//!
//! Each takes the port as a required argument; `egret-bin` wires
//! [`PermGroupRecompiler`] into the two states that hold one
//! (`OidcRoutesState`, `DataApiState`), and the control plane passes it
//! directly because it already depends on this crate.

use sqlx::{Postgres, Transaction};

use egret_perm::CompileError;

use crate::trigger::{
    affected_services_for_group_membership, lock_services_in_tx, recompile_services_in_tx,
};

/// The production [`egret_auth::groups::GroupMembershipRecompiler`]: the
/// affected set is [`affected_services_for_group_membership`], the recompile is
/// [`recompile_services_in_tx`] — the same two calls every other model write
/// in this crate makes.
pub struct PermGroupRecompiler;

/// Every compile failure is an `Internal` to the writer, the treatment
/// `egret-controlplane` already gives a role-membership write's
/// (`CpError::from(CompileError)`): the transaction rolled the membership write
/// back with it, so there is nothing the caller can fix by editing the request.
/// It is deliberately NOT a `Conflict`: the OIDC callback answers a `Conflict`
/// from `provision_and_sync` as "email already used by another account", which
/// would misname a refused recompile.
fn to_core(e: CompileError, group_names: Option<&[String]>) -> egret_core::Error {
    tracing::error!(
        error = %e,
        groups = ?group_names,
        "a group-membership write's policy recompile failed; the write is refused"
    );
    egret_core::Error::Internal(anyhow::anyhow!(
        "the engine policies this group-membership change reaches could not be recompiled: {e}"
    ))
}

#[async_trait::async_trait]
impl egret_auth::groups::GroupMembershipRecompiler for PermGroupRecompiler {
    async fn lock_affected_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        group_names: &[String],
    ) -> egret_core::Result<Vec<i64>> {
        let services = affected_services_for_group_membership(tx, group_names)
            .await
            .map_err(|e| to_core(e, Some(group_names)))?;
        lock_services_in_tx(tx, &services)
            .await
            .map_err(|e| to_core(e, Some(group_names)))?;
        Ok(services)
    }

    async fn recompile_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        service_ids: &[i64],
    ) -> egret_core::Result<()> {
        for summary in recompile_services_in_tx(tx, service_ids)
            .await
            .map_err(|e| to_core(e, None))?
        {
            if !summary.skipped.is_empty() {
                tracing::warn!(
                    skipped_count = summary.skipped.len(),
                    "a group-membership write's recompile skipped some items"
                );
            }
        }
        Ok(())
    }
}
