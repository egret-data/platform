# Phase 3d-D1 — Dashboards: click behaviour (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Builds on:** Phase A/B/C (merged PRs #25/#26/#27, `903df67`)

## Goal

Make dashboard cards interactive: clicking a value can **cross-filter** the dashboard
(set a Phase-C parameter), open a **custom URL**, or show the default **drill menu**
("Filter dashboard by …" / "See these records"). The signature Metabase dashboard
interaction — built to the same fidelity bar as A/B/C.

Phase D is decomposed into independent sub-phases; this is **D1 (click behaviour)**.
**D2 (dashboard tabs)** follows in its own spec. Public sharing/embedding is **out of
scope for Phase D** (per product decision — egret is an internal tool).

## Metabase grounding (clean-room)

Read the Metabase source (`data-portal/metabase`, `metabase-types/api/click-behavior.ts`,
`visualizations/click-actions/`):

- Click behaviour is a per-dashcard / per-column blob in `visualization_settings.click_behavior`,
  a union of `type`: `"actionMenu"` (default drill menu), `"crossfilter"` (with a
  `parameterMapping` from the clicked column to dashboard parameters), and `"link"`
  (`linkType: "url"` with a `{{column}}` `linkTemplate`, or to a dashboard/question).
- Cross-filter dispatches `setParameterValue` for each mapped parameter from the clicked
  value. URL links interpolate `{{column}}` from the clicked row. Default falls through to
  the standard drill menu.
- **No backend storage** beyond the dashcard settings blob.

### egret reuse (this is mostly wiring, not new infrastructure)

egret already has the drill primitives used on the chart Explore page:
`components/chart/drill.ts` (`DrillContext { dims: {col,value}[] }`, `drillFilters`,
`tableCellDrill`, `pivotCellDrill`), `DrillMenu` (a popover: "Filter by … / See these
records"), and `DrillRecordsDialog` (runs a filtered query and shows the rows).
Phase C added dashboard `parameters` + `useDashboardParams.setValue` (URL-synced) +
per-card `options.paramMappings`. D1 connects the click → these.

## Scope (this PR)

**In:** per-card click behaviour (`menu` / `crossfilter` / `url`) on **table, pivot, and
ECharts (bar/line/area/scatter/combo/pie)** cards; an edit-mode "On click" config; the
default drill menu + "See these records" on dashboard cards; cross-filter into Phase-C
parameters; custom-URL links with `{{column}}`/`{{value}}` substitution.

**Deferred (named):** link-to-another-dashboard/question, action buttons, per-column
(as opposed to per-card) click behaviour, click behaviour on scalar/gauge/funnel/etc.
(only the dimension-bearing viz types above).

## Data model — no backend change

Per-card, in the existing `dashboard_cards.options` jsonb:

```ts
type ClickBehavior =
  | { type: "menu" }                                    // default (or options.clickBehavior absent)
  | { type: "crossfilter"; parameterId: string }        // click value → set this dashboard parameter
  | { type: "url"; template: string };                  // click → open URL ({{column}} tokens)
// options.clickBehavior?: ClickBehavior
```

## Value-click plumbing (the one piece of new infrastructure)

A click on a card must yield a **`DrillContext`** (`{ dims: {col, value}[] }`), which D1
already knows how to consume. Two render paths:

- **Table / pivot** — `ChartRenderer` already can wire the existing `onCellDrill`
  (table/pivot produce a `DrillContext` via `tableCellDrill`/`pivotCellDrill`). In the
  read-only dashboard renderer this was dropped (Phase A); D1 re-enables it behind a new
  opt-in prop.
- **ECharts charts** — `EChartsRenderer` gains an optional
  `onPointClick?: (p: { name?: string; seriesName?: string; value?: unknown }) => void`
  wired to the echarts instance `chart.on("click", …)`. `ChartRenderer` translates a point
  click into a `DrillContext` using the viz's dimension column: cartesian → `cartesian.xAxis`
  (+ `series` as a 2nd dim when set), pie → `pie.dimension`. The clicked value is the
  category (`params.name`).

`ChartRenderer` gains one optional prop: **`onValueClick?: (ctx: DrillContext) => void`**.
When provided, it wires table/pivot `onCellDrill` and the ECharts `onPointClick` to build a
`DrillContext` and call `onValueClick`. When absent, behaviour is exactly today's (read-only;
ChartExplore and unmapped cards unaffected).

