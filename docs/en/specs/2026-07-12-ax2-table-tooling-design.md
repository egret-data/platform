# AX-2 — Table Tooling + Density — Design

**Status:** Approved
**Date:** 2026-07-12
**Initiative:** Access-Control UX uplift (increment 3 of 5) — see
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

## Goal

Make every Access-Control list fast to scan and search, and deliver on the
"data-dense dashboard" promise the pages currently under-serve. Add search +
faceted filters + column sort to the Users, Groups, and Roles tables, add a search
box to the Permissions catalog, and replace the oversized three-KPI `StatCardRow`
blocks (which show tiny numbers in big cards and leave ~40% of the page empty) with
compact one-line summary strips. Frontend-only — no backend change.

## Motivation / current state

- **Users / Groups / Roles** pages each render a `StatCardRow` (three large KPI cards
  for small counts) followed by a raw shadcn `<Table>` with **no search, filter, or
  sort**. The design-system's explicit anti-pattern is "no filtering"; at scale these
  tables are unusable, and the big KPI cards read as filler.
- **Permissions** page is a dense two-column grid of grouped permission cards (already
  the density reference — no KPI cards) but has **no search** across its ~40 permissions.
- `FilterableTable` v1 (built in AX-1, `frontend/src/components/explore/FilterableTable.tsx`)
  already provides search + sortable columns + a `filters?: ReactNode` slot over a
  `Column<T> = { key, label, sortable?, className?, value?, cell }` API. AX-2 generalizes
  its use and adds a standard faceted-filter control.

## Design

### 1. `FacetSelect` — a reusable filter control

Add a small `FacetSelect` component (co-located with `FilterableTable`, e.g.
`frontend/src/components/explore/FacetSelect.tsx`) — a labeled shadcn `Select` for one
facet:

```
FacetSelect({ label, value, onChange, options }: {
  label: string;
  value: string;                       // the active value; "" or "__all__" = no filter
  onChange: (v: string) => void;
  options: { value: string; label: string }[];  // excludes the "All" sentinel, which the component prepends
})
```

**Filtering stays in the page, not in `FilterableTable`.** Each page holds its facet
state, filters its `rows` array (facet + any page-specific predicate), and passes the
already-facet-filtered rows to `FilterableTable`, which layers search + sort on top.
This keeps `FilterableTable` a simple, single-responsibility unit (search + sort only)
and puts the domain filter logic where the domain data lives. The `FacetSelect`
controls are rendered into `FilterableTable`'s existing `filters` slot.

### 2. `SummaryStrip` — compact counts

Add a tiny `SummaryStrip` helper (co-located, e.g. in `FilterableTable.tsx` or a small
`summary.tsx`): renders a one-line `text-sm text-muted-foreground` list of `label: value`
pairs (value in `font-medium text-foreground tabular-nums`), separated by middots —
mirroring the Members page summary strip from AX-1. Replaces the `StatCardRow` blocks.

### 3. Per-page application

| Page | Summary strip | Search | Facet filters | Sortable columns |
|---|---|---|---|---|
| **Users** | "N users · N active · N disabled" | email / name | **Status** (active/disabled) · **Provider** (local/sso) | Email, Name |
| **Groups** | "N groups" (+ member count if cheaply available) | group name | — (Groups are simple; no facet unless a `source` field is present, then Source local/oidc) | Name |
| **Roles** | "N roles · N platform · N workspace" | name / description | **Scope** (platform/workspace) | Name |
| **Permissions** | *(unchanged — no KPI cards)* | key / description (filters the grouped cards) | *(none — keep the grouped-card layout; only add search)* | *(unchanged)* |

- **Users / Groups / Roles:** replace `StatCardRow` with `SummaryStrip`; replace the raw
  `<Table>` with `<FilterableTable>` (map the existing columns to `Column<T>`, preserving
  every existing cell renderer — badges, the roles/groups chips, the actions `⋯` menu,
  the assignee drill-in, etc.); add the facet `FacetSelect`s into the `filters` slot and
  apply them in the page's row filter. Preserve ALL existing behavior (create/edit/assign
  dialogs, delete, the "View access" deep-link from AX-3, the last-admin guard display).
- **Permissions:** keep the two-column grouped-card grid; add a single search `Input` at
  the top that filters which permissions render (match on `key` or `description`, case-
  insensitive); a group whose permissions all filter out is hidden. No summary strip, no
  facets, no layout change otherwise.

### 4. Consistency

`StatCard` / `StatCardRow` remain in the codebase (used elsewhere, e.g. control-plane
Engines/Warehouses per prior work) — AX-2 only stops using them on the four access pages.
Do not delete them.

## Testing

**Frontend (Vitest)** — per page:

- **Users:** typing in search narrows the rows; the Status facet filters to active/disabled;
  the Provider facet filters to local/sso; the summary strip shows the correct counts;
  a sortable column reorders. Existing behavior (open create/edit, "View access" item)
  still works.
- **Groups:** search narrows by name; summary strip count correct.
- **Roles:** search narrows; the Scope facet filters platform/workspace; summary strip
  shows total/platform/workspace; built-in badge + assignee drill-in still render.
- **Permissions:** the search box filters permissions across groups; a group with no
  matches is hidden; clearing search restores all.
- **`FacetSelect`:** renders the "All" option + the provided options; selecting calls
  `onChange`.

## Out of scope

- **AX-4** — surfacing the role hierarchy on the Roles table, the role-editor scope
  filter, and scope clarifiers.
- **AX-5** — bulk operations + undo.
- Converting the Permissions page to a flat `FilterableTable` (its grouped-card grid is
  the density reference — keep it).
- Any backend change (this is purely presentational).

## Notes

- Reuse: `FilterableTable` + its `Column<T>`/`filters` slot (AX-1), shadcn `Select`/`Input`,
  the Members page summary-strip pattern (AX-1) as the visual reference.
- Keep each page's diff focused: swap the container (StatCardRow→SummaryStrip, Table→
  FilterableTable) and add filter state; do not restructure the dialogs or data hooks.
