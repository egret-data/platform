# AX-4 Surface the Hidden Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Roles surface show the role hierarchy (an "Inherits from" column + a Tree-view toggle) and a role editor that only offers scope-appropriate permissions (plus in-dialog search + select-all-per-group), and add in-product platform-vs-workspace scope clarifiers.

**Architecture:** Frontend-only edits to `RolesPage.tsx`. The data is already present: `AccessRole` has `parentRoleId` + `scope`; each `PermissionDto` has `scope`. No backend change.

**Tech Stack:** React 18/TS, shadcn/ui, Tailwind v4, Vitest.

**Spec:** `docs/en/specs/2026-07-13-ax4-surface-model-design.md`

**Verified current state (`frontend/src/routes/control-plane/access/RolesPage.tsx`):**
- Main component uses `FilterableTable` (AX-2) with a `columns: Column<AccessRole>[]` (Scope/Name/Description/Permissions/Assignees/actions), `filtered` rows (Scope facet), a `SummaryStrip`, and dialogs `CreateRoleDialog`/`EditRoleDialog`/`RoleAssigneesDialog`/`ConfirmDeleteDialog`. `isAdmin` gates actions + "New role".
- `EditRoleDialog` fetches `listPermissions()` → `groups: PermissionCatalogGroup[]` (`{group, permissions: [{key, description, roles: string[], scope: string}]}`); holds `checked: Set<string>`, `inherited: Set<string>` (from `getRole`), `isSuperuser = role.name === "platform_admin"`; renders each group as an uppercase heading + a `sm:grid-cols-2` grid of `<Checkbox>`+`<Label>` per permission; `toggle(key, isChecked)` mutates `checked`. Save calls `setRolePermissions(role.id, [...checked])` unless superuser.
- `AccessRole = { id, scope, name, description, permissionCount, assigneeCount, parentRoleId? }`. `isSeeded(role)` + `SEEDED_ROLE_NAMES` exist.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `frontend/src/routes/control-plane/access/RolesPage.tsx` | editor scope-filter+search+select-all; "Inherits from" column; clarifiers; Tree-view toggle + in-file `RolesTree` | modify |
| `frontend/src/routes/control-plane/access/__tests__/RolesPage.test.tsx` | extend tests | modify |

(Per the spec, keep the tree as an in-file component. Only split to a sibling `RolesTree.tsx` if the file grows well past its current size — the reviewer will call it.)

---

## Task 1: Role editor — scope-filter + in-dialog search + select-all-per-group + scope line

**Files:** Modify `EditRoleDialog` in `frontend/src/routes/control-plane/access/RolesPage.tsx`.

- [ ] **Step 1: Add search state + the scope-filtered, searched groups**

In `EditRoleDialog`, after `const groups = permissions.data ?? [];`, add:
```tsx
  const [permQuery, setPermQuery] = useState("");

  // Only permissions this role's scope can hold; then apply the in-dialog search.
  const shownGroups = useMemo(() => {
    const needle = permQuery.trim().toLowerCase();
    return groups
      .map((g) => ({
        ...g,
        permissions: g.permissions.filter(
          (p) =>
            (!role || p.scope === role.scope) &&
            (needle === "" ||
              p.key.toLowerCase().includes(needle) ||
              p.description.toLowerCase().includes(needle)),
        ),
      }))
      .filter((g) => g.permissions.length > 0);
  }, [groups, role, permQuery]);
```

- [ ] **Step 2: Add a select-all-per-group helper**

Add inside `EditRoleDialog` (near `toggle`):
```tsx
  /** Toggle every non-inherited permission of a group on/off at once. */
  function toggleGroup(perms: { key: string }[], on: boolean) {
    setChecked((prev) => {
      const next = new Set(prev);
      for (const p of perms) {
        if (inherited.has(p.key)) continue; // inherited perms are fixed
        if (on) next.add(p.key);
        else next.delete(p.key);
      }
      return next;
    });
  }
```

- [ ] **Step 3: Add the scope clarifier line + search box; render `shownGroups` with a group header checkbox**

