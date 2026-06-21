# Phase 3d — Dashboards (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-21 · **Builds on:** 3c-3.8 ECharts viz + the chart-parity work (Table/Pivot/Charts, merged PRs #22–#24)

## Goal

Add **dashboards** to egret: a grid of saved charts (and, in later phases, text
cards and interactive filters) that lets a workspace assemble many questions onto
one canvas — matching Metabase closely in both behaviour and look, the same
fidelity bar we held for Table, Pivot, and Charts.

## Why match Metabase

Metabase's dashboard is a fixed **24-column grid** of *dashcards*. Each dashcard
references a saved question and is positioned by `(col, row, size_x, size_y)` in
grid units. Editing is an explicit mode (pencil → drag/resize/add/remove → Save);
viewing runs every card's query independently and renders it. Dashboard-level
**parameters/filters** wire a widget at the top to a field in each card's query.
We reproduce this model so the UX is familiar and the data model is a clean
superset we can grow into filters, tabs, and sharing.

This is a **clean-room** reimplementation: we copy behaviour and visual style,
never Metabase's AGPL source. egret stays Apache-2.0.

## Scope (phased — one PR per phase)

The feature is large, so it ships in four phases. **This spec covers the full
A→D design; Phase A is the deliverable of the first PR.** Later phases get their
own implementation plans built on this same data model.

- **Phase A — Core (this PR):** dashboard CRUD, a 24-column grid of *question
  cards* (each a saved chart), view mode (run + render every card) and edit mode
  (drag / resize / add from saved charts / remove / save layout).
- **Phase B — Card variety + polish:** text / heading / link cards (markdown),
  per-card title, duplicate / replace card, auto-refresh interval, fullscreen,
  night mode.
- **Phase C — Filters / parameters:** dashboard parameter widgets (date / text /
  number / id), map a parameter to a column in each card's query, inject as a
  SQL filter at run time, linked filters, required / locked.
- **Phase D — Advanced:** click behaviour (cross-filter, drill-through, custom
  URL), dashboard tabs, public link / embed.

### Out of scope for Phase A (named so reviewers know it is deliberate)

Text / heading / link cards, dashboard filters, auto-refresh, fullscreen, click
behaviour, tabs, public sharing, revision history. The data model is shaped to
accept them later without migration churn (`card_type`, `options` jsonb).

## Architecture

### Backend (already built in this PR)

Mirrors the existing `chart` slice in `egret-bi` exactly.

**Migration `0015_dashboard.sql`:**

```sql
CREATE TABLE dashboards (
    id, workspace_id, name, description, created_by, updated_at,
    UNIQUE (workspace_id, name)
);
CREATE TABLE dashboard_cards (
    id, dashboard_id, chart_id?,        -- chart_id nullable (Phase B text cards)
    card_type DEFAULT 'chart',
    grid_col, grid_row, size_x, size_y, -- 24-col grid units
    options jsonb DEFAULT '{}',         -- text body, overrides, click behaviour (later)
    ordinal                             -- placement order
);
-- dashboard:read / dashboard:manage granted to ws_editor + ws_admin
```

`grid_col` / `grid_row` (not `col` / `row`) because `row` is a reserved word in
Postgres.

**`DashboardRepo`** (`dashboard.rs`): `create`, `get`, `list_by_workspace`
(returns each dashboard with its card count), `update_meta`, `delete`, `cards`,
and **`replace_cards`** — the whole-layout save: in one transaction it deletes
the dashboard's cards and re-inserts the supplied set in order. Plus
`dashboard_workspace` and `owner` for the guards.

**Save model: whole-layout PUT.** The client always sends the *full* card set on
save; the server swaps it atomically. This keeps grid edits transactional and
avoids per-card diffing — exactly how a layout editor wants to persist.

**Routes** (`dashboard_router`): `GET/POST /ws/:w/dashboards`,
`GET/PUT/DELETE /ws/:w/dashboards/:id`. Guards, in order: workspace permission
(`dashboard:read` for GET, `dashboard:manage` for mutations) → dashboard belongs
to the path workspace → owner-or-admin for mutations → **every `chart_id` in the
incoming cards belongs to the same workspace** (else it is a cross-workspace
embed leak — the lesson from the chart nested-ownership review). `GET` and `PUT`
return a `DashboardDetailDto` (dashboard + ordered cards); list returns
`DashboardSummaryDto[]` (with `card_count`).

### Frontend — card data fetching (decision: client-side, reuse the pipeline)

A dashboard card runs the **existing** chart pipeline, no new backend endpoint:
`build-chart-data` (dataset + query spec → SQL + database_id) → `useSubmitQuery`
→ poll `useQueryStatus` / `useResults` until done → `(columns, rows)`. Each card
fetches **independently and in parallel**, with its own loading / error /
empty state, mirroring Metabase. This reuses everything the chart Explore page
already does and adds zero server surface for Phase A.

### Frontend — the reusable renderer refactor (prerequisite)

Today every visualization is rendered **inline** inside `ChartExplore.tsx` (a
54 KB file with a big switch building `EChartsRenderer` / `ChartTable` /
`BigNumber` options). A dashboard card needs the same rendering, so Phase A first
extracts:

