# Workspace → Access tab (B1 read-only roles + shared-objects roll-up) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give workspace admins a self-contained **Access** area inside their own workspace — the existing Members page, plus a read-only **Roles & permissions** view (what each workspace role can do) and a **Shared objects** roll-up (every object grant in the workspace and who holds it) — without exposing or requiring the platform-level Control-Plane access console.

**Architecture:** Keep authorization *definitions* centralized in the Control Plane (workspace roles remain global templates — see Non-Goals), and surface *workspace-local, read-oriented* views under `/explore/:workspace/settings`. Two new **workspace-scoped, `WsContext`-gated** read endpoints feed the two new tabs; both reuse the existing `access_grants` store and the `WsContext` + `egret_core::authz::require` admission pattern already used by the members router. No new authz mechanisms, no data-model change.

**Tech Stack:** Rust (axum, sqlx, utoipa) in `crates/egret-workspace`; React 18 + TanStack Query + shadcn/Tailwind in `frontend/src`; orval-generated client; Vitest for FE, `cargo test` for BE.

---

## Context & Rationale

This implements decision **B1** from the 2026-07-13 access-control review:

- **Assignment** of workspace roles and object shares is a workspace concern → already lives at `/explore/:workspace/settings/members` (AX-1) and per-object `ShareDialog`.
- **Definition** of what a role grants stays central (Control Plane → Roles), because workspace roles are **global templates** (`WORKSPACE_ROLES = [ws_admin, ws_editor, ws_viewer]`, no `workspace_id` on the role row). Editing one changes it for every workspace, so a ws_admin must not edit it — but they *should* be able to *see* it.

So this increment adds **read-only** workspace surfaces only. Truly per-workspace custom roles (add `roles.workspace_id`, let ws_admins define local roles) is **B2**, out of scope here and called out in Non-Goals.

### What already exists (do not rebuild)

- `frontend/src/routes/explore/settings/WorkspaceSettings.tsx` — the settings layout (renders child routes; ~1 KB).
- `frontend/src/routes/explore/settings/Members.tsx` — Members tab (keep as-is).
- `crates/egret-workspace/src/routes.rs` — `ws_member_router()`, `WorkspaceState`, `WsContext`, the `list_members` handler pattern (auth via `egret_core::authz::require(&*st.pdp, &ctx, "…")`), and `MemberDto`.
- `migrations/0068_access_grants.sql` — `access_grants(principal_type, principal_id, role_id → roles(id), scope_type ∈ {platform,workspace,object}, scope_id, object_type, …)`; `effect` column added by the B2-3 migration.
- `frontend/src/components/explore/FilterableTable.tsx`, `ListSkeleton.tsx`, `FacetSelect.tsx`, `SummaryStrip` — shared table primitives (used by Members / Users / Roles).
- `frontend/src/components/sharing/ShareDialog.tsx` — per-object share editor (the roll-up links into it).
- Per-object grant read endpoint `GET /api/ws/{workspace}/objects/{kind}/{id}/grants` (`sharesGrantList`) — per-object only; there is **no** workspace-wide roll-up yet.

---

## File Structure

**Backend (`crates/egret-workspace/src`)**
- Modify `routes.rs` — add two handlers + DTOs + extend `ws_member_router()` (or add a sibling `ws_access_router()`), and register both in the `WorkspaceMembersApi` OpenAPI fragment.
- Modify `members.rs` (the `MemberRepo` module) **or** add `access_views.rs` — add two read queries: `list_workspace_roles_with_perms()` and `list_object_grants(workspace_id)`. Prefer a new `access_views.rs` module so the members repo keeps one responsibility.
- Modify `lib.rs` — `mod access_views;` if the new module is added; export any new repo in `WorkspaceState` wiring (`crates/egret-server` builds `WorkspaceState`).

**Frontend (`frontend/src`)**
- Modify `routes/explore/settings/WorkspaceSettings.tsx` — add a tab nav (Members · Roles & permissions · Shared objects).
- Modify `App.tsx` — add two child routes under `settings`.
- Create `routes/explore/settings/RolesPermissions.tsx` — read-only role × permission view.
- Create `routes/explore/settings/SharedObjects.tsx` — object-grant roll-up (FilterableTable) with a row action opening `ShareDialog`.
- Create `api/workspaceAccess.ts` — two react-query hooks (`useWorkspaceRoleCatalog`, `useWorkspaceShares`) calling the new endpoints via `apiJson` (same convention as `api/members.ts`).
- Create tests: `routes/explore/settings/__tests__/RolesPermissions.test.tsx`, `SharedObjects.test.tsx`, and extend `WorkspaceSettings` nav test if present.

