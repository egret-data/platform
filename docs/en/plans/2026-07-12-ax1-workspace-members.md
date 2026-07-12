# AX-1 Workspace Members Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each workspace a Members admin page so a `ws_admin` can manage their own workspace's members/roles without a platform admin.

**Architecture:** Backend enriches the existing `/api/ws/{ws}/members` API (names + provenance + platform-admin flag), accepts custom workspace roles, and guards the last admin. Frontend adds a workspace-settings shell hosting a Members page (table + inline role select + add picker) built on a lean `FilterableTable`. No schema migration.

**Tech Stack:** Rust (axum, sqlx), Postgres, React 18/TS/Vite/Tailwind v4 (OKLCH)/shadcn/orval-adjacent hand hooks/Vitest.

**Spec:** `docs/en/specs/2026-07-12-ax1-workspace-members-design.md` · **Mockup:** `scratchpad/workspace-members-mockup.html`

**Key facts (verified):**
- `MemberRepo::add` is already **replace-then-insert** (one ws role per member) → role change = POST the member again. No new endpoint.
- `MemberRepo::list` returns only `{principal_type, principal_id, role}` (`WorkspaceMember` in `crates/egret-workspace/src/model.rs:15`).
- `add` guards role with `WORKSPACE_ROLES` const (`model.rs:22`) → rejects custom ws roles.
- No last-admin guard exists.
- FE routes: `ExploreRoutes` in `frontend/src/App.tsx` (uses `<AppShell>` + `<Routes>`); guard `RequirePermission` (`frontend/src/auth/guards.tsx:34`) checks `me.permissions` which is **platform-scoped only** → cannot gate a workspace perm; use **backend-authoritative 403 + friendly message** (mirror `ShareDialog`'s `forbidden` state).
- Client primitives: `apiJson<T>(path, init)` / `apiFetch` (`frontend/src/api/client.ts`). Roles: `listRoles()` → `AccessRole[]` with `.scope` (`frontend/src/api/access.ts:130`). Principal search: `listPrincipals(q, kind)` (`frontend/src/api/policies.ts:137`). Add-grant picker to copy: `AddGrantRow` in `frontend/src/components/sharing/ShareDialog.tsx`. Sort header: `SortableHeader` (`frontend/src/components/explore/SortableHeader.tsx`).
- Token wiring: `frontend/src/index.css` — `:root` (light ~L95), `@media dark`/`.dark` (~L143), `@theme inline` (~L178). `--highlight` is the pattern to mirror for a new `--success`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/egret-workspace/src/model.rs` | `WorkspaceMember` fields | enrich |
| `crates/egret-workspace/src/members.rs` | list query + role validation + `count_ws_admins` | enrich |
| `crates/egret-workspace/src/routes.rs` | `MemberDto` + last-admin guard in handlers | enrich |
| `crates/egret-workspace/tests/members_ax1.rs` | backend integration tests | **create** |
| `frontend/src/index.css` | `--success` token | add |
| `frontend/src/api/members.ts` | member hooks + types | **create** |
| `frontend/src/components/explore/FilterableTable.tsx` | lean search+sort table primitive | **create** |
| `frontend/src/routes/explore/settings/WorkspaceSettings.tsx` | settings shell + sub-nav | **create** |
| `frontend/src/routes/explore/settings/Members.tsx` | the Members page | **create** |
| `frontend/src/App.tsx` | route + nav gear entry | modify |
| `frontend/src/routes/explore/settings/__tests__/Members.test.tsx` | FE tests | **create** |

---

## Task 1: Backend — enrich member list (names, provenance, platform-admin flag)

**Files:** Modify `crates/egret-workspace/src/model.rs`, `crates/egret-workspace/src/members.rs`, `crates/egret-workspace/src/routes.rs`. Test in Task 4.

- [ ] **Step 1: Extend `WorkspaceMember`**

In `crates/egret-workspace/src/model.rs`, replace the `WorkspaceMember` struct:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceMember {
    pub principal_type: String, // user | group
    pub principal_id: Uuid,
    pub role: String,
    pub principal_name: Option<String>,
    pub granted_by_name: Option<String>,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub is_platform_admin: bool,
}
```

- [ ] **Step 2: Enrich the `list` query + `MemberRow`**

In `crates/egret-workspace/src/members.rs`, replace `list` and `MemberRow`:

```rust
    pub async fn list(&self, ws: WorkspaceId) -> Result<Vec<WorkspaceMember>> {
        let rows = sqlx::query_as::<_, MemberRow>(
            "SELECT ag.principal_type, ag.principal_id, r.name AS role, \
                    COALESCE(u.email::text, g.name) AS principal_name, \
                    gb.email::text AS granted_by_name, \
                    ag.created_at AS added_at, \
                    EXISTS ( \
                      SELECT 1 FROM access_grants pa JOIN roles pr ON pr.id = pa.role_id \
                      WHERE pa.principal_type = ag.principal_type AND pa.principal_id = ag.principal_id \
                        AND pa.scope_type = 'platform' AND pr.name = 'platform_admin' \
                    ) AS is_platform_admin \
             FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             LEFT JOIN users  u  ON ag.principal_type = 'user'  AND u.id = ag.principal_id \
             LEFT JOIN groups g  ON ag.principal_type = 'group' AND g.id = ag.principal_id \
             LEFT JOIN users  gb ON gb.id = ag.granted_by \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
             ORDER BY ag.created_at",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
```

Replace the `MemberRow` struct + `From` impl at the bottom of the file:

```rust
#[derive(sqlx::FromRow)]
struct MemberRow {
    principal_type: String,
    principal_id: Uuid,
    role: String,
    principal_name: Option<String>,
    granted_by_name: Option<String>,
    added_at: chrono::DateTime<chrono::Utc>,
    is_platform_admin: bool,
}

impl From<MemberRow> for WorkspaceMember {
    fn from(r: MemberRow) -> Self {
        WorkspaceMember {
            principal_type: r.principal_type,
            principal_id: r.principal_id,
            role: r.role,
            principal_name: r.principal_name,
            granted_by_name: r.granted_by_name,
            added_at: r.added_at,
            is_platform_admin: r.is_platform_admin,
        }
    }
}
```

- [ ] **Step 3: Extend `MemberDto` + its mapping**

In `crates/egret-workspace/src/routes.rs`, replace the `MemberDto` struct (~L428) and the `list_members` mapping:

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct MemberDto {
    pub principal_type: String,
    pub principal_id: String,
    pub role: String,
    pub principal_name: Option<String>,
    pub granted_by_name: Option<String>,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub is_platform_admin: bool,
}
```

In `list_members`, replace the `.map(|m| MemberDto { … })` closure:

```rust
            .map(|m| MemberDto {
                principal_type: m.principal_type,
                principal_id: m.principal_id.to_string(),
                role: m.role,
                principal_name: m.principal_name,
                granted_by_name: m.granted_by_name,
                added_at: m.added_at,
                is_platform_admin: m.is_platform_admin,
            })
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p egret-workspace`
Expected: compiles clean (chrono is already a dep — it's used elsewhere in the crate).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-workspace/src/model.rs crates/egret-workspace/src/members.rs crates/egret-workspace/src/routes.rs
git commit -m "feat(workspace): enrich member list with name, provenance, platform-admin flag"
```

---

## Task 2: Backend — accept custom workspace roles

**Files:** Modify `crates/egret-workspace/src/members.rs`. Test in Task 4.

- [ ] **Step 1: Replace the role guard in `add`**

In `crates/egret-workspace/src/members.rs::add`, replace the `WORKSPACE_ROLES.contains(&role)` guard with a DB-backed existence check (any `scope='workspace'` role — seeded or custom). Change the guard block to:

```rust
        if ptype != "user" && ptype != "group" {
            return Err(Error::InvalidInput(format!("bad principal_type: {ptype}")));
        }
        let role_ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM roles WHERE scope = 'workspace' AND name = $1)",
        )
        .bind(role)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        if !role_ok {
            return Err(Error::InvalidInput(format!(
                "unknown workspace role: {role}"
            )));
        }
```

Remove the now-unused `WORKSPACE_ROLES` import from `members.rs`'s `use crate::model::{...}` line if it is no longer referenced there (keep `WorkspaceMember`).

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p egret-workspace`
Expected: clean (fix the unused-import if clippy flags it).

- [ ] **Step 3: Commit**

```bash
git add crates/egret-workspace/src/members.rs
git commit -m "feat(workspace): accept any workspace-scoped role for members (incl. custom)"
```

---

## Task 3: Backend — last-admin guard

**Files:** Modify `crates/egret-workspace/src/members.rs`, `crates/egret-workspace/src/routes.rs`. Test in Task 4.

- [ ] **Step 1: Add `count_ws_admins` + a role lookup helper**

In `crates/egret-workspace/src/members.rs`, add to `impl MemberRepo`:

```rust
    /// Number of members holding `ws_admin` in the workspace.
    pub async fn count_ws_admins(&self, ws: WorkspaceId) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT count(*) FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 AND r.name = 'ws_admin'",
        )
        .bind(ws.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// The workspace role a principal currently holds here, if any.
    pub async fn role_of(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT r.name FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
               AND ag.principal_type = $2 AND ag.principal_id = $3",
        )
        .bind(ws.0)
        .bind(ptype)
        .bind(pid)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }
```

- [ ] **Step 2: Guard `remove_member` (routes.rs)**

In `crates/egret-workspace/src/routes.rs::remove_member`, after the `require(...)` line and before `st.members.remove(...)`, insert:

```rust
    // Don't let a workspace lose its last admin (it would become unmanageable
    // without platform intervention).
    if st.members.role_of(ctx.workspace_id, &ptype, pid).await.map_err(AuthError)?.as_deref() == Some("ws_admin")
        && st.members.count_ws_admins(ctx.workspace_id).await.map_err(AuthError)? <= 1
    {
        return Err(AuthError(egret_core::Error::Conflict(
            "the workspace must keep at least one admin".into(),
        )));
    }
```

- [ ] **Step 3: Guard `add_member` (demote path) (routes.rs)**

In `crates/egret-workspace/src/routes.rs::add_member`, after the `require(...)` line and before `st.members.add(...)`, insert:

```rust
    // A role change is a replace; demoting the last ws_admin is a last-admin removal.
    if body.role != "ws_admin"
        && st.members.role_of(ctx.workspace_id, &body.principal_type, body.principal_id).await.map_err(AuthError)?.as_deref() == Some("ws_admin")
        && st.members.count_ws_admins(ctx.workspace_id).await.map_err(AuthError)? <= 1
    {
        return Err(AuthError(egret_core::Error::Conflict(
            "the workspace must keep at least one admin".into(),
        )));
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p egret-workspace`
Expected: clean. (Confirm `egret_core::Error::Conflict` exists and `AuthError` wraps `egret_core::Error` — it does; other handlers use `AuthError(...)`.)

- [ ] **Step 5: Commit**

```bash
git add crates/egret-workspace/src/members.rs crates/egret-workspace/src/routes.rs
git commit -m "feat(workspace): guard against removing/demoting a workspace's last admin"
```

---

## Task 4: Backend — integration tests

**Files:** Create `crates/egret-workspace/tests/members_ax1.rs`.

Read `crates/egret-workspace/tests/workspace_db.rs` first for the fixture helpers (DB pool, workspace/user/role setup, `MemberRepo`/`WsContext` construction, the `has_object_grant_in_workspace` guest test). Reuse those helpers; match their DB-gating (self-skip without `DATABASE_URL`).

- [ ] **Step 1: Write the tests**

Create `crates/egret-workspace/tests/members_ax1.rs` with four tests. Fill each body using the `workspace_db.rs` helpers (create workspace, users, groups, roles; call `MemberRepo`/handlers directly as that file does). Each MUST assert a concrete outcome.

```rust
// 1. Enriched list resolves display names + provenance + platform-admin flag.
#[tokio::test]
async fn list_enriches_names_provenance_and_platform_admin() {
    // Arrange: ws with a user member (email known) added by admin, and a member who
    //          also holds platform_admin.
    // Assert: principal_name == that email; granted_by_name == admin email;
    //         added_at is set; is_platform_admin true for the admin-holder, false for a plain member.
}

// 2. add accepts a custom workspace role; rejects unknown / platform-scoped names.
#[tokio::test]
async fn add_accepts_custom_ws_role_rejects_bad_role() {
    // Arrange: create a custom workspace role "Report Builder".
    // Act+Assert: add(user, "Report Builder") -> Ok; add(user, "platform_admin") -> InvalidInput;
    //             add(user, "nope") -> InvalidInput.
}

// 3. Re-adding a member with a new role replaces (still one grant).
#[tokio::test]
async fn re_add_replaces_role() {
    // add(u, ws_editor) then add(u, ws_viewer); list shows u once with role ws_viewer.
}

// 4. Last-admin guard blocks remove + demote of the sole ws_admin.
#[tokio::test]
async fn last_admin_guard() {
    // Sole ws_admin: remove -> Conflict; demote via add(u, ws_editor) -> Conflict.
    // With a second ws_admin present: removing one -> Ok.
    // (Drive through the handler fns if workspace_db.rs does, else repo + explicit guard call.)
}
```

- [ ] **Step 2: Run the tests (once — never two workspace suites concurrently)**

Run: `cargo test -p egret-workspace --test members_ax1`
Expected: 4 passed (against a real Postgres; self-skips if `DATABASE_URL` unset — provision an ephemeral DB the way earlier increments did, or rely on the reviewer's DB run).

- [ ] **Step 3: Commit**

```bash
git add crates/egret-workspace/tests/members_ax1.rs
git commit -m "test(workspace): AX-1 member enrich, custom role, replace, last-admin guard"
```

---

## Task 5: Frontend — `--success` token

**Files:** Modify `frontend/src/index.css`.

- [ ] **Step 1: Add the token in all three blocks (mirror `--highlight`)**

In `:root` (light, ~L120 next to `--highlight`): add
```css
  --success: oklch(0.55 0.14 155);
  --success-foreground: oklch(0.99 0 0);
```
In the dark block (`.dark` / `@media`, ~L164 next to `--highlight`): add
```css
  --success: oklch(0.72 0.14 155);
  --success-foreground: oklch(0.18 0.04 155);
```
In `@theme inline` (~L195 next to `--color-highlight`): add
```css
  --color-success: var(--success);
  --color-success-foreground: var(--success-foreground);
```

- [ ] **Step 2: Verify build**

Run: `cd frontend && npm run build`
Expected: builds; `bg-success` / `text-success` utilities are now available.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/index.css
git commit -m "feat(fe): add --success semantic token (state colour, distinct from teal brand)"
```

---

## Task 6: Frontend — member API hooks

**Files:** Create `frontend/src/api/members.ts`.

- [ ] **Step 1: Write the hooks + types**

Create `frontend/src/api/members.ts` (mirror the `apiJson` usage in `frontend/src/api/access.ts` and react-query hook shape in `frontend/src/api/sqllab.ts`):

```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./client";
import { listRoles, type AccessRole } from "./access";

export interface WorkspaceMember {
  principal_type: "user" | "group";
  principal_id: string;
  role: string;
  principal_name: string | null;
  granted_by_name: string | null;
  added_at: string;
  is_platform_admin: boolean;
}

export const membersKey = (ws: string) => ["ws", ws, "members"] as const;

export function useWorkspaceMembers(ws: string) {
  return useQuery<WorkspaceMember[]>({
    queryKey: membersKey(ws),
    queryFn: () => apiJson<WorkspaceMember[]>(`/api/ws/${ws}/members`),
    retry: false,
  });
}

/** Add a member OR change an existing member's role (backend replaces). */
export function useAddMember(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { principal_type: string; principal_id: string; role: string }) =>
      apiJson(`/api/ws/${ws}/members`, { method: "POST", body: JSON.stringify(b) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: membersKey(ws) }),
  });
}

export function useRemoveMember(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (m: { principal_type: string; principal_id: string }) =>
      apiJson(`/api/ws/${ws}/members/${m.principal_type}/${m.principal_id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: membersKey(ws) }),
  });
}

