# AX-2 Table Tooling + Density Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add search + faceted filters + sort to the Users/Groups/Roles tables and a search box to the Permissions catalog, and replace the oversized 3-KPI StatCardRow blocks with compact one-line summary strips.

**Architecture:** Two small reusable helpers (`FacetSelect`, `SummaryStrip`) plus migrating four access pages onto the existing `FilterableTable` (AX-1). Each page holds its own facet state, filters its rows, and passes them to `FilterableTable` (search + sort). Frontend-only, no backend change.

**Tech Stack:** React 18/TS, shadcn/ui, Tailwind v4, Vitest.

**Spec:** `docs/en/specs/2026-07-12-ax2-table-tooling-design.md`

**Verified facts:**
- `FilterableTable` (`frontend/src/components/explore/FilterableTable.tsx`): props `{ rows: T[], columns: Column<T>[], getId: (row)=>string, searchPlaceholder?, filters?: ReactNode, initialSort?: SortState<string>, emptyText? }`; `Column<T> = { key, label, sortable?, className?, value?: (row)=>string|number, cell: (row)=>ReactNode }`. Search matches any column's `value`; sort via `SortableHeader`; `filters` renders next to the search box.
- Pages (`frontend/src/routes/control-plane/access/`): `UsersPage.tsx` (StatCardRow: Total/Active/Disabled; `<Table>` cols Email/Name/Provider/Status/Roles/Groups/actions; `AccessUser{id,email,name,provider,status,roles[],groups[]}`), `GroupsPage.tsx` (StatCardRow; cols Name/Source/Members/Roles/actions; `AccessGroup{id,name,source,...}`), `RolesPage.tsx` (StatCardRow: Total/Platform/Workspace; cols Scope/Name/Description/Permissions/Assignees/actions; `AccessRole{id,scope,name,description,permissionCount,assigneeCount,parentRoleId}`), `PermissionsPage.tsx` (grouped-card grid, `PermissionCatalogGroup{group, permissions:[{key,description,roles[]}]}` — NO StatCardRow).
- `StatCard`/`StatCardRow` at `@/components/control-plane/StatCard` — KEEP (used elsewhere); AX-2 just stops importing them on these pages.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `frontend/src/components/explore/FacetSelect.tsx` | labeled Select filter control | **create** |
| `frontend/src/components/explore/SummaryStrip.tsx` | one-line count strip | **create** |
| `frontend/src/components/explore/__tests__/FacetSelect.test.tsx` | test | **create** |
| `frontend/src/routes/control-plane/access/UsersPage.tsx` | strip + FilterableTable + Status/Provider facets | modify |
| `frontend/src/routes/control-plane/access/GroupsPage.tsx` | strip + FilterableTable + Source facet | modify |
| `frontend/src/routes/control-plane/access/RolesPage.tsx` | strip + FilterableTable + Scope facet | modify |
| `frontend/src/routes/control-plane/access/PermissionsPage.tsx` | search box over grouped cards | modify |
| `frontend/src/routes/control-plane/access/__tests__/{Users,Groups,Roles,Permissions}Page.test.tsx` | extend tests | modify |

---

## Task 1: `FacetSelect` + `SummaryStrip` helpers

**Files:** Create `frontend/src/components/explore/FacetSelect.tsx`, `frontend/src/components/explore/SummaryStrip.tsx`, `frontend/src/components/explore/__tests__/FacetSelect.test.tsx`.

- [ ] **Step 1: Write `FacetSelect`**

`frontend/src/components/explore/FacetSelect.tsx`:

```tsx
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** Sentinel for "no filter". Radix Select values must be non-empty. */
export const FACET_ALL = "__all__";

export interface FacetOption {
  value: string;
  label: string;
}

/**
 * A labeled dropdown for one table facet. Prepends an "All {label}" option
 * (value `FACET_ALL`). The parent owns the value + does the row filtering.
 */
export function FacetSelect({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: FacetOption[];
}) {
  return (
    <Select value={value || FACET_ALL} onValueChange={onChange}>
      <SelectTrigger className="h-9 w-auto min-w-32" aria-label={label}>
        <span className="text-muted-foreground">{label}:</span>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={FACET_ALL}>All</SelectItem>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
```

- [ ] **Step 2: Write `SummaryStrip`**

`frontend/src/components/explore/SummaryStrip.tsx`:

