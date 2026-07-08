# Platform RBAC P3 — Admin Console — Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** The frontend **Access control** admin: Users, Groups, Roles, and Permissions pages on the P2 APIs, grouped in the Control-Plane sidebar.

**Architecture:** One api client (`src/api/access.ts`); a new "Access control" sidebar group (Users/Groups/Roles/Permissions) replacing the placeholder Users item; four route pages following egret control-plane conventions.

**Tech:** React 18 + TanStack Query + shadcn/ui. Conventions to mirror: `src/routes/control-plane/Policies.tsx` + `ProfileDetailPage.tsx` (AppShell padding, `const COLUMNS`, sr-only Actions, `ConfirmDeleteDialog`, inline `Alert`, `Switch`, `Badge`, shadcn tokens), `src/components/ui/tag-combobox.tsx` (multi-select), `src/components/config/ConfigSection.tsx`.

**Spec:** `docs/en/specs/2026-07-07-platform-rbac-admin-design.md`. **Backend (P2):** `GET/POST /users`, `PATCH /users/{id}`, `POST/DELETE /users/{id}/roles(/{roleId})`, `GET/POST /groups`, `PATCH/DELETE /groups/{id}`, `POST/DELETE /groups/{id}/members(/{userId})`, `POST/DELETE /groups/{id}/roles(/{roleId})`, `GET/POST /access/roles`, `PATCH/DELETE /access/roles/{id}`, `PUT /access/roles/{id}/permissions`, `GET /permissions` — all under `/api/control-plane`.

---

### Task 1: api client + sidebar group + routes + Permissions page

**Files:** Create `src/api/access.ts`; modify `src/components/layout/Sidebar.tsx` + its test; modify `src/App.tsx`; create `src/routes/control-plane/access/PermissionsPage.tsx` + test.

- **`api/access.ts`** — use the existing api helper (see how `src/api/policies.ts` does `apiJson`/GET/POST/etc.). Types: `AccessUser {id,email,name,provider,status,roles:string[],groups:string[]}`, `AccessGroup {id,name,source,memberCount,roles:string[]}`, `Role {id,scope,name,description,permissionCount,assigneeCount}`, `PermissionGroup {group,permissions:{key,description,roles:string[]}[]}`. Functions for every endpoint: `listUsers/createUser/updateUser/assignUserRole/unassignUserRole`, `listGroups/createGroup/renameGroup/deleteGroup/addMember/removeMember/assignGroupRole/unassignGroupRole`, `listRoles/createRole/updateRole/deleteRole/setRolePermissions`, `listPermissions`. (Camel↔JSON: match the backend field casing — verify against the P2 DTOs; if the backend returns snake_case, type accordingly.)
- **Sidebar** — add a new group `{ heading: "Access control", items: [Users, Groups, Roles, Permissions] }` to `CONTROL_PLANE_GROUPS` (icons from lucide: `Users`, `UsersRound`/`Users2`, `Shield`/`UserCog`, `KeySquare`/`ListChecks`). REMOVE the `Users` item from the existing "Control Plane" group (it moves to the new section). Keep it collapsible like the others. Update `src/components/layout/__tests__/Sidebar.test.tsx` if it asserts on the Users item's location/count.
- **App.tsx** — replace `<Route path="users" element={<Placeholder title="Users" />} />` with the real pages:
  `users → UsersPage`, `groups → GroupsPage`, `roles → RolesPage`, `permissions → PermissionsPage` (import all four; the other three are built in later tasks — for THIS task, add the routes but you may stub Users/Groups/Roles as a temporary `<Placeholder>` so the app compiles, then later tasks replace the stubs. Actually: create the three page files as minimal stubs now and flesh them out in Tasks 2-3.).
- **PermissionsPage** (read-only reference) — `useQuery(listPermissions)`; render each `PermissionGroup` as a `ConfigSection`/card titled by `group`, a table of `key · description · roles` (roles as `Badge`s; "—" if none). This is the simplest page; build it fully here. Vitest: renders groups + a permission's roles from a mocked `listPermissions`.
- Commit `feat(frontend): access api client + Access-control sidebar group + Permissions page`.

