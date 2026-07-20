# Local-User Management Gaps + STS Cleanup — Design

> **Status:** Design approved (brainstorm). Next: implementation plan.
> **Scope:** Sub-project 2 of 2 of the Egret STS effort. Builds on sub-project 1
> (STS token broker, merged #304 `045d0cd`).

**Goal:** Close the remaining gaps in egret's local-user administration and retire
the now-dead Keycloak refresh-token path. Most local-user CRUD already exists
(platform-RBAC #174 + access-control series): the control-plane Access console
(`UsersPage`/`GroupsPage`) already creates local password users, edits name,
enables/disables (`status`), assigns roles, and manages group membership; and
sub-project 1 already made `SessionService::validate` reject a disabled user. This
sub-project fills four narrow gaps.

## 1. Scope (the four gaps)

1. **Password reset for local users** — no way to change/reset a local user's
   password after creation.
2. **Delete user (hard)** — no `DELETE /users/{id}`; only enable/disable exists.
3. **Immediate lockout** — disabling a user is only enforced on their *next*
   request; make disable/reset/delete revoke the user's sessions *now*.
4. **STS cleanup** — remove the unused `oidc_refresh_tokens` write + `offline_access`
   scope left behind after the Keycloak engine broker was deleted in #304.

## 2. Approach decisions (settled)

| Decision | Choice |
| --- | --- |
| Delete semantics | **Hard delete** (distinct from the existing `disabled`), transactional cleanup of dependents, with self + last-admin guards. |
| Session revocation | **Active** — a shared `SessionRepo::revoke_all_for_user` called on disable, password-reset, and delete (immediate lockout, not next-request). |

## 3. Components

### 3.1 Shared revoke seam
`SessionRepo::revoke_all_for_user(user_id) -> Result<u64>` — `DELETE FROM sessions
WHERE user_id = $1`, returns the count. The single choke point for all three
lockout triggers, so "immediate lockout" is one tested primitive.

### 3.2 Password reset
- **Backend:** `POST /api/control-plane/users/{id}/password` `{ password }` — gate
  `user:update`; `404` unknown id; `400` if the target's `provider != "local"`
  (an SSO account has no local password); `hash_password(password)` →
  `UserRepo::set_password(id, hash)` → `revoke_all_for_user(id)` (force re-login).
- **Frontend:** a "Reset password" row action in `UsersPage` (dialog with a
  password field), shown for `provider == "local"` users only.

### 3.3 Delete user (hard)
- **Backend:** `DELETE /api/control-plane/users/{id}` — new permission
  `user:delete` (`platform_admin` bypasses). Guards: `404` unknown id; `400` if
  the target is the **caller's own** account; `400` if the target is a
  `platform_admin` and no *other* active platform_admin remains (last-admin
  lockout). On success, **one transaction**: `revoke_all_for_user` + delete
  `user_groups` + delete role grants (`access_grants` where principal = user) +
  delete `oidc_refresh_tokens` + delete the `users` row. Audit-log rows reference
  the actor — confirm at implementation that the audit ref is denormalized or
  nullable; if a hard FK would block the delete, add `ON DELETE SET NULL` for that
  ref in the migration (do not cascade-delete audit history).
- **Frontend:** a destructive "Delete" row action in `UsersPage` with a
  confirmation dialog.

### 3.4 Immediate lockout (wire the seam)
Call `revoke_all_for_user` from the existing `update_user` when `status`
transitions to `disabled`, and from password-reset and delete. Apply the
last-admin guard to **disabling an admin** too (same lockout risk as delete).

### 3.5 STS cleanup
Remove the dead Keycloak refresh path (nothing reads it after #304):
- `crates/egret-auth/src/oidc_routes.rs` — drop the refresh-token write block
  (~209-237) and the `refresh_repo` field (~31).
- `crates/egret-auth/src/oidc.rs` — drop `offline_access` from the OIDC
  auth-request scope, and the refresh-token capture in the login exchange.
- `crates/egret-bin/src/main.rs:~1044` — remove the `OidcRefreshRepo`
  construction.
- Delete `crates/egret-auth/src/oidc_refresh.rs`.
- Migration: `DROP TABLE oidc_refresh_tokens`. Touch `metastore/src/lib.rs` so the
  embedded migrator re-embeds (known sqlx-staleness gotcha).

## 4. Permission

Add `user:delete` to the code-defined permission catalog (where
`user:create`/`user:update`/`user:disable` are defined, `egret-auth`/`egret-core`).
`platform_admin` bypasses; the delete route gates on it.

## 5. Error handling

`404` unknown user; `400` on self-target / last-admin / non-local-password-reset /
invalid input; `403` on missing permission. Delete is transactional — a
mid-cleanup failure rolls back, never a half-deleted user.

## 6. Testing

- **Unit/DB (`sqlx`):** `revoke_all_for_user` deletes exactly the target's
  sessions; `set_password` + revoke; `update_user` disable revokes sessions;
  delete removes the user + all dependents (user_groups, grants, refresh_tokens,
  sessions) and rolls back on a forced mid-transaction failure; self-guard and
  last-admin-guard each reject `400`.
- **STS cleanup:** an OIDC login still succeeds **without** `offline_access` (a
  session is created, no `oidc_refresh_tokens` row written); update/remove any
  test that asserted refresh-token storage.
- **Live e2e (kind) — the disable→lockout proof (SP2 headline):** create a local
  user via the console → log in as them (obtain a session) → admin
  `PATCH status=disabled` → assert the user's next authenticated request is
  **401 / logged out** immediately. Plus: password-reset invalidates the old
  session; delete makes the account gone (login fails). New
  `local-user-mgmt-kind.spec.ts` + a `just` recipe (or extend `sts-broker-kind`).

## 7. Task decomposition (for the plan)

1. `SessionRepo::revoke_all_for_user` seam + wire into `update_user` disable.
2. Password-reset route (`set_password` + revoke) + `UsersPage` UI.
3. Delete route (`user:delete` perm + self/last-admin guards + transactional
   cleanup) + `UsersPage` UI.
4. STS cleanup (remove refresh write + `offline_access` + `OidcRefreshRepo`; drop
   table).
5. Live e2e: disable→lockout (+ password-reset re-login + delete).

## 8. Out of scope

- IdP-driven revocation / SSO back-channel logout (future).
- Soft-delete / archival (the existing `disabled` status covers "keep but
  deactivate"; delete is a true removal).
- Bulk import / external user provisioning.
