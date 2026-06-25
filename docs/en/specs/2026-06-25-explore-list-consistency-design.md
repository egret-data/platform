# Explore List Consistency & Polish (Design)

**Status:** Approved
**Date:** 2026-06-25
**Scope:** Frontend only (React/TS). No backend / API / migration changes.
**Sequencing:** This is **Project A** of a two-part effort. **Project B** (per-type, flat
folders/groups for datasets/charts/dashboards — needs a backend table + routes) is a separate
spec/plan done after A.

## Problem

The three Explore list pages are inconsistent, and only one is Metabase-grade:

- **Datasets** (`routes/explore/Datasets.tsx`) is the rich reference: search, type + database
  filters, sortable columns (`Th`), a kebab actions menu (`DatasetActionsMenu`), AlertDialog
  confirm on delete, bulk select, and a real empty state. But it still prints raw timestamps
  (`new Date(updated_at).toLocaleString()`) and shows a bare "Loading…" string.
- **Charts** (`routes/explore/Charts.tsx`) is bare: a plain table, viz type shown as text only
  (no icon), a raw red **Delete** button with **no confirmation**, `toLocaleString` timestamps,
  no search / filter / sort, "Loading…" text.
- **Dashboards** (`routes/explore/Dashboards.tsx`) is in between: it has bookmark + trash +
  duplicate, but no search / sort, inline text action buttons (not a kebab), `toLocaleString`,
  and "Loading…" text. Its **move-to-trash** "Delete" is reversible (fine without a heavy
  confirm); only permanent delete (in Trash) is confirmed via `ConfirmDeleteDialog`.

No page has a real loading state (the whole app has zero skeletons). Timestamps don't use the
`timeAgo`/`fullTimestamp` helper that already exists at `lib/relativeTime.ts` (from the SQL Lab
work). The result: Charts/Dashboards feel unfinished next to Datasets and the Metabase-grade
detail pages (DashboardView, DatasetDetail).

## Goals

Bring Charts and Dashboards up to the Datasets pattern, and lift all three with shared,
consistent building blocks: relative timestamps, a real loading skeleton, viz-type icons on
charts, kebab row-actions, and confirm-on-destroy where the action is irreversible.

## Non-Goals

- **No folders/grouping** — that is Project B (separate spec; per-type, flat, needs backend).
- **No backend/API changes.** Charts has no "duplicate" endpoint, so charts get no Duplicate
  action. Bulk select is **not** added to Charts/Dashboards (Datasets keeps its existing one);
  revisit in B if needed.
- **No responsive overhaul** — cross-cutting, tracked separately.
- **No seed-data cleanup in code** — junk rows ("123" dashboard, "Tab 4") are data, not code;
  optionally deleted via the UI during live verification, not part of this PR.

## Current State (anchors)

- `lib/relativeTime.ts` — `timeAgo(iso)`, `fullTimestamp(iso)` (dayjs). Already shipped.
- `components/chart/chartTypes.ts` — `CHART_TYPES: { key, name, description, icon: LucideIcon }[]`.
- `api/charts.ts` — `Chart { id, name, viz_type, updated_at, … }`; `useCharts(ws)`;
  `useChartMutations(ws) → { create, update, remove }` (no duplicate). `VizType` union.
- `api/dashboards.ts` — `DashboardSummary { id, name, card_count, updated_at, bookmarked, … }`;
  `useDashboards(ws, {trashed})`; `useDashboardMutations → { create, remove, duplicate, trash,
  restore, addBookmark, removeBookmark }`.
- `routes/explore/Datasets.tsx` — the reference (inline `Th`, search box, Select filters,
  `DatasetActionsMenu`, AlertDialog confirm, bulk select). Uses `toLocaleString`.
- `components/dataset/DatasetActionsMenu.tsx` — kebab via `DropdownMenu` + `MoreHorizontal`.
- `components/ui/` — has `dropdown-menu`, `alert-dialog`, `select`, `input`, `checkbox`,
  `table`, etc. **No `skeleton`** primitive yet.

## Design

### New shared building blocks

**`components/ui/skeleton.tsx`** (standard shadcn): `<Skeleton className/>` = a
`animate-pulse rounded-md bg-muted` div. The app's first skeleton primitive.

**`components/explore/RelativeTime.tsx`**: `<RelativeTime iso={string} />` →
`<span title={fullTimestamp(iso)}>{timeAgo(iso)}</span>`. Single source for list timestamps.

