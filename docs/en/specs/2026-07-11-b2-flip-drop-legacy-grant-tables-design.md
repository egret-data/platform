# B2-flip — Drop the legacy grant tables, unify on access_grants

> **Status:** Design (approved to plan) · **Date:** 2026-07-11 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B2) · **Predecessors:** B1a–B1b-final (PDP + PEP cutover), B2-1a/1b/2/3 (object scope, sharing, notebooks, role hierarchy + deny).
>
> The deferred data-model flip. `access_grants` has been the read-model since B1a, kept in sync with the two legacy write-tables (`platform_role_assignments`, `workspace_members`) by triggers. B2-flip makes `access_grants` the **single source of truth**: repoint every writer and reader at it, then drop the triggers and the two legacy tables.

---

## 1. Goal & scope

`access_grants` (migration 0068) is the PDP's read-model; the two legacy tables remain the write path and are mirrored into `access_grants` by the 0068/0069 sync triggers. This dual-write is the last piece of the pre-Effort-B data model. B2-flip removes it: **`access_grants` becomes the only grant store.**

This is a **behavior-preserving refactor**. Every public method keeps its signature (`RoleRepo::assign/unassign`, `Authorizer::replace_platform_roles/workspace_roles_for`, `MemberRepo::add/remove/list`, `SessionService` derive, `WorkspaceRepo::list_for_principal`); only the SQL inside changes — from the legacy table to `access_grants`. The **contents** of `access_grants` do not change (the flip changes *who writes it*, not *what is in it*), so the **PDP decision matrix is unchanged**.

**In scope:** repoint all platform-grant writers/readers and all workspace-grant writers/readers to `access_grants`; a migration (0073) that defensively re-backfills, drops the two sync triggers + their functions, and drops the two legacy tables; fix any test that seeds/reads the legacy tables directly.

**Out of scope:** applying the B2-3 role-hierarchy closure to `Principal.permissions` (FE gating stays flat/own-perms/platform-only — behavior-preserving; a separate concern); B3 (per-user data-plane delegation); B4 (audit_log); any change to the object-grant or share paths (already on `access_grants`).

