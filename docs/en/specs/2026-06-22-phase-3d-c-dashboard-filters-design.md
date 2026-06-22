# Phase 3d-C — Dashboards: filters / parameters (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Builds on:** Phase A (grid, PR #25) + Phase B (card variety + view controls, PR #26, `b66cde3`)

## Goal

Add Metabase-style **dashboard filters** (parameters): a bar of filter widgets above
the grid whose values are mapped to each card's columns and injected into every
card's query, plus `{{slug}}` interpolation in text/heading cards. The signature
Metabase dashboard feature — built to the same fidelity bar as Phases A/B.

## Metabase grounding (clean-room)

Read the Metabase source (`data-portal/metabase`, `metabase-types/api/parameters.ts`,
`metabase-lib/v1/parameters/constants.ts`, `visualizations/shared/utils/parameter-substitution.ts`):

- A dashboard owns `parameters: Parameter[]`. A `Parameter` has `id, name, slug,
  type, default?, required?, sectionId?, filteringParameters?`. Type encodes the
  **operator** (`string/=`, `string/contains`, `string/starts-with`, `number/=`,
  `number/between`, `number/>=`, `date/range`, `date/relative`, `id`, `category`…).
- Per **dashcard**: `parameter_mappings: [{ parameter_id, target }]`, `target =
  ["dimension", ["field", id, opts]]` — maps a parameter to one of the card's fields.
- A parameter **value** reaches a card's query as a filter on the mapped field (the
  dashcard query carries `parameters: [{ type, target, value }]`). Filters are
  **live** (no Apply); `required` params use `default`. Category values default to
  **multi-select** (`in`); the value source is the connected field's distinct values.
- Values sync to the **URL** by slug. Text/heading cards substitute `{{slug}}` via
  `fillParametersInText`.
- **Linked filters** (`filteringParameters`) constrain one filter's options by
  another — **deferred** for egret.

### How this maps onto egret (no engine change)

egret builds SQL from a `QuerySpec { metrics, dimensions, columns, filters:
ChartFilter[], ... }`. A `ChartFilter` is `{ col, op, value }` with ops
`eq|ne|gt|ge|lt|le|like|in|not_in|is_null|is_not_null` (verified: `like` renders
`col LIKE <value>` verbatim; `in` renders `col IN (...)`). So a parameter value
mapped to a card's column becomes one (or two) extra `ChartFilter`s **injected
client-side** into the card's spec before the existing `build-chart-data` →
submit → poll pipeline. Mapping is by **column name** (egret has no field ids).

## Scope (this PR)

**In:** text / number / date / id / category parameters with a chosen operator;
explicit per-card column mapping; a filter widget bar; client-side injection;
URL-synced values by slug; `{{slug}}` interpolation; category multi-select with a
distinct-values dropdown sourced from the connected field.

**Deferred (named):** linked filters (`filteringParameters`), location parameter
types, static/"from another card" value sources, the full Metabase date picker
(we do range + relative presets + single), and "does not contain" (egret has no
`NOT LIKE`).

## Data model

### Backend — migration `0016_dashboard_parameters.sql`

```sql
ALTER TABLE dashboards ADD COLUMN parameters jsonb NOT NULL DEFAULT '[]';
```

Thread `parameters` through: `Dashboard` struct + `COLS`; `DashboardDetailDto`;
`UpdateDashboard` (accepts `parameters`); `DashboardRepo::update_meta` →
**`update`** (now sets `name, description, parameters`). Create starts with `[]`.
`replace_cards` is unchanged — per-card mappings live in `dashboard_cards.options`
(already jsonb, no migration). `get` returns the stored `parameters`.

### Parameter (stored in `dashboards.parameters`)

```ts
type ParamType = "text" | "number" | "date" | "id" | "category";
type ParamOperator =
  | "is" | "is-not" | "contains" | "starts-with" | "ends-with"   // text
  | "=" | "!=" | "between" | ">=" | "<="                         // number
  | "range" | "relative" | "single";                            // date
interface DashboardParameter {
  id: string;          // stable client-generated id
  name: string;        // display label
  slug: string;        // url key + {{slug}} token (derived from name, unique)
  type: ParamType;
  operator: ParamOperator;   // text/number/date pick an operator; id/category fixed
  default?: unknown;
  required?: boolean;
}
```

`id` (fixed `is`/eq) and `category` (fixed multi-select → `in`) don't expose an
operator picker; `text`/`number` do; `date` exposes a mode (range / relative /
single).

### Per-card mapping (in `dashboard_cards.options`)

```ts
options.paramMappings: { [parameterId: string]: string /* column name */ }
```
A card is affected by a parameter only if it has a mapping for it. Mappable columns
= the card chart's `params.spec.dimensions ∪ columns` (the non-metric fields).

## Parameter → ChartFilter mapping (`paramToFilters`)

Pure function `paramToFilters(param, value, column): ChartFilter[]` (empty when the
value is blank/undefined):

- **text** `is`→`{op:"eq"}` · `is-not`→`{op:"ne"}` · `contains`→`{op:"like", value:"%"+v+"%"}` · `starts-with`→`{op:"like", value:v+"%"}` · `ends-with`→`{op:"like", value:"%"+v}`
- **number** `=`→eq · `!=`→ne · `>=`→ge · `<=`→le · `between`→ **two** filters `{op:"ge",value:min}` + `{op:"le",value:max}`
- **date** `range`→ `{op:"ge",value:start}`+`{op:"le",value:end}` · `relative`→ compute `[start,end]` from the preset (last 7/30/90 days, etc.) then ge+le · `single`→ `{op:"eq",value:day}`
- **id** → `{op:"eq", value}`
- **category** → `{op:"in", value: string[]}` (multi-select)