```tsx
/** A compact one-line "label value · label value" summary (replaces KPI cards). */
export function SummaryStrip({ items }: { items: { label: string; value: number }[] }) {
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
      {items.map((it, i) => (
        <span key={it.label} className="inline-flex items-center gap-1">
          {i > 0 && <span className="mr-1 opacity-40">·</span>}
          <span className="font-medium tabular-nums text-foreground">{it.value}</span>
          {it.label}
        </span>
      ))}
    </div>
  );
}
```

- [ ] **Step 3: Write the FacetSelect test**

`frontend/src/components/explore/__tests__/FacetSelect.test.tsx` (copy the render/interaction harness from a sibling component test that drives a shadcn Select — e.g. how `Members.test.tsx` or `ResourceSelector` tests open a Select in jsdom; `vitest.setup.ts` already polyfills pointer capture):

```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { FacetSelect } from "@/components/explore/FacetSelect";

it("renders the All option plus provided options and calls onChange", async () => {
  const onChange = vi.fn();
  render(
    <FacetSelect
      label="Status"
      value=""
      onChange={onChange}
      options={[{ value: "active", label: "Active" }, { value: "disabled", label: "Disabled" }]}
    />,
  );
  fireEvent.click(screen.getByLabelText("Status"));
  expect(await screen.findByRole("option", { name: "All" })).toBeInTheDocument();
  fireEvent.click(await screen.findByRole("option", { name: "Active" }));
  expect(onChange).toHaveBeenCalledWith("active");
});
```

- [ ] **Step 4: Run + verify**

Run: `cd frontend && npm run test -- FacetSelect && npm run typecheck`
Expected: pass + clean. (If driving the Radix Select via `fireEvent.click` doesn't open it in jsdom, switch the trigger click to `userEvent.click` — match whatever the repo's other Select tests use.)

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/explore/FacetSelect.tsx frontend/src/components/explore/SummaryStrip.tsx frontend/src/components/explore/__tests__/FacetSelect.test.tsx
git commit -m "feat(fe): FacetSelect + SummaryStrip table-tooling helpers"
```

---

## Task 2: Users page — summary strip + FilterableTable + facets

**Files:** Modify `frontend/src/routes/control-plane/access/UsersPage.tsx` + its test.

Read the current `UsersPage.tsx` first. The page body currently: a `StatCardRow` (Total/Active/Disabled) + a raw `<Table>` with columns Email/Name/Provider/Status/Roles/Groups/actions and a `⋯` `DropdownMenu` (View access, Assign roles [admin], Edit). Keep every dialog (`CreateUserDialog`/`EditUserDialog`/`AssignRolesDialog`) and every cell renderer — only relocate the table into `FilterableTable` + add filters + swap the StatCardRow.

- [ ] **Step 1: Add imports + facet state**

Add imports:
```tsx
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
```
Remove the `StatCard, StatCardRow` import and the raw `Table*` imports if no longer used (check — the dialogs don't use Table). In the component, add:
```tsx
  const [statusFacet, setStatusFacet] = useState(FACET_ALL);
  const [providerFacet, setProviderFacet] = useState(FACET_ALL);
```

- [ ] **Step 2: Define columns (relocate existing cells)**

Below the component's data hooks, define `Column<AccessUser>[]` — move each current `<TableCell>`'s JSX into the matching `cell`, and give searchable/sortable columns a `value`:
```tsx
  const columns: Column<AccessUser>[] = [
    { key: "email", label: "Email", sortable: true, className: "font-medium",
      value: (u) => u.email, cell: (u) => u.email },
    { key: "name", label: "Name", sortable: true, value: (u) => u.name, cell: (u) => u.name },
    { key: "provider", label: "Provider",
      cell: (u) => <Badge variant="outline">{u.provider}</Badge> },
    { key: "status", label: "Status",
      cell: (u) => <Badge variant={u.status === "active" ? "default" : "secondary"}>{u.status}</Badge> },
    { key: "roles", label: "Roles",
      cell: (u) => u.roles.length === 0 ? <span className="text-muted-foreground">—</span>
        : <div className="flex flex-wrap gap-1">{u.roles.map((r) => <Badge key={r} variant="secondary">{r}</Badge>)}</div> },
    { key: "groups", label: "Groups",
      cell: (u) => u.groups.length === 0 ? <span className="text-muted-foreground">—</span>
        : <div className="flex flex-wrap gap-1">{u.groups.map((g) => <Badge key={g} variant="outline">{g}</Badge>)}</div> },
    { key: "actions", label: "", className: "text-right", cell: (u) => (
        /* MOVE the existing row `⋯` DropdownMenu JSX here verbatim (View access / Assign roles / Edit),
           using `u` as the row var and the existing `navigate`/`isAdmin`/`setEditTarget`/`setRolesTarget`. */
    ) },
  ];
