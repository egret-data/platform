# Platform RBAC & Access-Control Admin — Design Spec

**Status:** Design (approved to proceed to plan)
**Date:** 2026-07-07
**Branch:** `feat/platform-rbac-admin`
**Vietnamese reference:** `docs/vi/specs/2026-07-07-platform-rbac-admin-design.md`

## 1. Goal

Give egret a first-class **Access Control** admin surface and turn its already-present
(but unenforced) RBAC schema into real, fine-grained authorization. Today the
control plane has exactly one gate — `require_platform_admin` — and the **Users**
menu is a placeholder. This epic delivers:

1. A **code-defined permission catalog** (`resource:action`) that the backend enforces.
2. **Fine-grained enforcement**: every control-plane mutation checks a specific
   permission instead of the coarse `platform_admin` flag.
3. An **admin console** to manage Users, Groups, Roles, and Permissions.

Scope is **platform**. Workspace-member management (assigning workspace roles inside
a given workspace) is an explicit follow-up (see §11).

## 2. Existing model (reuse, do not rebuild)

`migrations/0002_identity.sql` + `migrations/0003_workspace_rbac.sql` already define:

- `users` — `provider` (`local`/`oidc`), `email` (citext, unique), `name`, `password_hash`, `status` (`active`/`disabled`), `last_login_at`.
- `groups` — `name` (unique), `source` (`local`/`oidc`); `user_groups` (m:n).
  OIDC groups are synced at login (`GroupRepo::sync_user_oidc_groups`, source `oidc`);
  local groups are admin-managed.
- `roles` — `scope` (`platform`/`workspace`), `name`, `description`; unique `(scope,name)`.
  Seeded: `platform_admin`, `data_steward`, `ws_admin`, `ws_editor`, `ws_viewer`.
- `role_permissions` — `(role_id, permission)`; permission is a `resource:action` string.
- `platform_role_assignments` — `(principal_type user|group, principal_id, role_id)`.
- `workspace_members` — `(workspace_id, principal_type, principal_id, role)` (workspace scope).

`Principal` already carries `platform_roles: Vec<String>` resolved in
`session.rs::platform_roles(user_id)`. This epic **adds** resolved permissions and
extends the CRUD/enforcement around the above.

## 3. Design decisions (confirmed)

| Decision | Choice |
|---|---|
| Permission model | **Code-defined** — backend owns the catalog + enforces; admins assign permissions to roles, cannot invent new permission strings. |
| Enforcement depth | **Fine-grained now** — replace `require_platform_admin` with `require_permission(p, "…")` per route. |
| Groups | **Local + OIDC** — admins create/edit local groups & memberships; OIDC groups read-only but role-assignable. |
| Scope | **Platform** this epic; workspace-member management is a follow-up. |

## 4. Permission catalog (code-defined)

Single source of truth in the backend (new `egret-core::rbac` module):

```rust
pub struct Permission { pub key: &'static str, pub group: &'static str, pub description: &'static str }
pub static PERMISSIONS: &[Permission] = &[ /* … */ ];
```

Catalog (`resource:action`), grouped by area:

| Area | Permissions |
|---|---|
| Engines | `engine:read` `engine:create` `engine:update` `engine:delete` `engine:provision` |
| Warehouses (catalog) | `warehouse:read` `warehouse:create` `warehouse:update` `warehouse:delete` |
| Storage | `storage:read` `storage:create` `storage:update` `storage:delete` |
| Secrets | `secret:read` `secret:create` `secret:update` `secret:delete` |
| Databases | `database:read` `database:create` `database:update` `database:delete` |
| Policies (Ranger) | `policy:read` `policy:create` `policy:update` `policy:delete` `policy:seed` |
| Workspaces | `workspace:read` `workspace:create` `workspace:update` `workspace:archive` |
| Users | `user:read` `user:create` `user:update` `user:disable` `user:assign_role` |
| Groups | `group:read` `group:create` `group:update` `group:delete` `group:manage_members` |
| Roles | `role:read` `role:create` `role:update` `role:delete` `role:assign_permission` |

- **Startup validation**: on boot, warn if any `role_permissions.permission` is not in the catalog (drift detection). Do not delete — just log.
- **`GET /control-plane/permissions`** returns the catalog (key/group/description) + which roles grant each, for the Permissions reference page.
- The catalog grows as new features ship; adding a permission = one entry + guarding the new route.

### Default role → permission seed (update `0003` follow-on migration)

- `platform_admin` → **superuser via the §5 bypass** (no per-permission rows needed; it always passes `require_permission`, so the catalog can grow without re-seeding it).
- `data_steward` → `policy:*`, `warehouse:read`, `database:read`, `storage:read`, `user:read`, `group:read`, `role:read`.
- Workspace roles (`ws_*`) keep their workspace-scoped perms (unchanged this epic).

## 5. Enforcement

- **Resolution**: extend `session.rs` to compute `permissions: HashSet<String>` for the
  authenticated user = union of `role_permissions` over every role assigned to the user
  **directly** or via **any of the user's groups** (`platform_role_assignments`
  where `principal` = the user OR one of their group ids). Store on `Principal`.