**`components/explore/SortableHeader.tsx`**: a generic sortable `<th>`:
`<SortableHeader<K> sortKey label current={{key,dir}} onSort={(k)=>…} />` rendering the label +
an `ArrowUp`/`ArrowDown` when active. Generalises the inline `Th` in Datasets.

**`components/explore/RowActionsMenu.tsx`**: the kebab shell —
`<RowActionsMenu>{children}</RowActionsMenu>` renders a ghost `MoreHorizontal` trigger +
`DropdownMenuContent align="end"`; children are `DropdownMenuItem`s. Resource-specific menus
compose it.

**`components/explore/ListSkeleton.tsx`**: `<ListSkeleton rows={n} cols={n} />` → a table of
`<Skeleton>` cells, used in place of the "Loading…" text on all three lists.

### Charts list (`routes/explore/Charts.tsx`) — rewrite to the Datasets pattern

- Header: title + `+ Chart`.
- Toolbar: search (by name) + a **viz-type filter** `Select` (options = `All types` +
  `CHART_TYPES`), reusing the search box style.
- Table columns: **icon + Name** (icon = `CHART_TYPES.find(viz_type).icon`), Type (label),
  `SortableHeader` on Name / Type / Updated; **Updated via `<RelativeTime>`**.
- Row kebab (`ChartActionsMenu`, composing `RowActionsMenu`): **Open** (→ `/explore/:ws/charts/:id`),
  **Delete** (→ AlertDialog confirm; charts are hard-deleted). Removes the raw red button.
- States: `ListSkeleton` while loading; empty state "No charts yet." with a `+ Chart` CTA;
  filtered-empty "No charts match the current filters."

### Dashboards list (`routes/explore/Dashboards.tsx`) — rewrite, keep trash/bookmark

- Keep the existing **Bookmarks** toggle, **Trash** toggle, and **+ Dashboard** dialog.
- Toolbar: add search (by name).
- Table columns: bookmark star + Name, Cards, Updated; `SortableHeader` on Name / Cards /
  Updated; **Updated via `<RelativeTime>`**.
- Row kebab (`DashboardActionsMenu`, composing `RowActionsMenu`):
  - Normal view: **Open**, **Duplicate** (exists), **Move to Trash** (reversible → no heavy
    confirm; a toast is enough).
  - Trash view: **Restore**, **Delete permanently** (keeps the existing `ConfirmDeleteDialog`
    type-to-confirm).
- States: `ListSkeleton` while loading; empty states for normal / bookmarked / trash (existing
  messages preserved).

### Datasets list (`routes/explore/Datasets.tsx`) — minimal lift

- Swap `new Date(updated_at).toLocaleString()` → `<RelativeTime iso={updated_at} />`.
- Swap the "Loading…" text → `<ListSkeleton>`.
- Refactor the inline `Th` → `SortableHeader`, and `DatasetActionsMenu` to compose
  `RowActionsMenu` (light DRY refactor; behaviour unchanged). Bulk select stays.

### Data flow

Each list owns its own state (query / filters / sort) exactly as Datasets does today — the
shared pieces are presentational/util. No new global state, no context. `RelativeTime` and
`SortableHeader` are pure; `RowActionsMenu` is a layout shell; `ListSkeleton` is static.

### Error handling

Unchanged from current per-page mutation handling (toast on failure). Confirm dialogs gate the
irreversible actions (chart delete, dashboard permanent delete). Move-to-trash and bookmark
stay one-click (reversible).

## Testing

Vitest (jsdom), following existing `routes/explore/__tests__` patterns (mock the api hooks):

- `RelativeTime`: renders `timeAgo` text + `fullTimestamp` title.
- `SortableHeader`: shows the arrow only on the active key; click calls `onSort`; toggles dir.
- `ListSkeleton`: renders the requested row/col count.
- `Charts`: renders rows with viz icon + label; search filters; viz-type filter filters; sort
  toggles; Delete opens the confirm and only deletes after confirming; empty state.
- `Dashboards`: search filters; sort toggles; kebab Open/Duplicate/Move-to-Trash call the right
  mutations; Trash view shows Restore + confirmed permanent delete; bookmark toggle.
- `Datasets`: existing tests still pass; add a check that Updated renders relative text.

Live Playwright verification at the end (stack up): all three lists look consistent, search /
sort / filters work, chart rows show type icons, chart delete asks to confirm, timestamps are
relative with full-time tooltips, loading shows skeletons; 0 new console errors.

## YAGNI cuts

No folders (B), no bulk select on Charts/Dashboards, no chart Duplicate (no API), no responsive
work, no seed cleanup in code.