/** Workspace-scoped roles offered when assigning a member (seeded + custom). */
export function useWorkspaceRoles() {
  return useQuery<AccessRole[]>({
    queryKey: ["ws", "roles", "workspace-scope"],
    queryFn: async () => (await listRoles()).filter((r) => r.scope === "workspace"),
    retry: false,
  });
}
```

Verify `apiJson`'s signature (`frontend/src/api/client.ts`) — if it doesn't set JSON `Content-Type` / credentials automatically, follow exactly how `access.ts` calls it for POSTs (copy that call shape, including any CSRF handling the client wraps).

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/api/members.ts
git commit -m "feat(fe): workspace member API hooks (list/add/remove + ws roles)"
```

---

## Task 7: Frontend — `FilterableTable` v1

**Files:** Create `frontend/src/components/explore/FilterableTable.tsx`.

A lean primitive: a search box + a slot for extra filters, over a shadcn `Table`, with sortable headers via the existing `SortableHeader`. Generic over a row type. Read `frontend/src/components/explore/SortableHeader.tsx` for its `SortState`/`onSort` API and reuse it.

- [ ] **Step 1: Write the component**

```tsx
import { useMemo, useState, type ReactNode } from "react";
import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";

export interface Column<T> {
  key: string;
  label: string;
  sortable?: boolean;
  className?: string;
  value?: (row: T) => string | number; // for sort + default search
  cell: (row: T) => ReactNode;
}

export function FilterableTable<T>({
  rows, columns, getId, searchPlaceholder = "Search…", filters, initialSort, emptyText = "No results.",
}: {
  rows: T[];
  columns: Column<T>[];
  getId: (row: T) => string;
  searchPlaceholder?: string;
  filters?: ReactNode;
  initialSort?: SortState<string>;
  emptyText?: string;
}) {
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<SortState<string> | undefined>(initialSort);

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    let r = rows;
    if (needle) {
      r = r.filter((row) =>
        columns.some((c) => String(c.value?.(row) ?? "").toLowerCase().includes(needle)),
      );
    }
    if (sort) {
      const col = columns.find((c) => c.key === sort.key);
      if (col?.value) {
        r = [...r].sort((a, b) => {
          const av = col.value!(a), bv = col.value!(b);
          const cmp = typeof av === "number" && typeof bv === "number" ? av - bv : String(av).localeCompare(String(bv));
          return sort.dir === "asc" ? cmp : -cmp;
        });
      }
    }
    return r;
  }, [rows, q, sort, columns]);

  const onSort = (key: string) =>
    setSort((s) => (s?.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative max-w-xs flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input className="pl-8" placeholder={searchPlaceholder} value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        {filters}
      </div>
      <div className="overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow>
              {columns.map((c) =>
                c.sortable ? (
                  <SortableHeader key={c.key} sortKey={c.key} label={c.label} current={sort} onSort={onSort} />
                ) : (
                  <TableHead key={c.key} className={c.className}>{c.label}</TableHead>
                ),
              )}
            </TableRow>
          </TableHeader>
          <TableBody>
            {shown.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableHead colSpan={columns.length} className="py-6 text-center font-normal text-muted-foreground">
                  {emptyText}
                </TableHead>
              </TableRow>
            ) : (
              shown.map((row) => (
                <TableRow key={getId(row)}>
                  {columns.map((c) => (
                    <td key={c.key} className={`p-4 align-middle ${c.className ?? ""}`}>{c.cell(row)}</td>
                  ))}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
```

