# P4: platform admin through groups (KI-284, KI-260, KI-285, #802)

Date: 2026-10-31. Branch `fix/platform-admin-through-groups`.

## Premise on main (99270626)

- **KI-284 reproduces.** Only four paths ran a last-admin check (user and
  group role unassign, disable, delete), each as a read before an unlocked
  write. Not guarded: removing a member from a group that holds
  `platform_admin` (`GroupRepo::change_member`), deleting such a group
  (`GroupRepo::delete_local`, the cascade drops every member), `PUT
  /api/admin/users/{id}/platform-roles` (`Authorizer::replace_platform_roles`),
  and the OIDC group sync (`GroupRepo::sync_user_oidc_groups`, every login, and
  the #778 stale-membership sweep that replays it with no claim). The guarded
  ones were racy: two removals of two admins could both read "another admin
  exists" and both commit.
- **The #796 lapsed-grant sweep is not a path.** It deletes nothing; it only
  recompiles profiles. No API writes an expiry on a platform-scope grant.
- **KI-260 / KI-285 reproduce.** `GET /api/control-plane/users` returns
  `roles` from `roles_of("user", id)` (direct only); the Users list, the user
  page, the disable dialog and `lastActiveAdminReason` read only that.
- **#802 reproduces** as described: eight inline predicates, one unused
  helper. Every one reads `Principal::platform_roles` (or the same query,
  `SessionService::platform_roles_of`), which the session already builds from
  direct AND group grants, unexpired.

## What changes

1. **One last-admin guard, inside the write's transaction**
   (`egret_auth::roles::LastAdminGuard`). `lock_via` takes one advisory lock
   (`PLATFORM_ADMIN_SET_LOCK_KEY`) and records whether an active admin exists;
   the writer writes; `refuse_if_emptied_via` re-reads on the same transaction
   and returns `Conflict("Cannot remove the last active platform admin")`
   (409, the existing message), which rolls the write back. "Active admin" is
   the existing `engine_admin_user_ids_via` definition (direct or group, active
   users). Used by: user/group role unassign, disable, user delete
   (`UserRepo::hard_delete`, so the Data API service-account delete is covered
   too), group member remove, group delete, `PUT .../platform-roles`, OIDC sync.
   - **Lock order:** after the user and group membership locks, before profile
     locks. No holder waits for a user/group lock, no profile-lock holder waits
     for it: no new deadlock cycle.
   - **Background:** the OIDC sync cannot refuse (it would fail the login, or
     the sweep would retry forever). It keeps the dropped memberships that carry
     `platform_admin`, logs a warning, and completes the rest.
   - The perm-compile revoke and status writes gain `_in_tx` variants so the
     control plane brackets them with the guard.
2. **Effective roles in the API.** `ControlPlaneUserDto.effectiveRoles`:
   `[{name, group}]`, `group` null for a direct grant, unexpired grants only
   (the session's rule). `roles` stays the direct set (what the editor can
   unassign). The FE never joins groups itself.
3. **FE.** Users list Roles column shows effective roles; a group-held role
   reads `platform_admin · via group platform-admins` (text, not colour). The
   user page keeps the direct-role editor and adds a read-only *Through groups*
   table. The disable dialog, the last-admin early check, the Impersonate menu
   item and the Access overview admin count use effective roles.
4. **#802.** `egret_core::context::{PLATFORM_ADMIN_ROLE, holds_platform_admin,
   Principal::is_platform_admin}` is the one predicate; a `PlatformAuth`
   extractor (egret-auth) refuses non-admins with 403. The eight sites route
   through it. No site changes who passes (each already read the session's
   direct+group role list).

## Decisions taken (not settled by the owner)

- The guard counts admins the way the existing guard and the engine-admin set
  do (expiry not considered); the UI list shows unexpired grants (the session's
  rule). They differ only for an expired platform grant, which no API writes.
- Group role *assign* and member *add* take no lock: they cannot shrink the set.
- The OIDC sync takes the lock only when it drops a membership, so an ordinary
  login does not serialize with admin writes.
