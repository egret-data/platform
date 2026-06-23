# Tier 2d — Filter UX overhaul (Metabase-style) (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Branch:** `feat/dashboards-tier2d-filter-ux` (off main)
**Part of:** the dashboard Metabase-parity program ([audit](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Bring egret's dashboard **filters** to Metabase's UX, found by walking Metabase's filter flow:
map a filter to cards by picking the column **on each card**, choose the value **widget**, set a
**default**, **link filters** (chain), and support more **parameter types**. Frontend-only (params
live in `dashboards.parameters` jsonb — no backend schema change).

## Scope

**In:** (A) in-card column mapping while a filter is selected; (B) value widget (dropdown / search /
input); (C) default-value editor; (D) **linked filters** (chain-filter on the option query);
(E) new types/operators: **boolean**, **location**, date **month-year** / **quarter-year**.

**Deferred (named, with reason):** text **does-not-contain** (needs a backend `not_like` op — egret's
`ChartFilter` only has `like`); date **all-options** (Metabase's combined picker — egret keeps the
existing single/range/relative); per-card viz-settings override; link-to-dashboard/question. No
backend change in this tier.

## Data model (`api/dashboards.ts` — jsonb, no migration)

```ts
export type ParamType = "text" | "number" | "date" | "id" | "category" | "boolean" | "location";
export type ParamOperator =
  | "is" | "is-not" | "contains" | "starts-with" | "ends-with"   // text/category/location
  | "=" | "!=" | "between" | ">=" | "<="                          // number
  | "range" | "relative" | "single" | "month-year" | "quarter-year"; // date
export type ParamWidget = "dropdown" | "search" | "input";
export interface DashboardParameter {
  id; name; slug; type: ParamType; operator: ParamOperator;
  default?: unknown;
  required?: boolean;
  widget?: ParamWidget;            // how the value is picked (view mode)
  filteringParameters?: string[];  // ids of params whose value filters THIS param's options
}
```

`OPERATORS` (in `ParameterEditor.tsx`) gains: `boolean: ["is"]`, `location: ["is","is-not"]`,
and date gains `"month-year"`, `"quarter-year"`. (`does-not-contain` is intentionally absent.)

## A. In-card column mapping during filter edit (the Metabase flow)

When `editPanel.kind === "filter"` (a filter selected in the sidebar), each **chart** card renders
an overlay over its body: a label "Column to filter on" + a `<select>` of the chart's columns
(`dimensions + columns`), value = `card.options.paramMappings[activeParamId] ?? ""`, `onChange`
→ `updateCardOptions(card.id, { paramMappings: { ...m, [paramId]: col } })`. The chart shows dimmed
behind the overlay. This is how a filter is mapped to cards (filter-centric, like Metabase).

Consequently, **remove** the per-parameter mapping section from `CardSettingsPanel` (mapping is now
done via the filter flow). `CardSettingsPanel` keeps Title / On-click / Move-to-tab.

## B. Value widget (`FilterSettingsPanel` + `FilterPill`)

- `FilterSettingsPanel` gains a "How should people filter on this?" control (radio: **Dropdown** /
  **Search** / **Input**) writing `param.widget`. Shown for category / id / location / text.
- `FilterPill`'s `FilterControl` honours `widget` (falling back to the type default):
  - **dropdown** → the current searchable checkbox list (category/location multi; id single).
  - **search** → the same list, search-first (the search box is the primary control).
  - **input** → a free-text input (single value; for text, or to skip the list).

## C. Default value (`FilterSettingsPanel`)

A "Default value" control whose editor matches the type (reuse `FilterPill`'s per-type control, or a
simple input/select), writing `param.default`. `useDashboardParams` already applies `default` on load
(no change there). Clearing it removes the default.

## D. Linked filters (chain-filter)

- `FilterSettingsPanel` gains a "Linked filters" section: a checkbox per OTHER parameter, writing
  `param.filteringParameters` (the ids whose value constrains this param's option list).
- **Option query** (`useColumnValues`) gains an optional `filters?: ChartFilter[]` arg, appended to
  the distinct-value query spec (`{ columns:[column], filters }`).
- **`ParamOptions`** (in `DashboardView`) computes those filters: it already finds the card that maps
  the param → `(datasetId, column)`. For each linked param `L` in `param.filteringParameters` that
  has a current value, read `colL = card.options.paramMappings[L]` (the same card's mapping on the
  same dataset); if present, add `paramToFilters(L, values[L.slug], colL)` to the option query.
  → the dropdown for the param only shows values consistent with the linked params' current values.
  (Linked param must be mapped on the same card/dataset for the chain to apply; otherwise it is
  ignored — no error.)

## E. New types / operators (`params.ts` `paramToFilters`)

- **boolean** (`is`): value is `"true"` / `"false"` (or bool); → `[{col, op:"eq", value: <bool>}]`;
  blank → `[]`. Widget: a 3-state select **Any / True / False** in `FilterPill`.
- **location** (`is` / `is-not`): behaves like a string category — multi-select →
  `in` / `not_in` array. Distinct values via `useColumnValues` (same as category).
- **date month-year** (value `"YYYY-MM"`): → `[ge "YYYY-MM-01", le <last day of month>]`. Widget:
  `<input type="month">`.
- **date quarter-year** (value `"YYYY-Q"`, Q in 1..4): → `[ge <quarter start>, le <quarter end>]`.
  Widget: a year input + a quarter select.
- `summarize` (the pill label helper) handles boolean ("True"/"False"), month-year (the value),
  quarter-year ("Q2 2024"), location (count like category).

## Data flow

```
edit a filter → FilterSettingsPanel: type/operator/widget/default/linked + (Delete)
  while filter selected → each chart card shows "Column to filter on" → writes paramMappings
view mode → FilterPill renders the value control per widget/type → setValue → paramToFilters → card refetch
linked: ParamOptions(param) builds option-query filters from linked params' values (same card mappings)
         → useColumnValues(…, filters) → constrained dropdown
```

## Error handling

- A linked param not mapped on the same card/dataset → ignored (no crash, options unconstrained).
- Boolean value binding: `eq` with a JS boolean is sent as JSON `true/false`; if the engine rejects
  a bool bind, fall back to the string `"true"/"false"` (verify live; note in the task).
- month-year/quarter-year with a partial value → no filter (blank).
- `widget:"input"` on a category clears the multi-select semantics (single string value) — acceptable.
- View mode and all prior behaviour (drill, cross-filter, required-gating, tabs, edit sidebar) unchanged.

## Testing

- **Vitest:**
  - `paramToFilters`: boolean → `eq` bool; location is/is-not → `in`/`not_in`; month-year →
    `ge/le` month range; quarter-year → `ge/le` quarter range.
  - `summarize`: boolean / month-year / quarter-year / location labels.
  - `useColumnValues`: passes `filters` into the option-query spec.
  - `FilterSettingsPanel`: widget radio writes `widget`; default control writes `default`; linked
    checkboxes write `filteringParameters`; new types appear in the type select.
  - `FilterPill`: widget `input` renders a text input; boolean renders the 3-state select;
    month-year/quarter-year render their pickers; `onChange` encodings correct.
  - `DashboardView`: selecting a filter shows the in-card "Column to filter on" select on chart
    cards and writes `paramMappings`; `CardSettingsPanel` no longer shows the per-param mapping.
    `ParamOptions` builds linked-filter constraints (unit-test the helper).
- **Live (Playwright MCP, side-by-side with Metabase):** add a filter → map it by picking columns on
  cards → set widget=Dropdown → values appear; link two filters → the dependent dropdown narrows;
  add a boolean/location/month-year filter and filter a card; set a default → it applies on reload.
  **0 console errors.**
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint &&
  npm run build && npx vitest run`.

## Conventions

Metabase as the standard · clean-room · TDD · full sweep before each commit · configured git identity
+ `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths ·
squash-merge · branch `feat/dashboards-tier2d-filter-ux`.
