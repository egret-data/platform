# Date Grouping — Sub-project B: Builder Temporal-Unit Breakout (Design)

**Status:** Approved approach, pending spec review
**Part of:** Date Grouping program (A → B → C). This is **B**, the no-code builder UI. Depends on **A**
(merged, PR #47), which added `QuerySpec.temporal_units` + the SQL.
**Canonical:** this file. Vietnamese reference: `docs/vi/specs/2026-06-24-date-grouping-builder-ui-design.md`.

## Goal

In the Explore chart builder, let the user pick a **temporal unit** for any temporal dimension (a
small dropdown next to the field), so a date/timestamp breakout groups by day/week/month/quarter/year
(truncation) or by hour-of-day/day-of-week/month-of-year/… (extraction). The chosen units flow into
`spec.temporal_units` (sub-project A consumes them). Charts grouped by extraction units render with
**friendly labels** (Mon…Sun, Jan…Dec, Q1…Q4) instead of raw integers.

This sub-project is **frontend only** — no backend change. A already accepts and applies the units.

## Decisions (from brainstorming)

- **Scope of the unit selector:** *all* temporal dimensions — wherever a chosen dimension column is
  `is_temporal`: cartesian x-axis, pie / funnel / waterfall / smart-scalar dimension, and each temporal
  column in the table/pivot dimension lists.
- **Auto-default:** selecting a temporal column into a dimension slot defaults its unit to **`month`**
  (Metabase-style; a date breakout immediately groups sensibly instead of per-second).
- **Friendly extraction labels:** mapped in B.

## Non-goals (explicit scope boundary)

- **Friendly labels render in cartesian + pie only** (the charts where extraction grouping is visually
  meaningful — axis categories and slice/legend labels). Table cells, funnel, and waterfall still show
  the raw extraction integer for now; the unit *selector* appears for those dimensions, but rendering a
  friendly label there is a later polish. Truncation units render as dates via existing date formatting.
- No dashboard-level Date-Grouping parameter (sub-project C).
- No backend change.

## Unit tokens (must match sub-project A exactly)

A's `TemporalUnit::parse` accepts these snake_case tokens; B emits the same strings:

- **Truncation:** `minute`, `hour`, `day`, `week`, `month`, `quarter`, `year`
- **Extraction:** `minute_of_hour`, `hour_of_day`, `day_of_week`, `day_of_month`, `day_of_year`,
  `week_of_year`, `month_of_year`, `quarter_of_year`

A unit absent from `temporal_units` means "raw / no grouping" (the UI's "None" choice).

## Components

### 1. `frontend/src/components/chart/temporal-units.ts` (shared module)

The single source of truth for unit metadata, used by both the selector and the renderer:

- `TEMPORAL_UNITS`: ordered list of `{ token, label, group }` where `group` is `"trunc"` | `"extract"`
  and `label` is the display string (`month` → "Month", `day_of_week` → "Day of week", etc.).
- `isExtractUnit(token: string): boolean`.
- `temporalLabel(token: string | undefined, value: unknown): string` — for extraction units with a
  natural name, map the integer to a label:
  - `day_of_week`: 1–7 → Mon, Tue, Wed, Thu, Fri, Sat, Sun (ISO, Monday=1 — matches A's normalization).
  - `month_of_year`: 1–12 → Jan … Dec.
  - `quarter_of_year`: 1–4 → Q1 … Q4.
  - all other tokens (and truncation, and `undefined`): return `String(value)`.

  Out-of-range or non-numeric values fall back to `String(value)` (defensive — never throw).

### 2. `frontend/src/components/chart/TemporalUnitSelect.tsx`

A small shadcn `<Select>` (`h-8 text-sm`, matching the existing sidebar controls):
- Props: `value: string | undefined` (the unit token, or undefined = None), `onChange: (token: string | undefined) => void`.
- Options: a "None" item (clears) plus two visually grouped sections ("Group by" / "Extract") built from
  `TEMPORAL_UNITS`, showing each `label`.
- Selecting "None" calls `onChange(undefined)`.

## ChartExplore wiring

`frontend/src/routes/explore/ChartExplore.tsx`:

1. **State:** `const [temporalUnits, setTemporalUnits] = useState<Record<string, string>>({})`.
   Hydrate on load from `p.spec?.temporal_units ?? {}` (alongside the other `setX(p…)` hydration).

2. **Selection helper + auto-default.** A helper that, when a dimension selector's value changes to a
   column, applies the default: if the column is `is_temporal` and not already keyed in `temporalUnits`,
   set it to `"month"`. Used by every temporal-capable dimension selector's `onValueChange`. (For the
   single-dimension selectors this fires on change; for the table/pivot multi-add lists it fires when a
   temporal column is added.)

3. **Render the selector.** After each dimension control, render
   `<TemporalUnitSelect value={temporalUnits[col]} onChange={(u) => setUnit(col, u)} />` **only when**
   `columns.find(c => c.column_name === col)?.is_temporal`. `setUnit(col, u)` sets or (when `u` is
   undefined) deletes the key. Applies to: `cartesian.xAxis`, `pie.dimension`, `funnel.dimension`,
   `waterfall.xAxis`, `smart.dimension`, and each temporal column in `tableCols` / pivot `pRows`+`pCols`.

4. **Prune into the spec (REQUIRED — A rejects orphan keys).** A returns `BadRequest` for a
   `temporal_units` key not present in the query's `dimensions`/`columns`. So `specForRun()` and
   `paramsForSave()` must include only the units whose column is in the current dimension set. Add a
   helper:

   ```ts
   const unitsFor = (dims: string[]) =>
     Object.fromEntries(Object.entries(temporalUnits).filter(([k]) => dims.includes(k)));
   ```

   Each viz branch that returns `dimensions: dims` adds `temporal_units: unitsFor(dims)`. (Branches with
   no temporal-capable dimension — scalar/gauge/progress — send `{}`.) This guarantees switching the
   x-axis away from a temporal column drops its stale unit before the request.

5. **Spec type.** Add `temporal_units?: Record<string, string>` to the chart spec type in
   `frontend/src/api/charts.ts` so `specForRun()`/`paramsForSave()` and the renderer are typed.

## Renderer: friendly extraction labels

`frontend/src/components/chart/ChartRenderer.tsx` already receives `params`, so it can read
`params.spec.temporal_units`:

- **Cartesian** (`echarts/options/cartesian.ts`): `CartesianBuildConfig` gains `xAxisUnit?: string`.
  When building x-axis categories and the x value in tooltips, if `isExtractUnit(xAxisUnit)`, map each
  category value through `temporalLabel(xAxisUnit, value)`. ChartRenderer passes
  `xAxisUnit: params.spec.temporal_units?.[cartesian.xAxis ?? ""]`.
- **Pie** (`echarts/options/pie.ts`): config gains `dimensionUnit?: string`; map each slice `name`
  through `temporalLabel` when it is an extraction unit. ChartRenderer passes
  `dimensionUnit: params.spec.temporal_units?.[pie.dimension ?? ""]`.

Truncation units need no mapping (the engine returns a date/timestamp the existing axis formatting
renders). The mapping only rewrites the *displayed* label; drill-through still carries the raw value.

## Data flow (end to end)

1. User picks a temporal column as x-axis → `temporalUnits[col]` auto-set to `"month"`; the
   `TemporalUnitSelect` appears showing "Month".
2. `specForRun()` sends `query_spec.temporal_units = { col: "month" }` (pruned to active dims).
3. A builds `date_trunc('month', col)` and groups by it; rows come back bucketed by month.
4. Renderer shows the date buckets (truncation → no label mapping). If the user instead picks
   "Day of week", A returns integers 1–7 and the cartesian axis maps them to Mon…Sun.

## Error handling

- Switching a dimension away from a temporal column prunes its unit (step 4) → no orphan-key error.
- `temporalLabel` is total (never throws); unknown/out-of-range values render as `String(value)`.
- "None" removes the key → A treats the dimension as raw.

## Testing (Vitest)

- `temporal-units.test.ts`: `temporalLabel` maps day_of_week 1→Mon / 7→Sun, month_of_year 1→Jan /
  12→Dec, quarter_of_year 2→Q2; truncation tokens and out-of-range values return `String(value)`;
  `isExtractUnit` classifies correctly.
- `TemporalUnitSelect.test.tsx`: renders grouped options + "None"; selecting an option calls `onChange`
  with the token; selecting "None" calls `onChange(undefined)`.
- `ChartExplore` unit behavior: a focused test (or extracted pure helper) that `unitsFor(dims)` prunes
  keys not in `dims`; auto-default sets `"month"` for a temporal column and leaves a non-temporal column
  untouched.
- Live verify (Playwright, 0 console errors, throwaway chart): pick a date x-axis → defaults to Month,
  chart groups monthly; switch to "Day of week" → axis shows Mon…Sun; switch x-axis to a non-temporal
  column → request succeeds (no orphan-key 400). Build + lint + typecheck + vitest all green.

## Files

- Create: `frontend/src/components/chart/temporal-units.ts`, `…/TemporalUnitSelect.tsx`,
  plus `__tests__/temporal-units.test.tsx`, `__tests__/TemporalUnitSelect.test.tsx`.
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (state, auto-default, selectors, pruning),
  `frontend/src/api/charts.ts` (spec type), `frontend/src/components/chart/ChartRenderer.tsx`,
  `…/echarts/options/cartesian.ts`, `…/echarts/options/pie.ts`.

## Conventions

Frontend sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`
(from `frontend/`). Configured git identity; `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
footer; stage explicit paths.
