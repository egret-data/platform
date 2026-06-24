# Date Grouping — Sub-project C: Dashboard Date-Grouping Parameter (Design)

**Status:** Approved approach, pending spec review
**Part of:** Date Grouping program (A → B → C). This is **C**, the dashboard-level parameter. Depends on
**A** (PR #47, backend `temporal_units`) and **B** (PR #48, `temporal-units.ts` + per-card grouping).
**Canonical:** this file. Vietnamese reference: `docs/vi/specs/2026-06-24-date-grouping-dashboard-param-design.md`.

## Goal

A dashboard parameter — a pill in the filter bar — that overrides the temporal grouping **unit** on the
cards it is connected to, re-gridding all of them together (e.g. flip every time-series card from monthly
to quarterly at once). Reuses the existing per-card parameter-mapping infrastructure, Metabase-style.

This sub-project is **frontend only**. The backend (A) already applies `spec.temporal_units`.

## Decisions (from brainstorming)

- **Per-card mapping** (Metabase-faithful): reuse `card.options.paramMappings` (param id → column). The
  parameter affects only the cards/columns the user connects it to.
- **Full unit list** (truncation + extraction), reusing B's `TEMPORAL_UNITS`.

## Key design rule — override only an already-grouped column

`applyParamsToChart` overrides `spec.temporal_units[mappedColumn]` **only when `mappedColumn` is already a
key in that card's `spec.temporal_units`** (i.e. already grouped by time, set in the builder by B — which
auto-defaults any temporal dimension to `month`). This rule:

- is **safe**: the column is already a validated temporal dimension, so the backend (A) never rejects it
  (no orphan-key `400`, no "temporal unit on non-temporal column"); and
- needs **no `is_temporal` metadata** in the parameter layer — it reads only the chart's own
  `spec.temporal_units`; and
- matches the mental model: the parameter "re-grids the time cards", it does not turn a non-grouped column
  into a grouped one.

A mapping to a column not currently time-grouped is a silent no-op (no override, no error).

## Components (all reuse existing infrastructure)

### 1. New parameter type — `frontend/src/api/dashboards.ts`

Add `"temporal_unit"` to the `ParamType` union. Its *value* is a unit token (`"month"`,
`"day_of_week"`, …, matching A/B). It carries no filter operator.

### 2. Parameter creation — `frontend/src/components/chart/dashboard/ParameterEditor.tsx`

- Add a `temporal_unit` entry to `OPERATORS` so it appears in the type `<select>`. Its operator is
  unused; give it a single benign placeholder (`OPERATORS.temporal_unit = ["is"]`) and keep `showOp`
  false for it (it already excludes everything except text/number/date).
- When the type becomes `temporal_unit`, set `param.default = "month"` (a sensible default unit) so the
  pill always has a value.
- Add a small default-unit dropdown (reusing the unit list) bound to `param.default`, shown only for
  `temporal_unit`, so the author can choose the default granularity.

### 3. View-mode widget — `frontend/src/components/chart/dashboard/FilterPill.tsx`