---

## Endpoint Contracts

### BE-1 · `GET /api/ws/{workspace}/roles`

Workspace-scope role catalog **with resolved permissions**, so a ws_admin can see what each role grants without `role:read` on the Control Plane.

- **Auth:** `WsContext` admission (caller is a workspace member) + `require(&*st.pdp, &ctx, "workspace:manage_members")` — same gate as Members (admins), so the tab matches who can already manage members. (If product wants any member to view, relax to admission-only; decide in Task 1, keep it admin-gated by default.)
- **Query:** `roles` where `scope = 'workspace'`, left-joined to the role→permission mapping. Reuse whatever table `controlPlaneAccessSetRolePermissions` / `getRole` read/write (confirm the table name in Task 1 by reading the CP `set_role_permissions` handler in `crates/egret-controlplane/src/access/`). Include inherited permissions is **not** required here (parity with the CP Roles editor, which edits direct perms); show direct perms only and note inheritance via `parent_role_id`.
- **Response `200 body = Vec<WsRoleDto>`:**

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct WsRoleDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parent_role_id: Option<String>,
    pub permissions: Vec<WsPermDto>, // direct permissions this role grants
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct WsPermDto {
    pub key: String,         // e.g. "chart:read"
    pub description: String,
    pub group: String,       // catalog area, for grouping in the UI
}
```

### BE-2 · `GET /api/ws/{workspace}/shares`

Roll-up of every **object** grant whose object lives in this workspace.

- **Auth:** `WsContext` + `require(…, "workspace:manage_members")`.
- **Query:** `access_grants ag JOIN roles r ON r.id = ag.role_id WHERE ag.scope_type = 'object'` **and the object belongs to `ctx.workspace_id`**. Object→workspace resolution is by `object_type`: charts/dashboards/notebooks/datasets/saved_queries/folders each carry `workspace_id`. Confirm in Task 4 whether the BI layer already exposes an object→workspace resolver used by `require_object` admission and reuse it; otherwise build the roll-up as a `UNION ALL` over the object tables filtered by `workspace_id`, joined back to `access_grants` on `(object_type, scope_id)`. Enrich principal + role name like `MemberDto` does.
- **Response `200 body = Vec<ObjectGrantRollupDto>`:**

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct ObjectGrantRollupDto {
    pub kind: String,               // chart | dashboard | notebook | dataset | saved_query | folder
    pub object_id: String,
    pub object_name: Option<String>,
    pub principal_type: String,     // user | group
    pub principal_id: String,
    pub principal_name: Option<String>,
    pub role: String,               // granted role name (e.g. ws_viewer)
    pub effect: String,             // allow | deny
}
```

---

## Non-Goals (explicit)

- **No** editing of role permissions inside the workspace (that stays in Control Plane; workspace view is read-only). Editing a global template from a workspace context is the trap this plan avoids.
- **No** `roles.workspace_id` / per-workspace custom roles (**B2**, separate plan).
- **No** change to object sharing itself — `SharedObjects` links into the existing `ShareDialog` for edits; it does not add a new grant-write path.
- **No** access-request / approval workflow (future).

---

## Tasks

> TDD throughout. Backend tests use the existing `crates/egret-workspace` test harness (an integration DB via `DATABASE_URL`, following `members` tests). Frontend tests use Vitest + Testing Library with `vi.mock` of the api module, following `Members.test.tsx`.

### Task 1: Confirm the role→permission read path (spike, no code)

**Files:** read-only — `crates/egret-controlplane/src/access/` (the `set_role_permissions` + `get_role`/`list_permissions` handlers), `migrations/0068_access_grants.sql` and the migration that adds the role→permission table.

- [ ] **Step 1:** Read the CP access handlers to find the exact table + columns that store a role's direct permissions (name it in a comment at the top of the new `access_views.rs`). Confirm the `roles` columns: `id, name, description, scope, parent_role_id`.
- [ ] **Step 2:** Confirm the workspace-role gate. Decide `workspace:manage_members` (admin-only, default) vs admission-only (any member). Record the decision in the plan check-in.
- [ ] **Step 3:** No commit (spike). Capture findings inline in Task 2/4 code before writing them.

### Task 2: BE-1 query — `list_workspace_roles_with_perms`