## Click resolution (pure helper)

`resolveClick(clickBehavior, ctx, card, parameters)` decides what a click does
(`components/chart/dashboard/click-behavior.ts`, unit-tested):

- `url` → substitute `{{col}}`/`{{value}}` in `template` from `ctx.dims` (first dim is
  the primary), guard to `^https?://` (reuse the Phase-B safe-URL pattern); returns
  `{ kind: "url", href }`.
- `crossfilter` → find the clicked dim whose column equals the parameter's mapping on this
  card (`card.options.paramMappings[parameterId]`), else the primary dim; returns
  `{ kind: "crossfilter", parameterId, value }`.
- `menu` / unset → returns `{ kind: "menu", ctx }` (open the drill menu).

## Frontend wiring

- **`DashboardCardView`** gains `onValueClick?` and threads it to `ChartRenderer`
  (chart cards only).
- **`DashboardView`** provides the per-card `onValueClick`: it reads the card's
  `options.clickBehavior`, calls `resolveClick`, and:
  - `url` → `window.open(href, "_blank", "noreferrer")`.
  - `crossfilter` → `paramsState.setValue(param.slug, value)` (Phase C) → mapped cards
    refetch.
  - `menu` → opens the existing **`DrillMenu`** at the cursor. The menu's "Filter dashboard
    by {col} is {val}" entries cross-filter any dashboard parameter mapped (Phase C) to that
    column; "See these records" opens **`DrillRecordsDialog`** for the card's dataset +
    `drillFilters(ctx)`.
  - Drill-menu + records-dialog state lives in `DashboardView` (mirrors ChartExplore).
- **Edit mode** — an **"On click"** control per chart card: Menu (default) / Cross-filter
  (→ a parameter `<select>`) / Custom URL (→ a `template` input with a `{{column}}` hint),
  writing `options.clickBehavior` via the existing `updateCardOptions`.

## Data flow

```
value click on a card
  → ChartRenderer (onValueClick) builds DrillContext
  → DashboardView.onValueClick(card, ctx)
       → resolveClick(card.options.clickBehavior, ctx, card, parameters)
            ├ url        → window.open(interpolated href)
            ├ crossfilter→ paramsState.setValue(slug, value)  → mapped cards refetch (Phase C)
            └ menu       → <DrillMenu> → Filter (cross-filter mapped param) | See records (<DrillRecordsDialog>)
```

## Error handling

- A `url` template that resolves to a non-`http(s)` URL → not opened (XSS guard).
- `crossfilter` whose parameter was deleted, or with no resolvable clicked value → no-op.
- Click behaviour only applies to dimension-bearing viz; scalar/gauge/etc. cards ignore clicks.
- A click on a card with no `clickBehavior` and no mappable column → the menu still offers
  "See these records"; "Filter by" entries appear only for columns mapped to a parameter.
- ChartExplore (which passes its own `onCellDrill`, not `onValueClick`) is unchanged.

## Testing

- **Vitest:** `resolveClick` (url interpolation + safe-URL guard; crossfilter value
  resolution via paramMappings vs primary dim; menu fallthrough); `ChartRenderer` calls
  `onValueClick` with the right `DrillContext` for a table cell and for an ECharts point
  (mock the echarts click); `EChartsRenderer` registers the click handler; the edit-mode
  "On click" config writes `options.clickBehavior`; DashboardView cross-filter sets the
  parameter value; the drill menu + records dialog open.
- **Live (manual, Playwright MCP browser — egret has no automated Playwright suite):**
  add two cards sharing a dimension + a parameter mapped to that dimension; set one card's
  click to cross-filter → click a value → the other card refetches filtered + the URL gets
  the slug; set a card to a custom URL → click opens it; default menu → "See these records"
  shows rows. **0 console errors.**
- **Full sweep** before each commit: from `frontend/`,
  `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Out of scope (later / other phases)

Dashboard tabs (D2); link-to-dashboard/question, action buttons, per-column click
behaviour, public sharing/embedding.

## Conventions

Metabase as the standard · clean-room (no AGPL) · TDD · full sweep before each commit ·
configured git identity (never `--author`), footer `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>` · stage explicit paths · squash-merge.