- Icon: a `Clock` for `param.type === "temporal_unit"` (in the icon switch at the top).
- Value widget: when `param.type === "temporal_unit"`, render a unit dropdown built from
  `TEMPORAL_UNITS` (grouped "Group by" / "Extract", same content as B's `TemporalUnitSelect`), whose
  selection sets the param value to the unit token. No "None" (a date-grouping param always has a unit;
  clearing the pill falls back to the param `default`).

### 4. Override application — `frontend/src/components/chart/dashboard/params.ts`

`applyParamsToChart(chart, parameters, values, mappings, now)` currently collects filter predicates into
`extra` and appends them to `spec.filters`. Extend it to also collect temporal-unit overrides:

```ts
const temporalOverrides: Record<string, string> = {};
for (const param of parameters) {
  const column = mappings[param.id];
  if (!column) continue;
  if (param.type === "temporal_unit") {
    const unit = (values[param.slug] ?? param.default);
    const current = chart.params.spec.temporal_units ?? {};
    if (typeof unit === "string" && unit && column in current) {
      temporalOverrides[column] = unit;   // re-grid an already-grouped column
    }
    continue;                              // not a filter
  }
  extra.push(...paramToFilters(param, values[param.slug], column, now));
}
```

Return early only when BOTH `extra` and `temporalOverrides` are empty. Otherwise merge:

```ts
spec: {
  ...chart.params.spec,
  filters: [...chart.params.spec.filters, ...extra],
  temporal_units: { ...(chart.params.spec.temporal_units ?? {}), ...temporalOverrides },
},
```

### 5. Pill summary — `frontend/src/components/chart/dashboard/params.ts` (`summarize`)

Add a branch: for `param.type === "temporal_unit"`, return the unit's display label
(`TEMPORAL_UNITS.find((u) => u.token === value)?.label ?? String(value)`), e.g. value `"month"` → `"Month"`.

### 6. Mapping picker — `frontend/src/routes/explore/DashboardView.tsx`

The per-card mapping UI (the column `<select>` near line 437 that writes `paramMappings`) already maps any
parameter to a card column. For a `temporal_unit` active parameter, **filter its column options to the
card's currently time-grouped columns** — the keys of that card's `chart.params.spec.temporal_units`. This
guides the author to valid targets (and the override rule no-ops anything else). The data is already on the
chart; no new fetch.

## Data flow (end to end)

1. **Edit:** author adds a parameter, picks type **Date grouping** (default `month`), and connects it to
   each relevant card's time-grouped date column via the existing mapping picker.
2. **View:** the filter bar shows a unit dropdown pill. Changing it sets `values[slug]`.
3. `applyParamsToChart` overrides `temporal_units[mappedColumn] = unit` for every mapped card whose mapped
   column is already grouped → those cards re-query at the new unit and re-render together.
4. A card with a filter parameter AND the date-grouping parameter gets both (filters appended, unit
   overridden) in the same pass.

## Error handling

- Mapped column not currently in the card's `temporal_units` → no override (silent no-op, no `400`).
- Blank param value → falls back to `param.default`; if that is also blank, no override.
- The override only changes the unit value of an existing key, so the backend's orphan-key and
  non-temporal guards are never triggered.

## Testing (Vitest)

- `params.ts` (`applyParamsToChart`): a `temporal_unit` param mapped to a column that IS in the chart's
  `temporal_units` overrides its unit; a column NOT in `temporal_units` is a no-op; a filter param and a
  temporal_unit param on the same card both apply; unchanged chart returned when nothing maps.
- `summarize`: `temporal_unit` value `"month"` → `"Month"`, `"day_of_week"` → `"Day of week"`.
- `ParameterEditor`: selecting type `temporal_unit` sets `default = "month"` and hides the operator row.
- `FilterPill`: a `temporal_unit` param renders the unit dropdown and the Clock icon.
- Live verify (Playwright, throwaway dashboard NOT `a1`, 0 console errors): add a Date-grouping param to a
  dashboard with two time-series cards, map it to both date columns, then switch the pill Month → Quarter
  and confirm both cards re-grid to quarterly together; a card not mapped is unaffected; reload preserves
  the param + mapping. Full frontend sweep green.

## Files

- Modify: `frontend/src/api/dashboards.ts` (ParamType), `…/dashboard/params.ts` (applyParamsToChart +
  summarize), `…/dashboard/FilterPill.tsx` (widget + icon), `…/dashboard/ParameterEditor.tsx` (type +
  default), `frontend/src/routes/explore/DashboardView.tsx` (mapping-picker filter).
- Reuse: `frontend/src/components/chart/temporal-units.ts` (`TEMPORAL_UNITS`).

## Non-goals

- No backend change. No Object/Map viz. No new mapping infrastructure (reuse `paramMappings`). The param
  re-grids already-grouped columns only (it does not group a previously-raw column).

## Conventions

Frontend sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`
(from `frontend/`). Configured git identity; `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
footer; stage explicit paths.