**Files:**
- Create: `crates/egret-workspace/src/access_views.rs`
- Modify: `crates/egret-workspace/src/lib.rs` (add `mod access_views;`)
- Test: add `#[sqlx::test]` (or the crate's existing integration-test module) covering the query.

- [ ] **Step 1: Write the failing test.** Seed a workspace-scope role with 2 permissions; assert the query returns the role with both permission keys and its `group`.

```rust
#[tokio::test]
async fn workspace_roles_include_direct_permissions() {
    let db = test_db().await; // follow the members test's harness
    let repo = AccessViewsRepo::new(db.clone());
    // seed: a workspace-scope role "ws_editor" with perms "chart:read","chart:write"
    seed_ws_role_with_perms(&db, "ws_editor", &["chart:read", "chart:write"]).await;

    let roles = repo.list_workspace_roles_with_perms().await.unwrap();
    let editor = roles.iter().find(|r| r.name == "ws_editor").unwrap();
    let keys: Vec<_> = editor.permissions.iter().map(|p| p.key.as_str()).collect();
    assert!(keys.contains(&"chart:read"));
    assert!(keys.contains(&"chart:write"));
}
```

- [ ] **Step 2: Run it, verify it fails** (type/methods absent).
  Run: `cargo test -p egret-workspace workspace_roles_include_direct_permissions`
  Expected: FAIL (unresolved `AccessViewsRepo`).

- [ ] **Step 3: Implement `AccessViewsRepo::list_workspace_roles_with_perms`** using the table confirmed in Task 1. Shape (adjust join table name to Task 1 finding):

```rust
pub struct AccessViewsRepo { db: Db }

impl AccessViewsRepo {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn list_workspace_roles_with_perms(&self) -> egret_core::Result<Vec<WsRole>> {
        // roles (scope='workspace') LEFT JOIN role_permissions rp
        //   JOIN the code-defined permission catalog for description + group.
        // Group rows by role in Rust. Order roles by name, perms by key.
        // ... sqlx::query_as rows, then fold into Vec<WsRole> ...
    }
}
```

- [ ] **Step 4: Run the test, verify PASS.**
  Run: `cargo test -p egret-workspace workspace_roles_include_direct_permissions`
- [ ] **Step 5: Commit.** `feat(workspace): read workspace-role catalog with permissions`

### Task 3: BE-1 handler + route + OpenAPI

**Files:** Modify `crates/egret-workspace/src/routes.rs`; wire `AccessViewsRepo` into `WorkspaceState` (and its construction in `crates/egret-server`).

- [ ] **Step 1: Write the failing handler test** (integration, following the members router test): a workspace admin GETs `/api/ws/{ws}/roles` → 200 with the seeded role; a non-member → 403.
- [ ] **Step 2: Run, verify fail** (route not mounted → 404).
- [ ] **Step 3: Implement** `list_workspace_roles` mirroring `list_members`:

```rust
#[utoipa::path(get, path = "/api/ws/{workspace}/roles", tag = "workspace-members",
  params(WorkspacePath),
  responses((status = 200, body = Vec<WsRoleDto>), (status = 401, body = ErrorBody),
            (status = 403, body = ErrorBody), (status = 500, body = ErrorBody)))]
async fn list_workspace_roles(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<WsRoleDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await.map_err(AuthError)?;
    let roles = st.access_views.list_workspace_roles_with_perms().await.map_err(AuthError)?;
    Ok(Json(roles.into_iter().map(WsRoleDto::from).collect()))
}
```

  Add `.route("/{workspace}/roles", get(list_workspace_roles))` to the router, register `list_workspace_roles` + `WsRoleDto` + `WsPermDto` in the `WorkspaceMembersApi` `#[openapi(...)]`.
- [ ] **Step 4: Run, verify PASS** (both the 200 and 403 cases).
- [ ] **Step 5: Commit.** `feat(workspace): GET /ws/{ws}/roles endpoint`

### Task 4: BE-2 query + handler — shared-objects roll-up

**Files:** Modify `access_views.rs`, `routes.rs`.

- [ ] **Step 1: Spike (read-only)** — find the object→workspace resolver used by `require_object`/`WsContext` object admission (`crates/egret-workspace/src/rbac.rs` + BI object handlers). Reuse it; note the source in a comment.
- [ ] **Step 2: Write the failing test** — seed a chart in workspace W shared with a group as `ws_viewer` (allow) and a user denied; assert `list_object_grants(W)` returns both rows with correct `kind`, `role`, `effect`, and principal name; a grant on an object in a *different* workspace is **excluded** (cross-tenant guard).
- [ ] **Step 3: Run, verify fail.**
- [ ] **Step 4: Implement `list_object_grants(workspace_id)`** — `access_grants` (`scope_type='object'`) joined to workspace objects (reuse resolver or `UNION ALL` over object tables filtered by `workspace_id`), joined to `roles` for the role name and to principals for names. Return `effect` (default `'allow'` if the column is null on legacy rows).
- [ ] **Step 5: Implement handler** `list_workspace_shares` (same auth + shape as Task 3), route `.route("/{workspace}/shares", get(list_workspace_shares))`, register in OpenAPI.
- [ ] **Step 6: Run, verify PASS** (including the cross-workspace exclusion — this is the security-critical assertion; do not skip).
- [ ] **Step 7: Commit.** `feat(workspace): GET /ws/{ws}/shares object-grant roll-up`

### Task 5: Regenerate the API client

**Files:** `frontend/src/api/generated/**` (orval output).

- [ ] **Step 1:** Rebuild the OpenAPI spec and run orval (follow the repo's existing codegen command — check `package.json`/`justfile` for the `orval` / `openapi` script). Confirm new `workspace-members` operations appear.
- [ ] **Step 2:** Typecheck. Run: `cd frontend && npm run typecheck`. Expected: PASS.
- [ ] **Step 3: Commit.** `chore(api): regenerate client for ws roles + shares`

> Note: `api/members.ts` calls `apiJson` directly (not the orval client), so `api/workspaceAccess.ts` (Task 6) may do the same and this task becomes optional. If you keep `apiJson`, skip client regen and just ensure the OpenAPI still builds.

### Task 6: FE api hooks

**Files:** Create `frontend/src/api/workspaceAccess.ts`.

- [ ] **Step 1: Write the module** (mirror `api/members.ts` — `apiJson`, paths without `/api` prefix):

```ts
import { useQuery } from "@tanstack/react-query";
import { apiJson } from "./client";

export interface WsPerm { key: string; description: string; group: string }
export interface WsRole {
  id: string; name: string; description: string;
  parent_role_id: string | null; permissions: WsPerm[];
}
export interface ObjectGrantRollup {
  kind: string; object_id: string; object_name: string | null;
  principal_type: "user" | "group"; principal_id: string;
  principal_name: string | null; role: string; effect: string;
}

export function useWorkspaceRoleCatalog(ws: string) {
  return useQuery<WsRole[]>({
    queryKey: ["ws", ws, "role-catalog"],
    queryFn: () => apiJson<WsRole[]>(`/ws/${ws}/roles`),
    retry: false,
  });
}

export function useWorkspaceShares(ws: string) {
  return useQuery<ObjectGrantRollup[]>({
    queryKey: ["ws", ws, "shares"],
    queryFn: () => apiJson<ObjectGrantRollup[]>(`/ws/${ws}/shares`),
    retry: false,
  });
}
```

- [ ] **Step 2:** Typecheck. Run: `cd frontend && npm run typecheck`. Expected: PASS.
- [ ] **Step 3: Commit.** `feat(fe): workspace access api hooks`

### Task 7: Settings tab navigation

**Files:** Modify `WorkspaceSettings.tsx`, `App.tsx`.

- [ ] **Step 1: Write the failing nav test** — render `WorkspaceSettings` at `/explore/demo/settings/roles`; assert three tab links (Members, Roles & permissions, Shared objects) exist and the active one is marked.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** a horizontal tab bar in `WorkspaceSettings.tsx` using `NavLink` (active style consistent with the sidebar's `bg-accent`), pointing at `members`, `roles`, `shares` relative routes; keep the existing `<Outlet/>`. Add the two child routes in `App.tsx`:

```tsx
<Route path="settings" element={<WorkspaceSettings />}>
  <Route index element={<Navigate to="members" replace />} />
  <Route path="members" element={<Members />} />
  <Route path="roles" element={<RolesPermissions />} />
  <Route path="shares" element={<SharedObjects />} />
</Route>
```

- [ ] **Step 4: Run, verify PASS.**
- [ ] **Step 5: Commit.** `feat(fe): workspace settings access tabs`

### Task 8: Roles & permissions page (read-only)

**Files:** Create `RolesPermissions.tsx` + test.

- [ ] **Step 1: Write the failing test** — `vi.mock("@/api/workspaceAccess")` returns two roles (ws_admin w/ perms, ws_viewer w/ 1 perm); assert role names render, permission keys render grouped, the `parent_role_id` "Inherits from" is shown, and there is **no** edit control (read-only).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.** Header + the standard note ("Roles are defined platform-wide in Control Plane → Access control and apply to every workspace. This is a read-only view."). For each role render a `Card`: name, `built-in`/inherits badge, and a `FilterableTable` of its permissions (columns: Permission `key` mono, Description, Group) with a search box. Loading → `ListSkeleton`; 403 → "Only workspace admins can view roles." Mirror `Members.tsx` error/loading handling.
- [ ] **Step 4: Run, verify PASS.**
- [ ] **Step 5: Commit.** `feat(fe): workspace roles & permissions (read-only)`

### Task 9: Shared objects roll-up page

**Files:** Create `SharedObjects.tsx` + test.

- [ ] **Step 1: Write the failing test** — mock `useWorkspaceShares` with a chart shared to a group (allow) and a dashboard denied to a user; assert both rows render with kind badge, principal, role, and an allow/deny badge; assert the row action "Manage sharing" opens `ShareDialog` for that object (kind+id).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.** `FilterableTable` (columns: Object `kind`+name, Principal type+name, Role, Effect allow/deny badge using the same `--success`/`--destructive` `color-mix` tokens as `AccessExplorer`), a `FacetSelect` on `kind` and on `effect`, a `SummaryStrip` (`N shares · M objects · K denied`), search, and a per-row dropdown "Manage sharing…" that opens the existing `ShareDialog` (pass `kind`/`id`). Reuse `AccessExplorer`'s `allowStyle`/`denyStyle` (extract to a shared helper if convenient, else copy the two `CSSProperties`). Loading → `ListSkeleton`; empty → "Nothing shared in this workspace yet."
- [ ] **Step 4: Run, verify PASS.**
- [ ] **Step 5: Commit.** `feat(fe): workspace shared-objects roll-up`

### Task 10: Wire the Members-page note to the new tab

**Files:** Modify `Members.tsx` (the existing note at lines ~287–293).

- [ ] **Step 1:** Update the note copy so it references the new **Roles & permissions** tab for "what a role can do" and keeps the Control-Plane pointer for platform-wide access. Keep it one sentence.
- [ ] **Step 2:** Run the Members test to confirm no regression. Run: `cd frontend && npm run test -- Members`.
- [ ] **Step 3: Commit.** `chore(fe): cross-link members note to roles tab`

---

## Verification (before proposing merge)

Per the repo's "run full CI locally before merge" rule (GitHub CI is out of quota):

- [ ] Backend: `cargo fmt --all --check && cargo clippy --all-targets && cargo test -p egret-workspace` (and `--workspace` if touching shared crates). Do **not** run two `cargo test --workspace` concurrently.
- [ ] Frontend: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] Manual (Playwright MCP browser driving, per the live-e2e recipe — this is *manual driving*, not an automated Playwright test): log in, open a workspace → Settings → Roles & permissions (verify read-only, perms grouped), → Shared objects (share a chart via ShareDialog, confirm it appears in the roll-up with correct effect). Confirm a non-admin member gets the 403 copy.
- [ ] **Security check (critical):** confirm `GET /ws/{ws}/shares` never returns a grant for an object in another workspace (the Task 4 cross-workspace test is the guard) — this is the IDOR surface.

---

## Self-Review Checklist (author runs before handoff)

1. **Spec coverage:** Members (existing) ✅ · Roles & permissions read-only view (Tasks 2,3,8) ✅ · Shared-objects roll-up (Tasks 4,9) ✅ · tab IA (Task 7) ✅ · Non-Goals honored (no role editing in ws, no `workspace_id`) ✅.
2. **Placeholder scan:** Two spikes (Tasks 1, 4 Step 1) are genuine read-to-confirm steps, not code placeholders — the exact join-table name and object→workspace resolver must be read from the codebase, not guessed. Every code-writing step shows code.
3. **Type consistency:** `WsRoleDto`/`WsPermDto`/`ObjectGrantRollupDto` (BE) ↔ `WsRole`/`WsPerm`/`ObjectGrantRollup` (FE) names/fields match across Tasks 2–9.

---

## Follow-ups (not in this plan)

- **B2:** per-workspace custom roles (`roles.workspace_id`, Cedar template scoping, ws-admin role CRUD) — makes "push definition down to the workspace" fully valid.
- Control-Plane **Access control Overview** page + **Who-can-access** reverse view (from the same review, P0 items).
- If `useWorkspaceRoles()` in `Members.tsx` currently 403s for non-`role:read` ws_admins (it calls the CP `listRoles()`), repoint it at the new `GET /ws/{ws}/roles` and drop the CP dependency.