**Decisions locked in brainstorming:** one increment (platform + workspace together); **preserve one-workspace-role-per-member** (`MemberRepo::add` deletes the principal's existing workspace grants for that workspace before inserting the new one, matching the old `workspace_members` primary key); **drop the legacy tables definitively** in 0073 (after a defensive re-backfill — the data is already fully mirrored); keep `Principal.permissions` flat.

## 2. Current state (what moves)

**`platform_role_assignments (principal_type, principal_id, role_id)`** — platform-scope grants.
- Writers: `RoleRepo::assign`/`unassign` (`egret-auth/src/roles.rs`, the Access-control console); `Authorizer::replace_platform_roles` (`egret-workspace/src/rbac.rs`, user platform-roles edit); the bootstrap admin seed (`egret-bin/src/main.rs`).
- Readers: `SessionService::platform_roles` + `SessionService::permissions` (`egret-auth/src/session.rs`, → `Principal`); `RoleRepo::assignees_of`/`assignees_named`/`roles_of`/`count_platform_admins`; the bootstrap idempotency check.

**`workspace_members (workspace_id, principal_type, principal_id, role, added_by, added_at)`** — workspace-scope grants; `role` is a NAME string; PK `(workspace_id, principal_type, principal_id)` → one role per member.
- Writers: `MemberRepo::add`/`remove` (`egret-workspace/src/members.rs`).
- Readers: `Authorizer::workspace_roles_for` (`egret-workspace/src/rbac.rs`, WsContext display role); `MemberRepo::list`; `WorkspaceRepo::list_for_principal` (`egret-workspace/src/workspaces.rs`, "my workspaces").

**Triggers (0068/0069):** `sync_platform_grant_trg` (AFTER INSERT/DELETE on `platform_role_assignments`) and `sync_workspace_grant_trg` (AFTER INSERT/UPDATE/DELETE on `workspace_members`) mirror both tables into `access_grants`. Dropped by 0073.

**`access_grants`** is the destination: `(principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, granted_by, created_at)`, UNIQUE `(principal_type, principal_id, role_id, scope_type, scope_id, effect)`, `role_id` FK `ON DELETE CASCADE`. Platform grant = `scope_type='platform'`, `scope_id=NULL`. Workspace grant = `scope_type='workspace'`, `scope_id=workspace_id`, `role_id` = the workspace role's id.

## 3. Repoint the writers

**Platform:**
- `RoleRepo::assign(ptype, pid, role_id)` → `INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) VALUES ($1,$2,$3,'platform',NULL) ON CONFLICT DO NOTHING` (effect defaults `allow`). `unassign` → `DELETE FROM access_grants WHERE principal_type=$1 AND principal_id=$2 AND role_id=$3 AND scope_type='platform'`.
- `Authorizer::replace_platform_roles(user, names)` → in a tx: `DELETE FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND scope_type='platform'`, then for each validated name `INSERT ... SELECT 'user',$1,id,'platform',NULL FROM roles WHERE scope='platform' AND name=$2 ON CONFLICT DO NOTHING`.
- Bootstrap (`egret-bin`): `INSERT INTO access_grants ... SELECT 'user',$1,id,'platform',NULL FROM roles WHERE scope='platform' AND name='platform_admin'`.

**Workspace:**
- `MemberRepo::add(ws, ptype, pid, role_name, by)` → validate role_name (existing `WORKSPACE_ROLES` check); resolve to `role_id`; in a tx: `DELETE FROM access_grants WHERE principal_type=$ptype AND principal_id=$pid AND scope_type='workspace' AND scope_id=$ws` (one-role-per-member), then `INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, granted_by) VALUES ($ptype,$pid,$role_id,'workspace',$ws,$by) ON CONFLICT DO NOTHING`.
- `MemberRepo::remove(ws, ptype, pid)` → `DELETE FROM access_grants WHERE principal_type=$ptype AND principal_id=$pid AND scope_type='workspace' AND scope_id=$ws`.

## 4. Repoint the readers

All join `roles` for the role NAME where a name is needed. Group union (`principal_id IN (SELECT group_id FROM user_groups WHERE user_id=$1)`) is preserved exactly where it exists today.

- `SessionService::platform_roles` → `SELECT DISTINCT r.name FROM access_grants ag JOIN roles r ON r.id=ag.role_id WHERE ag.scope_type='platform' AND (<user-or-group principal>)`.
- `SessionService::permissions` → `SELECT DISTINCT rp.permission FROM access_grants ag JOIN role_permissions rp ON rp.role_id=ag.role_id WHERE ag.scope_type='platform' AND (<user-or-group principal>)`. **Flat/own-perms/platform-only, unchanged** — the role-hierarchy closure is NOT applied here (out of scope).
- `RoleRepo::assignees_of`/`assignees_named`/`roles_of` → `access_grants` with `scope_type='platform'` in place of `platform_role_assignments` (same joins to `users`/`groups`/`roles`).
- `RoleRepo::count_platform_admins` → `SELECT count(*) FROM access_grants ag JOIN roles r ON r.id=ag.role_id WHERE ag.scope_type='platform' AND r.name='platform_admin'`. **The last-platform-admin guard is preserved on the new source.**
- `Authorizer::workspace_roles_for(user, ws)` → `SELECT DISTINCT r.name FROM access_grants ag JOIN roles r ON r.id=ag.role_id WHERE ag.scope_type='workspace' AND ag.scope_id=$ws AND (<user-or-group principal>)`.
- `MemberRepo::list(ws)` → `SELECT ag.principal_type, ag.principal_id, r.name AS role FROM access_grants ag JOIN roles r ON r.id=ag.role_id WHERE ag.scope_type='workspace' AND ag.scope_id=$ws`.
- `WorkspaceRepo::list_for_principal(user)` (non-admin branch) → `JOIN access_grants ag ON ag.scope_id=w.id AND ag.scope_type='workspace'` in place of `workspace_members wm`, with the same user-or-group predicate.

## 5. Migration 0073 (drop)

```
-- Defensive final re-backfill (self-heal any historical drift), then drop the
-- sync triggers + functions, then the two legacy tables.
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT principal_type, principal_id, role_id, 'platform', NULL FROM platform_role_assignments
  ON CONFLICT DO NOTHING;
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT wm.principal_type, wm.principal_id, r.id, 'workspace', wm.workspace_id
  FROM workspace_members wm JOIN roles r ON r.scope='workspace' AND r.name=wm.role
  ON CONFLICT DO NOTHING;
DROP TRIGGER sync_platform_grant_trg ON platform_role_assignments;
DROP TRIGGER sync_workspace_grant_trg ON workspace_members;
DROP FUNCTION sync_platform_grant();
DROP FUNCTION sync_workspace_grant();
DROP TABLE platform_role_assignments;
DROP TABLE workspace_members;
```

`touch crates/egret-metastore/src/lib.rs` after adding it (sqlx re-embed gotcha). No orphan concern: workspaces are archive-only (never hard-deleted), and `access_grants.role_id` already cascades on role delete.

## 6. WsContext / object-guest interplay

The `WsContext` extractor already admits object-guests via `has_object_grant_in_workspace` (reads `access_grants`, filters `effect='allow'`) and resolves a member's display role via `workspace_roles_for`. After the flip, `workspace_roles_for` reads `access_grants` workspace grants: a member resolves their workspace role; an object-guest holds no workspace grant → empty → the existing `object_guest` sentinel path. The member-vs-object-guest distinction is unchanged.

## 7. Testing

- **Behavior-preserving:** the existing `egret-auth` / `egret-workspace` / `egret-controlplane` suites pass. Tests that seed or read `platform_role_assignments` / `workspace_members` directly are repointed to `access_grants` (or to the public repo methods) — these edits are the bulk of the churn.
- **Writers:** `assign`/`unassign` write/remove a platform grant in `access_grants`; `replace_platform_roles` replaces the set; `MemberRepo::add` inserts a workspace grant AND removes the principal's prior workspace grant for that ws (one-role-per-member — assert a re-add with a different role leaves exactly one row); `remove` deletes it.
- **Readers:** `platform_roles`/`permissions` derive from `access_grants`; `count_platform_admins` counts platform_admin grants there (assert the last-admin guard still fires); `workspace_roles_for` + `MemberRepo::list` read workspace grants; `list_for_principal` lists via `access_grants`.
- **Migration:** a test asserting `platform_role_assignments` and `workspace_members` no longer exist after `migrate()`, and that pre-existing grants survive in `access_grants`.
- **PDP parity:** the B1a regression matrix + object-scope + B2-3 suites pass unchanged (the flip does not change `access_grants` contents).
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build` (FE is unaffected — members/roles DTOs are unchanged; regen only if openapi moves, which it should not), openapi current, adversarial security review.

## 8. Security & safety

- **No new permission surface** — signatures and semantics preserved; the flip removes the dual-write drift risk (single source of truth).
- **No grant lost** — the defensive re-backfill in 0073 runs before the drops; the data was already trigger-mirrored since 0068.
- **One-role-per-member preserved** — `add` deletes-then-inserts in a tx, so a member never accumulates multiple workspace roles unintentionally (matching the old PK).
- **Last-platform-admin guard preserved** — `count_platform_admins` now counts `access_grants` platform_admin grants; the guard that blocks removing the final admin is unchanged in behavior.
- **Bootstrap idempotency preserved** — the "a platform_admin already exists" check reads `access_grants`.
- **Transactional writes** — `replace_platform_roles` and `MemberRepo::add` do their delete+insert in one transaction so a concurrent read never sees an empty interim state.

---

*Design v1.0 (2026-07-11). Makes `access_grants` the single grant store: repoint every platform/workspace grant writer and reader off the two legacy tables, defensively re-backfill, and drop the 0068/0069 sync triggers + `platform_role_assignments` + `workspace_members`. Behavior-preserving — PDP parity unchanged; `Principal.permissions` stays flat.*