Replace the matrix block (the `<div className="space-y-4">{permissions.isLoading ? … : groups.map(...)}</div>`) with:
```tsx
          {!isSuperuser && role && (
            <p className="text-xs text-muted-foreground">
              This is a <span className="font-medium text-foreground">{role.scope}</span> role
              — it can hold {role.scope}-scoped permissions.
            </p>
          )}

          {!isSuperuser && !permissions.isLoading && !permissions.isError && groups.length > 0 && (
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="pl-8"
                placeholder="Search permissions…"
                value={permQuery}
                onChange={(e) => setPermQuery(e.target.value)}
              />
            </div>
          )}

          <div className="space-y-4">
            {permissions.isLoading ? (
              <p className="text-sm text-muted-foreground">Loading…</p>
            ) : permissions.isError ? (
              <p className="text-sm text-destructive">
                {permissions.error?.message ?? "Failed to load permissions."}
              </p>
            ) : shownGroups.length === 0 ? (
              <p className="text-sm text-muted-foreground">No permissions match your search.</p>
            ) : (
              shownGroups.map((group) => {
                const editable = group.permissions.filter((p) => !inherited.has(p.key));
                const allOn = editable.length > 0 && editable.every((p) => checked.has(p.key));
                return (
                  <div key={group.group} className="space-y-1.5">
                    <div className="flex items-center gap-2">
                      {!isSuperuser && editable.length > 0 && (
                        <Checkbox
                          aria-label={`Select all ${group.group}`}
                          checked={allOn}
                          onCheckedChange={(c) => toggleGroup(editable, c === true)}
                        />
                      )}
                      <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                        {group.group}
                      </h3>
                    </div>
                    <div className="grid grid-cols-1 gap-x-6 gap-y-1.5 rounded-md border p-3 sm:grid-cols-2">
                      {group.permissions.map((p) => {
                        const id = `perm-${p.key}`;
                        const isInherited = inherited.has(p.key);
                        return (
                          <div key={p.key} className="flex items-center gap-2">
                            <Checkbox
                              id={id}
                              checked={isInherited || checked.has(p.key)}
                              disabled={isSuperuser || isInherited}
                              onCheckedChange={(c) => toggle(p.key, c === true)}
                            />
                            <Label htmlFor={id} className="cursor-pointer font-normal" title={p.description}>
                              {p.key}
                            </Label>
                            {isInherited && (
                              <Badge variant="secondary" className="font-normal">
                                inherited
                              </Badge>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                );
              })
            )}
          </div>
```
Add `Search` to the lucide-react import at the top of the file (it currently imports `MoreHorizontal, Pencil, Plus, Trash2`). `useMemo`, `Input`, `Checkbox` are already imported.

Note on checkbox affordance (spec item): the shadcn `Checkbox` already renders a filled check on `checked`. Leave it as the standard control; do not build a custom one. If, on visual smoke, the checked state is genuinely hard to see in dark mode, that's a shared shadcn-token issue out of this task's scope — flag it, don't hack the component here.

- [ ] **Step 4: Typecheck + lint + test**

Run: `cd frontend && npm run typecheck && npm run lint && npm run test -- RolesPage`
Expected: clean; existing RolesPage tests still pass.

- [ ] **Step 5: Extend the test**

In `__tests__/RolesPage.test.tsx`, add a test that opens `EditRoleDialog` for a **workspace** role (mock `listPermissions` to return a workspace group e.g. "Charts"/`chart:read` scope `workspace` AND a platform group e.g. "Engines"/`engine:read` scope `platform`; mock `getRole` for the role): assert the workspace group renders and the platform group does NOT; typing `chart` in "Search permissions…" keeps the chart permission and drops others; the "Select all Charts" checkbox toggles the chart permission. (Drive the dialog open via the row actions menu → "Edit permissions", as existing RolesPage tests do; reuse their harness.)

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/control-plane/access/RolesPage.tsx frontend/src/routes/control-plane/access/__tests__/RolesPage.test.tsx
git commit -m "feat(fe): role editor — scope-filter the permission matrix + search + select-all-per-group"
```

---

## Task 2: Roles page — "Inherits from" column + scope clarifier

**Files:** Modify the `RolesPage` main component in `frontend/src/routes/control-plane/access/RolesPage.tsx` + its test.

- [ ] **Step 1: Build a parent-name map + add the "Inherits from" column**

In the `RolesPage` component, before the `columns` definition, add:
```tsx
  const nameById = useMemo(() => new Map(rows.map((r) => [r.id, r.name])), [rows]);