```
(Copy the actions `DropdownMenu` block from the current row exactly — it already references `u`, `navigate`, `isAdmin`, the dialog setters.)

- [ ] **Step 3: Compute filtered rows + summary**

```tsx
  const filtered = rows.filter((u) =>
    (statusFacet === FACET_ALL || u.status === statusFacet) &&
    (providerFacet === FACET_ALL || u.provider === providerFacet),
  );
  const total = rows.length;
  const active = rows.filter((u) => u.status === "active").length;
```

- [ ] **Step 4: Replace the card body (StatCardRow + Table) with strip + FilterableTable**

Inside the `<Card><CardContent>` (keep the CardHeader with the "New user" button), replace the `StatCardRow{...}` + `<Table>…</Table>` with:
```tsx
          <SummaryStrip items={[
            { label: "users", value: total },
            { label: "active", value: active },
            { label: "disabled", value: total - active },
          ]} />
          <FilterableTable
            rows={filtered}
            columns={columns}
            getId={(u) => u.id}
            searchPlaceholder="Search users…"
            initialSort={{ key: "email", dir: "asc" }}
            emptyText={users.isLoading ? "Loading…" : users.isError ? (users.error?.message ?? "Failed to load users.") : "No users."}
            filters={
              <>
                <FacetSelect label="Status" value={statusFacet} onChange={setStatusFacet}
                  options={[{ value: "active", label: "Active" }, { value: "disabled", label: "Disabled" }]} />
                <FacetSelect label="Provider" value={providerFacet} onChange={setProviderFacet}
                  options={[{ value: "local", label: "Local" }, { value: "sso", label: "SSO" }]} />
              </>
            }
          />
```
(Verify the real provider values — grep `provider` in the AccessUser type / the API; if they're e.g. `"local"`/`"oidc"` not `"sso"`, use the real values. Match the DB/DTO.)

- [ ] **Step 5: Typecheck + lint + test the page**

Run: `cd frontend && npm run typecheck && npm run lint && npm run test -- UsersPage`
Expected: clean; existing UsersPage tests still pass (they were wrapped in MemoryRouter in AX-3 — keep that).

- [ ] **Step 6: Extend the test**

In `__tests__/UsersPage.test.tsx`, add assertions: typing in the search box (`getByPlaceholderText("Search users…")`) narrows rows; the Status facet filters to active-only; the summary strip shows the total count. Run `npm run test -- UsersPage` → pass.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/routes/control-plane/access/UsersPage.tsx frontend/src/routes/control-plane/access/__tests__/UsersPage.test.tsx
git commit -m "feat(fe): Users page — summary strip + FilterableTable + status/provider facets"
```

---

## Task 3: Groups page — summary strip + FilterableTable + Source facet

**Files:** Modify `frontend/src/routes/control-plane/access/GroupsPage.tsx` + its test.

Read the current `GroupsPage.tsx`. Same migration as Users: StatCardRow → SummaryStrip, `<Table>` (cols Name/Source/Members/Roles/actions) → FilterableTable, add a Source facet. `AccessGroup` has a `source` field (`"local"`/`"oidc"`). Keep all dialogs (create/rename/members/roles/delete) + the OIDC read-only handling + the "View access" deep-link.

- [ ] **Step 1: Imports + facet state** — add the three helper imports (as Task 2 Step 1); remove `StatCard/StatCardRow`; add `const [sourceFacet, setSourceFacet] = useState(FACET_ALL);`.

- [ ] **Step 2: Columns** — define `Column<AccessGroup>[]` relocating the current cells: Name (`sortable`, `value: g=>g.name`), Source (badge — copy current cell, incl. the OIDC tooltip), Members (count cell), Roles (chips cell), actions (`className:"text-right"`, copy the `⋯` menu verbatim). Give Name `value` for search/sort.