If `SortableHeader`'s prop names differ from `{sortKey,label,current,onSort}`, adapt to the real signature (read the file). Keep the primitive minimal — AX-2 generalizes it.

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/components/explore/FilterableTable.tsx
git commit -m "feat(fe): FilterableTable v1 (search + sort primitive)"
```

---

## Task 8: Frontend — settings shell + route + nav entry

**Files:** Create `frontend/src/routes/explore/settings/WorkspaceSettings.tsx`; modify `frontend/src/App.tsx`.

- [ ] **Step 1: Write the settings shell**

Create `frontend/src/routes/explore/settings/WorkspaceSettings.tsx` — a sub-nav + `<Outlet/>` layout (only "Members" now):

```tsx
import { NavLink, Outlet, useParams } from "react-router-dom";
import { Users } from "lucide-react";

const TABS = [{ to: "members", label: "Members", icon: Users }];

export default function WorkspaceSettings() {
  const ws = useParams().workspace!;
  return (
    <div className="p-4">
      <h1 className="mb-4 text-lg font-semibold">Workspace settings</h1>
      <div className="grid grid-cols-[180px_1fr] gap-6">
        <nav className="flex flex-col gap-1">
          {TABS.map((t) => (
            <NavLink
              key={t.to}
              to={`/explore/${ws}/settings/${t.to}`}
              className={({ isActive }) =>
                `flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium ${isActive ? "bg-muted text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"}`
              }
            >
              <t.icon className="h-4 w-4" /> {t.label}
            </NavLink>
          ))}
        </nav>
        <div className="min-w-0"><Outlet /></div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Wire routes in `App.tsx`**

Add imports near the other explore imports:
```tsx
import WorkspaceSettings from "@/routes/explore/settings/WorkspaceSettings";
import Members from "@/routes/explore/settings/Members";
```
Inside `ExploreRoutes`'s `<Routes>` (alongside `notebook-settings`), add:
```tsx
          <Route path="settings" element={<WorkspaceSettings />}>
            <Route index element={<Navigate to="members" replace />} />
            <Route path="members" element={<Members />} />
          </Route>
```

- [ ] **Step 3: Add the nav gear entry**

In the workspace nav rendered by `AppShell` (find where `AppShell` renders the explore nav links — grep `AppShell` under `frontend/src`), add a "Workspace settings" link to `/explore/${ws}/settings/members` with a `Settings` (gear) icon, placed with the other workspace nav items. Show it to all members; the page itself handles non-admins via a friendly forbidden state (Task 9). Match the existing nav-item markup exactly.

- [ ] **Step 4: Typecheck + build**

Run: `cd frontend && npm run typecheck && npm run build`
Expected: clean; navigating to `/explore/{ws}/settings` redirects to `…/members`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/settings/WorkspaceSettings.tsx frontend/src/App.tsx
git commit -m "feat(fe): workspace-settings shell + route + nav entry"
```

---

## Task 9: Frontend — the Members page

**Files:** Create `frontend/src/routes/explore/settings/Members.tsx`. Mirror the published mockup (`scratchpad/workspace-members-mockup.html`) for layout/markup and copy the `AddGrantRow` picker pattern from `frontend/src/components/sharing/ShareDialog.tsx`.

- [ ] **Step 1: Write the page**

Build `Members.tsx` with these parts (complete logic below; presentational markup follows the mockup):

- Data: `const ws = useParams().workspace!;` `const { data, isLoading, error } = useWorkspaceMembers(ws);` `const roles = useWorkspaceRoles();` `const add = useAddMember(ws);` `const remove = useRemoveMember(ws);`
- **Forbidden state** (non-admin): `const forbidden = (error as { status?: number } | null)?.status === 403;` — when true, render a friendly panel: *"Only workspace admins can manage members."* (mirror `ShareDialog`'s `forbidden` branch).
- **Summary strip:** compute counts from `data` — total, `admins = data.filter(m => m.role === "ws_admin").length`, `people = users`, `groups`. Render a one-line `text-sm text-muted-foreground` strip, not KPI cards.
- **Scope clarifier:** a callout: *"Workspace roles apply only inside {ws}. Platform-wide access is granted separately in Control Plane → Access control."*
- **Add picker:** copy `AddGrantRow` from `ShareDialog.tsx` (User/Group toggle + `listPrincipals` search + role select). Replace its role options with `roles.data` (workspace roles); on Add call `add.mutate({ principal_type, principal_id, role })` with success/error toasts.
- **Table via `FilterableTable`** with columns: Member (name = `m.principal_name ?? m.principal_id`, plus a `platform admin` `Badge` styled with `--highlight` when `m.is_platform_admin`), Type (`Badge`), Role (inline `Select`), Added by (`m.granted_by_name ?? "—"`), Added (relative time from `m.added_at`), and a `⋯` actions menu (Remove). Provide `value:` accessors for search/sort (name, role, added_at).
- **Inline role change:** the Role `Select`'s `onValueChange` calls `add.mutate({ principal_type: m.principal_type, principal_id: m.principal_id, role: newRole })` (backend replaces). On error toast the message. **Guardrail:** if `m.role === "ws_admin"` and `admins <= 1`, render the role as a disabled control with a `Last admin — role locked` caption instead of the `Select`.
- **Remove:** in the `⋯` menu, a destructive "Remove from workspace" that opens a confirm (`ConfirmDeleteDialog` or `AlertDialog`) then `remove.mutate(...)`. Disable it when `m.role === "ws_admin" && admins <= 1`. If the member is the current user, add a "You're removing your own access" warning line in the confirm.
- **Role swatch colours:** admin → `--success`, editor → `--primary`, viewer → `--muted-foreground` (inline `style={{ background: "var(--success)" }}` etc., never `hsl(var(--token))`).

Keep the whole page focused; if it grows past ~250 lines, extract the row's role-cell into a small local component (not a new file).

- [ ] **Step 2: Verify against the running app (manual smoke)**

With the local stack running (`analytics` workspace seeded), navigate to `/explore/analytics/settings/members`: real names render (not UUIDs), alice shows a `platform admin` badge and (as sole admin) a locked role, changing bob's role persists, the add picker resolves people. (Automated assertions in Step 3.)

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/explore/settings/Members.tsx
git commit -m "feat(fe): workspace Members page (inline role, add picker, guardrails, scope clarifier)"
```

---

## Task 10: Frontend — Members page tests

**Files:** Create `frontend/src/routes/explore/settings/__tests__/Members.test.tsx`.

Read an existing route test (e.g. `frontend/src/routes/explore/__tests__/Notebooks.test.tsx`) for the QueryClientProvider + router + mock harness; copy it.

- [ ] **Step 1: Write failing tests**

```tsx
// Mock @/api/members to return a fixed member set incl. a platform-admin member and a sole ws_admin.
it("renders member names (not UUIDs) and a platform-admin badge", async () => { /* findByText email; findByText "platform admin" */ });
it("changing a member's role calls the add mutation with the new role", async () => { /* open Select, pick a role, assert mutate called */ });
it("locks the sole admin's role and disables their remove", async () => { /* assert the last-admin caption + disabled control */ });
it("shows the scope clarifier", async () => { /* findByText(/apply only inside/i) */ });
```

Fill bodies against the copied harness; mock `@/api/members` + `@/api/policies` (`listPrincipals`).

- [ ] **Step 2: Run — fail then implement-adjust then pass**

Run: `cd frontend && npm run test -- Members`
Expected: PASS after wiring the mocks to the page.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/explore/settings/__tests__/Members.test.tsx
git commit -m "test(fe): Members page — names, role change, last-admin lock, clarifier"
```

---

## Task 11: Full CI + review

**Files:** none (verification).

- [ ] **Step 1: Backend sweep** — `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-workspace` (run the member test against an ephemeral DB as prior increments did). Expect green.
- [ ] **Step 2: Frontend sweep** — `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`. Expect green.
- [ ] **Step 3: Adversarial security/UX review** over the branch diff. Focus: `workspace:manage_members` gating on every member path; last-admin guard can't be bypassed via the add/replace path; custom-role validation is server-side (never trust the client role string); cross-workspace isolation of list/add/remove; the settings route/nav doesn't leak actions to non-admins (page 403s friendly). Fix any HIGH/MEDIUM in-branch.
- [ ] **Step 4: Hand off to `superpowers:finishing-a-development-branch`** → Push + PR → squash-merge `--admin` (the user's standard flow), then update memory.

---

## Self-Review

**Spec coverage:** §1a enrich → Task 1 ✓; §1b custom roles → Task 2 ✓; §1c last-admin → Task 3 ✓; §1d no new endpoint (role change via add) → Tasks 3/6/9 ✓; §2a shell → Task 8 ✓; §2b Members page → Task 9 ✓; §2c FilterableTable → Task 7 ✓; §2d hooks → Task 6 ✓; §3 `--success` → Task 5 ✓; testing → Tasks 4 + 10 + 11 ✓.

**Placeholder scan:** Task 4/10 test bodies are specified as arrange/act/assert against named fixtures (`workspace_db.rs` / `Notebooks.test.tsx` harnesses) rather than fully coded, because they depend on those existing harnesses the implementer must read first; each names the exact scenario + assertion. Task 9's presentational markup defers to the mockup + `AddGrantRow` (concrete sources) while all logic (mutations, guardrails, forbidden state, counts) is spelled out. No `TBD`.

**Type consistency:** `WorkspaceMember`/`MemberRow`/`MemberDto` gain the same 4 fields (`principal_name: Option<String>`, `granted_by_name: Option<String>`, `added_at: DateTime<Utc>`, `is_platform_admin: bool`); FE `WorkspaceMember` mirrors them as `string | null` / `string` / `boolean`. Role change everywhere = `useAddMember` POST (no separate update). `count_ws_admins`/`role_of` used consistently in both guards. `--success` token name consistent across the three CSS blocks + `--color-success`.