```
(`useMemo` is imported.) In the `columns` array, insert a new column right after the `name` column (before `description`):
```tsx
    {
      key: "parent",
      label: "Inherits from",
      sortable: true,
      value: (r) => (r.parentRoleId ? (nameById.get(r.parentRoleId) ?? "") : ""),
      cell: (r) =>
        r.parentRoleId && nameById.get(r.parentRoleId) ? (
          <Badge variant="outline" className="font-normal">
            {nameById.get(r.parentRoleId)}
          </Badge>
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
    },
```

- [ ] **Step 2: Add the scope clarifier under the page header**

Replace the page header `<p>` subtitle block:
```tsx
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Roles</h1>
        <p className="text-sm text-muted-foreground">
          Platform roles apply across all of egret; workspace roles apply inside a single
          workspace and are assigned per-workspace on that workspace's Members page.
        </p>
      </div>
```

- [ ] **Step 3: Typecheck + lint + test**

Run: `cd frontend && npm run typecheck && npm run lint && npm run test -- RolesPage`
Expected: clean; existing tests pass.

- [ ] **Step 4: Extend the test**

Add assertions: a role with a `parentRoleId` pointing at another role in the list renders that parent's **name** (as a chip) in the row; a root role renders `—` in the Inherits-from column. (Mock `listRoles` with a parent + child pair.)

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/access/RolesPage.tsx frontend/src/routes/control-plane/access/__tests__/RolesPage.test.tsx
git commit -m "feat(fe): Roles table — Inherits-from column + scope clarifier"
```

---

## Task 3: Roles page — Tree-view toggle + `RolesTree`

**Files:** Modify the `RolesPage` main component + add an in-file `RolesTree` component in `frontend/src/routes/control-plane/access/RolesPage.tsx` + its test.

- [ ] **Step 1: Add the `RolesTree` in-file component**

Add this component in `RolesPage.tsx` (e.g. below `isSeeded`):
```tsx
/** Read-only indented tree of the role hierarchy, grouped by scope. */
function RolesTree({
  roles,
  isAdmin,
  onEdit,
}: {
  roles: AccessRole[];
  isAdmin: boolean;
  onEdit: (role: AccessRole) => void;
}) {
  const childrenOf = useMemo(() => {
    const ids = new Set(roles.map((r) => r.id));
    const m = new Map<string | null, AccessRole[]>();
    for (const r of roles) {
      const parent = r.parentRoleId && ids.has(r.parentRoleId) ? r.parentRoleId : null;
      const arr = m.get(parent) ?? [];
      arr.push(r);
      m.set(parent, arr);
    }
    return m;
  }, [roles]);

  const renderNode = (r: AccessRole, depth: number): React.ReactNode => {
    if (depth > 64) return null; // cycle guard (DB prevents cycles)
    const kids = childrenOf.get(r.id) ?? [];
    return (
      <div key={r.id}>
        <button
          type="button"
          disabled={!isAdmin}
          onClick={() => isAdmin && onEdit(r)}
          className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent disabled:cursor-default disabled:hover:bg-transparent"
          style={{ paddingLeft: `${8 + depth * 20}px` }}
        >
          <span className="font-medium">{r.name}</span>
          {isSeeded(r) && (
            <Badge variant="secondary" className="font-normal">
              built-in
            </Badge>
          )}
        </button>
        {kids.map((k) => renderNode(k, depth + 1))}
      </div>
    );
  };

  const scopes: AccessRole["scope"][] = ["platform", "workspace"];
  const roots = childrenOf.get(null) ?? [];
  return (
    <div className="space-y-4">
      {scopes.map((scope) => {
        const scopeRoots = roots.filter((r) => r.scope === scope);
        if (scopeRoots.length === 0) return null;
        return (
          <div key={scope} className="space-y-1">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              {scope}
            </h3>
            {scopeRoots.map((r) => renderNode(r, 0))}
          </div>
        );
      })}
    </div>
  );
}
```
Add `import type React from "react";` (or use `ReactNode` from the existing react import) — match the file's react import style; if `React` isn't imported, type `renderNode`'s return as `import("react").ReactNode` or add `ReactNode` to the `import { … } from "react"` line.

- [ ] **Step 2: Add the view-mode toggle + conditional render**

In `RolesPage`, add state: `const [treeView, setTreeView] = useState(false);`. In the `CardHeader`'s action area (next to the "New role" button), add a toggle button:
```tsx
            <Button
              size="sm"
              variant={treeView ? "secondary" : "outline"}
              onClick={() => setTreeView((v) => !v)}
              aria-pressed={treeView}
            >
              {treeView ? "List view" : "Tree view"}
            </Button>
```
In `CardContent`, keep the `SummaryStrip`, and render the tree OR the table:
```tsx
          {treeView ? (
            <>
              <p className="text-xs text-muted-foreground">
                Tree view shows the full hierarchy — switch to list to search or filter.
              </p>
              <RolesTree roles={rows} isAdmin={isAdmin} onEdit={setEditTarget} />
            </>
          ) : (
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(r) => r.id}
              searchPlaceholder="Search roles…"
              initialSort={{ key: "name", dir: "asc" }}
              emptyText={
                roles.isLoading ? "Loading…" : roles.isError ? (roles.error?.message ?? "Failed to load roles.") : "No roles."
              }
              filters={
                <FacetSelect label="Scope" value={scopeFacet} onChange={setScopeFacet}
                  options={[{ value: "platform", label: "Platform" }, { value: "workspace", label: "Workspace" }]} />
              }
            />
          )}
```
(The search box + Scope facet live inside `FilterableTable`, so they naturally disappear in tree mode — satisfying "hidden while Tree view is active".)

- [ ] **Step 3: Typecheck + lint + build**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build`
Expected: clean.

- [ ] **Step 4: Extend the test**

Add: clicking the "Tree view" toggle renders the tree — a child role appears (nested) and the "Search roles…" box is no longer present; toggling back to "List view" restores the table + search. (Mock `listRoles` with a platform parent + a platform child + a workspace role, so both scope sections + a nesting appear.)

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/access/RolesPage.tsx frontend/src/routes/control-plane/access/__tests__/RolesPage.test.tsx
git commit -m "feat(fe): Roles page — Tree-view toggle + indented hierarchy tree"
```

---

## Task 4: Full frontend CI + review + finish

**Files:** none.

- [ ] **Step 1: Frontend sweep** — `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`. Expect all green.
- [ ] **Step 2: Optional live smoke** — open `/control-plane/roles`: the "Inherits from" column shows parents; the clarifier renders; "Tree view" shows the indented hierarchy grouped by scope; editing a workspace role (e.g. `ws_admin`) shows only workspace permission groups (no Engines/Warehouses/Storage/Secrets); the in-dialog search + select-all-per-group work.
- [ ] **Step 3: UX/code review** over the branch diff. Focus: the editor scope-filter is correct (workspace role → workspace perms only; platform role → platform perms; superuser unchanged); no existing behavior lost (create/edit/delete/assignees dialogs, built-in badge, assignee drill-in, admin gating, the AX-2 search/facet/sort in list mode); the tree correctly nests + groups by scope + guards cycles; select-all skips inherited perms; a11y (toggle has aria-pressed, group select-all + tree nodes have aria-labels/are buttons). Fix any issue in-branch.
- [ ] **Step 4: Hand off to `superpowers:finishing-a-development-branch`** → Push + PR → squash-merge `--admin`; update memory.

---

## Self-Review

**Spec coverage:** editor scope-filter → Task 1 Step 1/3 ✓; in-dialog search → Task 1 ✓; select-all-per-group → Task 1 ✓; checkbox affordance (keep shadcn standard, flag if weak) → Task 1 Step 3 note ✓; editor scope line → Task 1 Step 3 ✓; "Inherits from" column → Task 2 ✓; Roles-page clarifier → Task 2 ✓; Tree-view toggle + tree + scope grouping + hide search/facet + node→edit → Task 3 ✓; tests → Tasks 1-3 + 4 ✓; FE-only, no backend ✓; superuser treatment preserved (matrix only renders when `!isSuperuser`, scope line gated `!isSuperuser`) ✓.

**Placeholder scan:** All code is complete. Test steps name the exact scenario + assertions against the existing RolesPage test harness the implementer reuses (open dialog via the row actions menu; mock `listRoles`/`listPermissions`/`getRole`). No `TBD`.

**Type consistency:** `AccessRole`/`PermissionCatalogGroup` come from `@/api/access` unchanged. `shownGroups` derives from `groups` with the same permission shape (adds nothing). `RolesTree` props `{roles, isAdmin, onEdit}` match the call site `<RolesTree roles={rows} isAdmin={isAdmin} onEdit={setEditTarget} />`. `treeView`/`permQuery`/`nameById` are self-consistent. `p.scope` + `role.scope` are both the `"platform"|"workspace"` strings already on the DTOs.