- [ ] **Step 3: Filter + summary**
```tsx
  const filtered = rows.filter((g) => sourceFacet === FACET_ALL || g.source === sourceFacet);
```
SummaryStrip items: `{ label: "groups", value: rows.length }` (add a member-sum item only if a member count is present on `AccessGroup` without an extra fetch; otherwise just the group count).

- [ ] **Step 4: Replace body** — swap StatCardRow + Table for `<SummaryStrip .../>` + `<FilterableTable rows={filtered} columns={columns} getId={g=>g.id} searchPlaceholder="Search groups…" initialSort={{key:"name",dir:"asc"}} emptyText={…loading/error/"No groups."} filters={<FacetSelect label="Source" value={sourceFacet} onChange={setSourceFacet} options={[{value:"local",label:"Local"},{value:"oidc",label:"OIDC"}]} />} />`. (Confirm the real `source` values from `AccessGroup`.)

- [ ] **Step 5: Typecheck + lint + test** — `cd frontend && npm run typecheck && npm run lint && npm run test -- GroupsPage`. Extend the test: search narrows by name; the summary count renders.

- [ ] **Step 6: Commit**
```bash
git add frontend/src/routes/control-plane/access/GroupsPage.tsx frontend/src/routes/control-plane/access/__tests__/GroupsPage.test.tsx
git commit -m "feat(fe): Groups page — summary strip + FilterableTable + source facet"
```

---

## Task 4: Roles page — summary strip + FilterableTable + Scope facet

**Files:** Modify `frontend/src/routes/control-plane/access/RolesPage.tsx` + its test.

Read the current `RolesPage.tsx`. Migration: StatCardRow (Total/Platform/Workspace) → SummaryStrip, `<Table>` (cols Scope/Name/Description/Permissions/Assignees/actions) → FilterableTable, add a Scope facet. Keep the create/edit/delete/assignees dialogs, the built-in badge, the assignee drill-in `Button`, and the admin-only actions.

- [ ] **Step 1: Imports + facet state** — helper imports; remove `StatCard/StatCardRow`; `const [scopeFacet, setScopeFacet] = useState(FACET_ALL);`.

- [ ] **Step 2: Columns** — `Column<AccessRole>[]`: Scope (badge cell), Name (`sortable`, `value: r=>r.name`; keep the built-in badge next to the name), Description (`value: r=>r.description`, muted cell), Permissions (the `permissionCount` badge cell), Assignees (the assignee-count `Button`/badge drill-in cell — copy verbatim), actions (`text-right`, admin-gated `⋯` menu verbatim). Give Name + Description `value` for search.

- [ ] **Step 3: Filter + summary**
```tsx
  const filtered = rows.filter((r) => scopeFacet === FACET_ALL || r.scope === scopeFacet);
  const platform = rows.filter((r) => r.scope === "platform").length;
```
SummaryStrip items: `{label:"roles",value:rows.length}`, `{label:"platform",value:platform}`, `{label:"workspace",value:rows.length-platform}`.

- [ ] **Step 4: Replace body** — keep the `deleteError` Alert; swap StatCardRow + Table for `<SummaryStrip.../>` + `<FilterableTable rows={filtered} columns={columns} getId={r=>r.id} searchPlaceholder="Search roles…" initialSort={{key:"name",dir:"asc"}} emptyText={…} filters={<FacetSelect label="Scope" value={scopeFacet} onChange={setScopeFacet} options={[{value:"platform",label:"Platform"},{value:"workspace",label:"Workspace"}]} />} />`.

- [ ] **Step 5: Typecheck + lint + test** — `cd frontend && npm run typecheck && npm run lint && npm run test -- RolesPage`. Extend the test: the Scope facet filters to platform-only; search narrows by name; summary shows total/platform/workspace; the built-in badge + assignee drill-in still render.

- [ ] **Step 6: Commit**
```bash
git add frontend/src/routes/control-plane/access/RolesPage.tsx frontend/src/routes/control-plane/access/__tests__/RolesPage.test.tsx
git commit -m "feat(fe): Roles page — summary strip + FilterableTable + scope facet"
```

---

## Task 5: Permissions page — search box over grouped cards

**Files:** Modify `frontend/src/routes/control-plane/access/PermissionsPage.tsx` + its test (create the test if none exists).