- **`<ChartRenderer chart={ChartLike} columns rows />`** — a pure presentational
  component: given a chart's `viz_type` + `params` and a `(columns, rows)` result,
  it renders the correct visualization. This is the switch lifted verbatim out of
  `ChartExplore`, depending only on its inputs.
- **`useChartData(ws, chart)`** — the build→submit→poll hook returning
  `{ columns, rows, isLoading, error }` for a saved chart.

`ChartExplore` is refactored to consume `ChartRenderer` so its behaviour is
unchanged and its existing tests stay green. This keeps each unit focused: the
renderer knows nothing about data fetching, the hook knows nothing about
rendering, and both are testable in isolation.

### Frontend — pages

- **`api/dashboards.ts`** — `Dashboard` / `DashboardCard` types + react-query
  hooks (`useDashboards`, `useDashboard`, `useDashboardMutations`), mirroring
  `api/charts.ts`.
- **`Dashboards.tsx`** — list page (mirror `Charts.tsx`): name (link), card
  count, updated-at, **+ Dashboard**, delete. Replaces the current placeholder.
- **`DashboardView.tsx`** — the grid, using **react-grid-layout** (24 columns,
  Metabase-like row height), with two modes:
  - **View mode:** each card → `useChartData` + `<ChartRenderer>`; per-card
    loading / error; cards are not draggable.
  - **Edit mode** (pencil toggle, like Metabase): cards draggable / resizable; an
    **"Add a chart"** sidebar lists saved charts to drop onto the grid; remove
    card; **Save** (PUT the full layout) / **Cancel** (discard, refetch).

react-grid-layout is the de-facto dashboard-grid library and what Metabase itself
uses; it gives drag, resize, and collision out of the box. React 18 compatible.

**Grid sizing (Metabase's constants).** 24 columns
(`GRID_WIDTH = 24`). Row height is derived from width, not fixed:
`rowHeight = max(width / 24 / (10/9), 40px)` — a `10/9` cell aspect ratio with a
`40px` minimum (`GRID_ASPECT_RATIO`, `MIN_ROW_HEIGHT`). Cards have a per-viz
**minimum size** (e.g. a scalar can be small, a table needs more) and a
sensible **default size** when first dropped (Metabase picks a default per
display type). Phase A uses one default (e.g. 6×4) and a global minimum;
per-viz minimums can follow.

**Validated against Metabase + Superset (2026-06-21).** Metabase confirms the
model: 24-col grid, flat `{col, row, size_x, size_y}` dashcards, whole-dashboard
PUT, parallel per-card fetch (one query request per dashcard, no batch),
`react-grid-layout`, explicit edit mode. Superset takes a different shape — a
nested component **tree** (`ROW`/`COLUMN`/`TABS` containing `CHART`/`MARKDOWN`),
12 columns, `react-dnd`, a whole-document `position_json` PUT — which confirms
whole-layout persistence and independent per-chart fetch are industry-normal
while showing the tree model is heavier than we need. We stay flat (Metabase
style) for Phase A; Superset's **native-filter scoping** (`targets` +
`chartsInScope`) informs Phase C and its first-class **tabs** inform Phase D.

## Data flow

```
DashboardView (view)                DashboardView (edit)
  └─ useDashboard(ws, id)             ├─ local layout state (RGL onLayoutChange)
       └─ cards[]                     ├─ Add-chart sidebar → useCharts(ws)
            └─ per card:              └─ Save → useDashboardMutations.update
                 useChartData(ws,         → PUT { name, cards: [{chart_id,
                   chartOfCard)               grid_col,grid_row,size_x,size_y}] }
                 → <ChartRenderer>
```

## Error handling

- Card whose chart was deleted, or whose query errors: the card renders an inline
  error tile, never breaks the rest of the grid.
- Duplicate dashboard name in a workspace → `409 Conflict` (unique constraint),
  surfaced as a toast.
- Cross-workspace `chart_id` in a save → `404` (guard), surfaced as a toast.
- Non-owner / non-admin editing → `403`.

## Testing

- **Backend:** `dashboard_repo.rs` (DB-gated, self-skips without `DATABASE_URL`)
  covers CRUD, `replace_cards` full-swap, card count, cascade delete, unique
  conflict; `http.rs` adds an unauthenticated-rejected smoke test. *(Done.)*
- **Frontend (Vitest):** `ChartRenderer` renders each viz family from
  `(columns, rows)`; `useChartData` drives the pipeline (mocked api); the
  dashboard grid maps cards ↔ layout and the editor builds the correct save
  payload.
- **Live (Playwright):** create → edit → add charts → drag / resize → save →
  reload shows the persisted layout with cards rendering data; 0 console errors.
- **Full sweep before every commit:** from `frontend/`,
  `npm run typecheck && npm run lint && npm run build && npm test`.

## Conventions

Metabase as the standard for every decision · clean-room (no AGPL code) · TDD ·
full sweep before each commit · commit with the configured identity (never
`--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · stage explicit paths only · squash-merge the PR.