- **`platform_admin` = superuser**: holds every permission. Implement as either
  (a) seeding all catalog keys into its `role_permissions`, or (b) a bypass in
  `require_permission` when the principal has the `platform_admin` role. Choose **(b)**
  (bypass) so the catalog can grow without a migration each time; `platform_admin`
  always passes.
- **Guard**: `require_permission(&Principal, perm: &str) -> Result<(), CpError::Forbidden>`
  in `egret-controlplane::auth`. Every control-plane route swaps its
  `require_platform_admin(&p)?` for the specific `require_permission(&p, "engine:create")?`.
  A route → permission mapping table lives in the plan (one row per endpoint).
- `require_platform_admin` is kept only for genuinely platform-admin-only actions
  (e.g. `role:*`, `user:assign_role`) or removed once every caller is migrated.

## 6. Backend APIs (new `access` module in egret-controlplane)

All under `/control-plane`, guarded by the matching permission:

- **Users**: `GET /users` (list, +roles/groups), `POST /users` (local), `PATCH /users/{id}` (name/status), `POST /users/{id}/roles` / `DELETE …/roles/{roleId}` (assign/unassign platform role).
- **Groups**: `GET /groups` (list, +source/members/roles), `POST /groups` (local), `PATCH /groups/{id}`, `DELETE /groups/{id}` (local only), `POST /groups/{id}/members` / `DELETE …/members/{userId}` (local only), `POST /groups/{id}/roles` / `DELETE …/roles/{roleId}`.
- **Roles**: `GET /roles` (list, +#perms/#assignees; note: `/policies` already exposes a Ranger `/roles` — this is the RBAC roles endpoint, mount as `/access/roles` to avoid collision), `POST`, `PATCH`, `DELETE` (custom only; seeded roles non-deletable), `PUT /access/roles/{id}/permissions` (set the role's permission list, validated against the catalog).
- **Permissions**: `GET /permissions` (catalog + role coverage).
- Reuse existing `UserRepo`/`GroupRepo`; add a `RoleRepo` (roles, role_permissions, platform_role_assignments) in egret-auth.

## 7. Admin console (frontend)

New route group **Access control** in the Control Plane sidebar (collapsible group,
replacing the placeholder `Users`), following egret control-plane conventions
(ConfigSection, `COLUMNS` tables, ConfirmDeleteDialog, inline Alert, shadcn tokens):

- **Users** — table: email · name · provider · status · roles. Actions: create local user, edit (name/status), disable, **assign roles** (multiselect from roles), view group membership.
- **Groups** — table: name · source (`oidc`/`local` badge) · members · roles. Actions: create local group, edit, delete (local only), **manage members** (add/remove users; disabled for oidc), **assign roles**.
- **Roles** — table: scope · name · description · #permissions · #assignees. Actions: create/edit/delete custom roles (seeded roles read-only name/scope), **edit permissions** (grouped checklist from the catalog), **assignees** (users/groups holding this role).
- **Permissions** — read-only reference: catalog grouped by area, each showing description + the roles that grant it. No create/edit.

## 8. Menu IA

Group `Users`, `Groups`, `Roles`, `Permissions` under one collapsible **Access control**
section in the Control Plane sidebar (avoids a flat list of items; consistent with the
existing grouping feedback). The old placeholder `Users` route is replaced.

## 9. Safety / lock-out prevention

- `platform_admin` bypass guarantees a super-admin always retains full access.
- **Last-admin guard**: refuse to remove the final `platform_admin` assignment
  (user or group) — return a clear 409 with a message.
- The bootstrap admin user keeps `platform_admin`.
- Seeded roles (`platform_admin`, `data_steward`, `ws_*`) cannot be deleted; their
  name/scope are immutable (permissions of non-`platform_admin` seeded roles are editable).

## 10. Testing

- Backend unit: catalog integrity (no dup keys), permission resolution (user direct +
  via group), `require_permission` allow/deny + `platform_admin` bypass, last-admin guard,
  seeded-role immutability. DB-gated store tests for RoleRepo/assignments.
- Route tests: representative endpoints return 403 without the permission, 200 with it.
- Frontend: Vitest for each page (list render, create/edit/delete, assign flows).
- Full local CI (fmt/clippy/workspace tests on `egret_test` + frontend lint/typecheck/test/build) before merge.

## 11. Out of scope (follow-ups)

- **Workspace-member management** (assign `ws_*` roles inside a workspace) — the
  `workspace_members` table + a per-workspace Members UI.
- SCIM / automated user provisioning.
- Per-permission audit log (who granted what, when) beyond existing `added_by`/`added_at`.
- Custom **workspace-scoped** permission enforcement (this epic enforces platform perms).

## 12. Phasing (spec → plan → subagent-driven)

- **P1 — Enforcement core**: `egret-core::rbac` catalog + startup validation;
  `Principal.permissions` resolution in session; `require_permission` guard +
  `platform_admin` bypass; migrate every control-plane route from
  `require_platform_admin` to a specific permission (route→perm table); default
  role→permission seed migration. Backend tests.
- **P2 — Access APIs**: `RoleRepo` + the `access` module CRUD/assignment endpoints
  (users/groups/roles/permissions) with last-admin + immutability guards. Route tests.
- **P3 — Admin console**: the four frontend pages + the grouped sidebar section +
  api client + Vitest. Full CI + browser smoke.