The page is a `lg:grid-cols-2` grid of `ConfigSection` cards, one per `PermissionCatalogGroup`, each a `<Table>` of `{key, description, roles[]}`. Keep the layout; add a search `Input` that filters which permissions render; hide a group whose permissions all filter out.

- [ ] **Step 1: Add search state + filter**

Add imports:
```tsx
import { useState } from "react";
import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
```
In the component:
```tsx
  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  const filteredGroups = groups
    .map((g) => ({
      ...g,
      permissions: needle
        ? g.permissions.filter(
            (p) => p.key.toLowerCase().includes(needle) || p.description.toLowerCase().includes(needle),
          )
        : g.permissions,
    }))
    .filter((g) => g.permissions.length > 0);
```

- [ ] **Step 2: Render the search box + use `filteredGroups`**

Between the page header `<div>` and the grid, add (only when there is data):
```tsx
      {!permissions.isLoading && !permissions.isError && groups.length > 0 && (
        <div className="relative max-w-xs">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input className="pl-8" placeholder="Search permissions…" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
      )}
```
Change the grid to map over `filteredGroups` instead of `groups`. When `filteredGroups.length === 0` (a search with no matches), render `<p className="text-sm text-muted-foreground">No permissions match "{q}".</p>` in place of the grid.

- [ ] **Step 3: Typecheck + lint**

Run: `cd frontend && npm run typecheck && npm run lint`
Expected: clean.

- [ ] **Step 4: Test**

Create/extend `__tests__/PermissionsPage.test.tsx` (copy a sibling access test harness — QueryClientProvider, `vi.mock("@/api/access", …)` returning a couple of `PermissionCatalogGroup`s). Assert: all permissions render initially; typing a permission key in "Search permissions…" narrows to matching permission(s) and hides non-matching groups; clearing restores all. Run `npm run test -- PermissionsPage` → pass.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/access/PermissionsPage.tsx frontend/src/routes/control-plane/access/__tests__/PermissionsPage.test.tsx
git commit -m "feat(fe): Permissions page — search across grouped permission cards"
```

---

## Task 6: Full frontend CI + review + finish

**Files:** none.

- [ ] **Step 1: Frontend sweep** — `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`. Expect all green (all four page tests + FacetSelect + the rest of the suite).
- [ ] **Step 2: Optional live smoke** — with the stack running, open `/control-plane/users`, `/roles`, `/groups`, `/permissions`: the summary strips show counts, search + facets filter, sort works, and every existing action (create/edit/assign/delete, "View access", assignee drill-in) still works.
- [ ] **Step 3: UX/code review** over the branch diff. Focus: no existing behavior/dialog/action was dropped in the table migration; the facet `value`s match the real DTO field values (provider/source/scope); `StatCard`/`StatCardRow` are still exported (only unused on these pages), not deleted; a11y — the FacetSelect has an aria-label, the search inputs are labeled/placeholdered. Fix any issue in-branch.
- [ ] **Step 4: Hand off to `superpowers:finishing-a-development-branch`** → Push + PR → squash-merge `--admin`; update memory.

---

## Self-Review

**Spec coverage:** FacetSelect + SummaryStrip → Task 1 ✓; Users (strip + FilterableTable + Status/Provider) → Task 2 ✓; Groups (strip + FilterableTable + Source) → Task 3 ✓; Roles (strip + FilterableTable + Scope) → Task 4 ✓; Permissions (search over grouped cards, layout kept) → Task 5 ✓; StatCard kept → Task 6 review ✓; tests per page → Tasks 1-5 ✓; FE-only (no backend) ✓.

**Placeholder scan:** Task 1's helper code is complete. Tasks 2-4 give complete new code (imports, facet state, filter, summary, FilterableTable invocation) and instruct relocating the EXISTING cell/actions JSX (which already lives verbatim in each page) into `Column.cell` — a mechanical move, not a placeholder; the columns list + facet options are spelled out. Each task flags "verify the real provider/source/scope values" because those must match the DTO. Task 5 is fully coded. No `TBD`.

**Type consistency:** `FacetSelect`/`FACET_ALL`/`FacetOption` and `SummaryStrip` are defined in Task 1 and used identically in Tasks 2-5. `Column<T>` comes from `FilterableTable` (AX-1). Facet state is a `string` compared to `FACET_ALL` on every page. Row id accessor is `(x)=>x.id` everywhere.