## Frontend architecture

1. **`api/dashboards.ts`** — add `parameters: DashboardParameter[]` to
   `DashboardDetail`; the update mutation payload gains `parameters`. New
   `paramToFilters` + `applyParamsToChart(chart, values, mappings)` helpers
   (pure, unit-tested) — the latter returns a chart whose `params.spec.filters`
   includes the injected filters for the card's mapped params.

2. **`useDashboardParams(parameters)`** hook — reads/writes each param's value to
   the URL by slug (`?<slug>=<value>`, JSON-encoded for arrays/ranges), seeded from
   `default`; returns `{ values, setValue }`. Mirrors Phase B's
   `useDashboardViewControls` URL-sync pattern.

3. **`useColumnValues(ws, chart, column)`** — runs a dimension-only QuerySpec
   (`{ dimensions:[column], metrics:[], filters:[], orderby:[], row_limit:1000,
   rollup_groups:[] }`) through build→submit→poll and returns distinct values, for
   category/id dropdowns. Source = the first card+column the parameter is mapped to.

4. **Filter widget bar** (`<DashboardFilterBar>`, shown in view AND edit) — one
   widget per parameter by type: text input; number input(s) (two for `between`);
   date range picker / relative preset dropdown / single date; id input or dropdown;
   category multi-select dropdown (values from `useColumnValues`). Editing a value
   calls `setValue`.

5. **Parameter manager** (edit mode) — a "+ Filter" control adds a parameter
   (name → slug, type, operator, default, required); an editor row per parameter to
   rename / change type+operator / set default / mark required / delete. Mutates the
   draft `parameters` (kept in DashboardView state alongside `draftCards`), saved by
   the existing whole-layout PUT (now also sending `parameters`).

6. **Per-card mapping UI** (edit mode) — in each card, for every dashboard
   parameter, a dropdown to pick which of the card's columns it maps to (or "none"),
   writing `options.paramMappings[paramId]`.

7. **Injection** — `DashboardView` computes, per card, `applyParamsToChart(chart,
   values, card.options.paramMappings)` and passes the augmented chart to the card.
   Because `useChartData` keys on `JSON.stringify(spec)`, changing a parameter value
   changes the injected filters → the spec key changes → cards **auto-refetch**. No
   new nonce needed.

8. **`{{slug}}` interpolation** — `interpolateParams(text, parameters, values)`
   replaces `{{slug}}` tokens with the current value (blank when unset); TextCard /
   HeadingCard receive interpolated text (DashboardCardView threads param values in).

## Data flow

```
DashboardView (view)
  ├─ useDashboard → dashboard.parameters
  ├─ useDashboardParams(parameters) ⇄ URL (?slug=value) → values
  ├─ <DashboardFilterBar parameters values onChange=setValue />
  └─ per card:
       applyParamsToChart(chart, values, card.options.paramMappings)
         → useChartData(ws, augmentedChart)   // spec.filters changed → refetch
       text/heading: interpolateParams(options.text, parameters, values)

DashboardView (edit)  — also: parameter manager (draft parameters),
  per-card mapping dropdowns; Save = whole-layout PUT { name, description,
  parameters, cards (options carry paramMappings) }.
```

## Error handling

- A parameter mapped to a column a card no longer has → that card simply isn't
  filtered (mapping ignored), no crash.
- Required parameter with no value and no default → the widget is flagged; cards
  still render unfiltered (egret has no hard-block; we surface a hint).
- Blank parameter value → `paramToFilters` returns `[]` (no filter injected).
- `between`/`range` with only one bound → inject only the provided side.
- Distinct-values query failure → the dropdown shows a free-text fallback, no crash.
- Duplicate slugs → slugs are uniquified on creation.

## Testing

- **Backend:** extend `dashboard_repo.rs` (parameters round-trip on create/get/update)
  and the http auth smoke test. (`0016` applies; remember the sqlx-migration-staleness
  gotcha when verifying locally — rebuild `egret-metastore`.)
- **Vitest:** `paramToFilters` (every type+operator → correct ChartFilter[], incl.
  `%` wildcards, between→2 filters, category→in, relative→range); `applyParamsToChart`
  (injects only mapped params, leaves unmapped cards untouched); `interpolateParams`
  (`{{slug}}` substitution, unset→blank); `useDashboardParams` (URL round-trip,
  default seeding); the filter widgets render per type; the parameter manager
  add/edit/delete; per-card mapping dropdown writes `options.paramMappings`.
- **Live (manual, via Playwright MCP browser):** add a text + a category + a date
  parameter, map them to cards, set values → affected cards refetch filtered;
  `{{slug}}` shows in a text card; values persist in the URL and across reload;
  **0 console errors**. (This is manual MCP browser driving, not an automated
  Playwright test — egret has no Playwright suite; automated tests are Vitest.)
- **Full sweep** before each commit: from `frontend/`,
  `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Out of scope (later)

Linked filters, location types, static/card value sources, full date picker,
"does not contain"; click-behaviour, tabs, public sharing (Phase D).

## Conventions

Metabase as the standard · clean-room (no AGPL) · TDD · full sweep before each
commit · configured git identity (never `--author`), footer
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage
explicit paths · squash-merge.