### Task 2: Users page + Groups page

**Files:** `src/routes/control-plane/access/UsersPage.tsx`, `GroupsPage.tsx` (+ tests).

- **UsersPage** — `COLUMNS = [Email, Name, Provider, Status, Roles, (Actions)]`. Table via `listUsers`. Row: status `Badge` (active/disabled), roles as `Badge`s, groups maybe in a tooltip or a column. Actions: **Edit** (name; toggle status active/disabled → `updateUser`, with a confirm when disabling), **Assign roles** (a dialog: `TagCombobox`/multiselect of `listRoles` names → `assignUserRole`/`unassignUserRole` diffing current vs selected). **New user** button → dialog `{email,name,password}` → `createUser`. Handle the last-admin 409 (inline `Alert` on the assign dialog). Invalidate `["control-plane","access","users"]`.
- **GroupsPage** — `COLUMNS = [Name, Source, Members, Roles, (Actions)]`. `listGroups`. Source `Badge` (`local`/`oidc`). Actions: **New group** (local) → `createGroup`; **Edit/rename** (local only — disable for oidc; 409 → inline Alert); **Manage members** dialog (add/remove users via `TagCombobox` of `listUsers`; disabled for oidc); **Assign roles** dialog; **Delete** (local only) via `ConfirmDeleteDialog` (409 → Alert). oidc rows show a muted "synced" hint and disabled mutation controls.
- Vitest each: list render; create flow calls the api; role-assign diff calls assign/unassign; oidc group's delete/rename disabled.
- Commit `feat(frontend): Users + Groups admin pages`.

### Task 3: Roles page

**Files:** `src/routes/control-plane/access/RolesPage.tsx` (+ test).

- `COLUMNS = [Scope, Name, Description, Permissions, Assignees, (Actions)]`. `listRoles` — show `permissionCount`/`assigneeCount` (badges). `scope` badge (platform/workspace). Seeded roles (name in `platform_admin|data_steward|ws_admin|ws_editor|ws_viewer`) show a "built-in" badge and have **Delete disabled**.
- **New role** → dialog `{scope, name, description}` → `createRole`.
- **Edit** → dialog: description (editable), name/scope read-only; a **permissions checklist** grouped by area (fetch the catalog via `listPermissions` → group → `Checkbox` per key, prefilled from the role's current permissions; `platform_admin` shows a note "superuser — bypasses all checks" and the checklist can be read-only) → `setRolePermissions` (400 on unknown → inline Alert, shouldn't happen from the checklist). Optionally show assignees (users/groups holding the role).
- **Delete** custom role via `ConfirmDeleteDialog` (seeded → 409 guarded server-side + disabled client-side).
- Vitest: list render (seeded badge + delete disabled); create flow; permissions checklist prefill + save calls `setRolePermissions` with the checked keys.
- Commit `feat(frontend): Roles admin page (permission checklist + assignees)`.

### Task 4: Gate + browser smoke

- `npm run lint && npm run typecheck && npm run test && npm run build` — all pass.
- Rebuild `serve` (backend already has P1+P2); browser smoke as `admin@egret.local`: Access control section shows 4 pages; create a local group; create a custom role + set permissions; assign a role to a user; Permissions page lists the catalog; last-admin removal blocked with a clear message. 0 console errors.
- Commit any fixes. P3 (and the epic) ready for local CI → PR.

## Notes
- Field casing: confirm the P2 DTO JSON casing (snake vs camel) and type `access.ts` to match — do not assume.
- Keep dialogs/tables consistent with `ProfileDetailPage.tsx` (same `ConfirmDeleteDialog`, `Alert` destructive, `Switch`, `COLUMNS` + sr-only Actions).
- Query keys under `["control-plane","access", …]`; invalidate on every mutation.
