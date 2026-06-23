# Tier 3b — Date picker, filter value sources, per-card viz override (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-23 · **Branch:** `feat/dashboards-tier3b` (off main)
**Part of:** the dashboard Metabase-parity program ([audit](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Close three remaining Metabase-parity gaps, all **frontend-only** (params live in `dashboards.parameters` jsonb, card overrides in `dashboard_cards.options` jsonb — no backend or migration):

- **A. Date "all-options" picker** — Metabase's unified Relative/Specific date filter widget (egret has separate single/range/relative/month-year/quarter-year operators today).
- **B. Filter value sources** — a parameter's dropdown values can come from a **custom static list** or **another card's column**, not only a distinct-values query.
- **C. Per-card viz override** — edit a card's display/viz settings directly on the dashboard, reusing the chart builder's settings sections; the override is stored on the card and merged over the saved chart's display settings at render.

## Scope

**In:** A (new `all-options` date operator + combined picker), B (`valuesSource` on a parameter + a source picker in filter settings + resolution in the option loader), C (display-only `vizOverride` on a card + a shared `ChartSettingsSections` component + a dashboard viz-settings panel + merge at render + reset).

**Out / deferred:** backend changes of any kind; `does-not-contain` text operator (needs backend `not_like`); date "exclude" sub-mode (Metabase's third date tab — keep Relative + Specific only this tier); value-source "From connected fields" graph traversal (we do explicit card+column only); overriding the **query** per card (vizOverride is display-only — the query stays the saved chart's spec plus dashboard param injection).

## Existing code (verified)

- `frontend/src/api/charts.ts`: `ChartParams { spec: QuerySpec; columnSettings?; conditionalFormatting?; tableState?; cartesian?; pie?; scalar?; progress?; gauge?; funnel?; waterfall?; smartscalar?; boxplot?; sankey?; ... }` (display configs are all the non-`spec` fields). `Chart { viz_type: VizType; params: ChartParams; ... }`. `VizType` enumerated there.
- `frontend/src/components/chart/settings/`: reusable section components — `CartesianDisplaySection`, `PieDisplaySection`, `FormatSection`, `ConditionalFormattingSection`, `TableColumnsSection` (each takes a config + `onChange`). `frontend/src/routes/explore/ChartExplore.tsx` composes them with per-config local state and assembles `params` for save.
- `frontend/src/components/chart/dashboard/DashboardCardView.tsx`: renders `<ChartRenderer vizType={chart.viz_type} params={chart.params} columns rows onValueClick />`. The dashboard already injects param filters into the query via `applyParamsToChart(chart, parameters, values, mappings)` (`params.ts`) BEFORE this render.
- `frontend/src/components/chart/dashboard/FilterPill.tsx` (`FilterControl`), `FilterSettingsPanel.tsx`, `ParameterEditor.tsx` (`OPERATORS`), `params.ts` (`paramToFilters`, `summarize`), `api/useColumnValues.ts`, `DashboardView.tsx` (`ParamOptions` loader). `DashboardParameter` already has `type/operator/widget/default/filteringParameters`.

## Data model (all jsonb, no migration)

```ts
// api/dashboards.ts
export type ParamOperator = ... | "all-options";   // date, added to the existing union

export interface DashboardParameter {
  // ...existing...
  valuesSource?: ParamValuesSource;
}
export interface ParamValuesSource {
  type: "distinct" | "static" | "card";   // default "distinct" (today's behaviour)
  static?: string[];                       // type==="static"
  cardId?: string;                         // type==="card" — the dashboard card to read from
  column?: string;                         // type==="card" — column on that card's chart
}

// DashboardCard.options gains (untyped jsonb today; documented shape):
//   vizOverride?: VizOverride
export interface VizOverride {            // DISPLAY-ONLY — never includes `spec`
  viz_type?: VizType;
  // any display field of ChartParams: cartesian?/pie?/scalar?/progress?/gauge?/funnel?/
  //   waterfall?/smartscalar?/boxplot?/sankey?/columnSettings?/conditionalFormatting?/tableState?/
  //   rows?/cols?/metrics?/showRowTotals?/showColTotals?/condense?
  [k: string]: unknown;
}
```

## A. Date "all-options" picker

- `OPERATORS.date` (in `ParameterEditor.tsx`) gains `"all-options"`. When a date parameter uses it, `FilterPill`'s `FilterControl` renders a new `DateFilterPicker` with two tabs:
  - **Relative**: a direction (Past / Next / Current) + N + unit (days / weeks / months / quarters / years). Encodes to `{ mode: "relative", dir, n, unit }`.
  - **Specific**: an operator (On / Before / After / Between) + one or two date inputs. Encodes to `{ mode: "specific", op, from, to? }`.
- The pill value for `all-options` is a structured object (stored in the param value map as JSON). `params.ts`:
  - `paramToFilters` gains an `all-options` branch: relative → compute `[ge, le]` from `now` (reuse the existing relative-day logic, generalised to weeks/months/quarters/years); specific On → `eq`, Before → `lt`/`le`, After → `gt`/`ge`, Between → `ge`+`le`. Blank/partial → `[]`.
  - `summarize` labels it ("Past 7 days", "After 2026-01-01", "Jan 1 – Mar 31", etc.).
- Existing date operators are unchanged (back-compat). `DateFilterPicker` is a self-contained component with its own tests.

## B. Filter value sources

- `FilterSettingsPanel` gains a "Where do values come from?" control (shown for category / location / id / text widgets): **Distinct** (current — from the mapped card's column) / **Custom list** (a textarea, one value per line → `static`) / **From a card column** (a card `<select>` of the dashboard's chart cards + a column `<select>`).
- `useColumnValues` is unchanged; the **option loader** (`ParamOptions` in `DashboardView.tsx`) resolves the source:
  - `distinct` (or absent) → today's behaviour (distinct-query the param's mapped card/column, with linked filters).
  - `static` → emit `param.valuesSource.static` directly via `onOptionsLoaded(slug, list)` (no query); a small `StaticOptions` helper (no hook) or inline effect.
  - `card` → distinct-query the chosen `cardId`'s chart dataset + `column` (same machinery as distinct, different card/column).
- Linked filters (Tier 2d) continue to apply only to query-backed sources (distinct/card); static ignores them.

## C. Per-card viz override (full editor reuse)

- **Extract** `ChartExplore`'s settings-section composition into a shared, presentational `ChartSettingsSections` component:
  `<ChartSettingsSections vizType params onChange />` where `params: ChartParams` and `onChange(next: ChartParams)`. It renders the right display sections for `vizType` (cartesian/pie/scalar/... + Format / Conditional formatting / Table columns), exactly as `ChartExplore` does today. `ChartExplore` is refactored to render `ChartSettingsSections` instead of inlining the sections — its behaviour must stay identical (guarded by its existing tests). Only display sections move; field-zone/query building stays in `ChartExplore`.
- **Merge:** `applyVizOverride(chart: Chart, override?: VizOverride): { viz_type: VizType; params: ChartParams }` (new, in `params.ts` or a sibling) deep-merges the override's display fields over `chart.params` and overrides `viz_type` if set. It NEVER touches `params.spec` (the query). Order in `DashboardCardView`: the dashboard first injects params (`applyParamsToChart` → query), then `applyVizOverride` (display) → `<ChartRenderer vizType={eff.viz_type} params={eff.params} />`.
- **Dashboard viz panel:** the card edit toolbar gains a "Visualization" action opening a panel (in the edit sidebar) that renders `ChartSettingsSections` bound to the **effective** params (chart.params ⊕ current override), writing changes into `card.options.vizOverride` (only the diff vs the chart defaults is stored; simplest correct approach: store the full edited display params as the override, and a **Reset to chart defaults** button clears `vizOverride`).
- A card with no `vizOverride` renders exactly as today.

## Data flow

```
view render per chart card:
  chart → applyParamsToChart(chart, params, values, mappings)   // inject filters into spec (Tier 1/2d)
        → applyVizOverride(_, card.options.vizOverride)          // display-only merge (Tier 3b-C)
        → <ChartRenderer vizType params />

filter option loader (ParamOptions):
  source = param.valuesSource?.type ?? "distinct"
  distinct → useColumnValues(mapped card/col, linkedFilters)
  card     → useColumnValues(chosen card/col, linkedFilters)
  static   → onOptionsLoaded(slug, param.valuesSource.static)

date all-options value → paramToFilters → ge/le filters (relative or specific)
```

## Error handling

- `vizOverride` for the wrong viz type (e.g. a `pie` override on a card whose chart became cartesian) → `applyVizOverride` merges by field; unrelated display fields are ignored by `ChartRenderer` for the active viz type, so a stale override is inert, not a crash. Changing `viz_type` via override re-renders with that type.
- Value source `card`/`column` pointing at a deleted card or absent column → the option query returns nothing; the dropdown shows "No values" (no crash). `static` with an empty list → empty dropdown.
- `all-options` with a partial value (e.g. Between with one date) → `paramToFilters` emits only the bounds it has (or `[]` if none).
- `ChartSettingsSections` extraction must not change chart-builder behaviour — the existing `ChartExplore`/settings tests are the regression guard.
- A `vizOverride` never includes `spec`; the merge strips/ignores `spec` if somehow present, so dashboard param injection always wins for the query.

## Testing

- **Vitest:**
  - `paramToFilters` + `summarize` for `all-options` (relative all units; specific on/before/after/between; partial → []).
  - `DateFilterPicker` (tab switch; encodings for relative + each specific op).
  - value-source resolution: `ParamOptions`/loader emits static list directly; uses the chosen card/column for `card`; falls back to distinct when absent. `FilterSettingsPanel` writes `valuesSource` (static textarea → array; card+column selects).
  - `applyVizOverride` (display merge; viz_type override; `spec` never touched; no-override identity).
  - `ChartSettingsSections` renders the right sections per viz type and round-trips `onChange`; `ChartExplore` tests stay green (behaviour identical).
  - dashboard viz panel writes `card.options.vizOverride` and Reset clears it; `DashboardCardView` renders the effective params.
- **Live (Playwright MCP, side-by-side Metabase), 0 console errors:** add a date filter with the all-options picker (relative + specific) and filter a card; set a filter's values to a custom list and to another card's column; override a card's viz (e.g. change series colours / axis / viz type) on the dashboard and Reset.
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Conventions

Metabase as the standard · clean-room · TDD · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual (en canonical + vi reference) · squash-merge · branch `feat/dashboards-tier3b`.
