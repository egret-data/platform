# Date Grouping — Sub-project B (Builder Temporal-Unit Breakout) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the Explore builder, let users pick a temporal unit for any temporal dimension; group via `spec.temporal_units` (sub-project A), with friendly labels (Mon…Sun, Jan…Dec, Q1…Q4) for extraction units in cartesian + pie.

**Architecture:** A shared `temporal-units.ts` module is the single source of unit tokens, label maps, and helpers. A `TemporalUnitSelect` dropdown writes into a `temporalUnits` map in `ChartExplore`; a `useEffect` auto-defaults a newly-selected temporal dimension to `month`; `specForRun()` prunes the map to the active dimensions (A rejects orphan keys). The cartesian/pie ECharts option builders map extraction integers to friendly labels.

**Tech Stack:** React 18 + TS + Vite + Vitest + Tailwind v4 + shadcn `Select`, ECharts.

**Spec:** `docs/en/specs/2026-06-24-date-grouping-builder-ui-design.md`
**Branch:** `feat/date-grouping-builder-ui` (already checked out; spec committed).
**Depends on:** sub-project A (merged, PR #47) — backend already accepts `temporal_units` and rejects orphan keys.

**Conventions (every task):** all commands from `frontend/`. Sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Commit footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Do NOT push until Task 7.

**Unit tokens (must match A's `TemporalUnit::parse`):** truncation `minute, hour, day, week, month, quarter, year`; extraction `minute_of_hour, hour_of_day, day_of_week, day_of_month, day_of_year, week_of_year, month_of_year, quarter_of_year`.

---

### Task 1: `temporal-units.ts` shared module

**Files:**
- Create: `frontend/src/components/chart/temporal-units.ts`
- Test: `frontend/src/components/chart/__tests__/temporal-units.test.ts`

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/__tests__/temporal-units.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { TEMPORAL_UNITS, isExtractUnit, temporalLabel, pruneUnits } from "../temporal-units";

describe("temporal-units", () => {
  it("lists all 15 Metabase tokens grouped trunc/extract", () => {
    expect(TEMPORAL_UNITS).toHaveLength(15);
    expect(TEMPORAL_UNITS.filter((u) => u.group === "trunc").map((u) => u.token)).toEqual([
      "minute", "hour", "day", "week", "month", "quarter", "year",
    ]);
    expect(TEMPORAL_UNITS.filter((u) => u.group === "extract")).toHaveLength(8);
  });

  it("classifies extraction units", () => {
    expect(isExtractUnit("month")).toBe(false);
    expect(isExtractUnit("day_of_week")).toBe(true);
    expect(isExtractUnit(undefined)).toBe(false);
  });

  it("maps extraction integers to friendly labels", () => {
    expect(temporalLabel("day_of_week", 1)).toBe("Mon");
    expect(temporalLabel("day_of_week", 7)).toBe("Sun");
    expect(temporalLabel("month_of_year", 1)).toBe("Jan");
    expect(temporalLabel("month_of_year", 12)).toBe("Dec");
    expect(temporalLabel("quarter_of_year", 2)).toBe("Q2");
    // truncation / out-of-range / unknown → String(value)
    expect(temporalLabel("month", "2024-01-01")).toBe("2024-01-01");
    expect(temporalLabel("day_of_week", 9)).toBe("9");
    expect(temporalLabel(undefined, 5)).toBe("5");
  });

  it("prunes units to the selected dimension set", () => {
    expect(pruneUnits({ created_at: "month", region: "day" }, ["created_at"])).toEqual({
      created_at: "month",
    });
    expect(pruneUnits({ created_at: "month" }, [])).toEqual({});
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/__tests__/temporal-units.test.ts`
Expected: FAIL — cannot resolve `../temporal-units`.

- [ ] **Step 3: Implement the module**

`frontend/src/components/chart/temporal-units.ts`:

```ts
export type TemporalGroup = "trunc" | "extract";
export interface TemporalUnitDef {
  token: string;
  label: string;
  group: TemporalGroup;
}

/** The Metabase temporal-unit set. Tokens MUST match the backend TemporalUnit::parse (sub-project A). */
export const TEMPORAL_UNITS: TemporalUnitDef[] = [
  { token: "minute", label: "Minute", group: "trunc" },
  { token: "hour", label: "Hour", group: "trunc" },
  { token: "day", label: "Day", group: "trunc" },
  { token: "week", label: "Week", group: "trunc" },
  { token: "month", label: "Month", group: "trunc" },
  { token: "quarter", label: "Quarter", group: "trunc" },
  { token: "year", label: "Year", group: "trunc" },
  { token: "minute_of_hour", label: "Minute of hour", group: "extract" },
  { token: "hour_of_day", label: "Hour of day", group: "extract" },
  { token: "day_of_week", label: "Day of week", group: "extract" },
  { token: "day_of_month", label: "Day of month", group: "extract" },
  { token: "day_of_year", label: "Day of year", group: "extract" },
  { token: "week_of_year", label: "Week of year", group: "extract" },
  { token: "month_of_year", label: "Month of year", group: "extract" },
  { token: "quarter_of_year", label: "Quarter of year", group: "extract" },
];

const EXTRACT_TOKENS = new Set(
  TEMPORAL_UNITS.filter((u) => u.group === "extract").map((u) => u.token),
);

export function isExtractUnit(token: string | undefined): boolean {
  return token != null && EXTRACT_TOKENS.has(token);
}

const DOW = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Display label for a (possibly extraction-grouped) dimension value. Total: never throws;
 *  unknown tokens, truncation, and out-of-range values fall back to String(value). */
export function temporalLabel(token: string | undefined, value: unknown): string {
  const n = typeof value === "number" ? value : Number(value);
  if (token === "day_of_week" && Number.isInteger(n) && n >= 1 && n <= 7) return DOW[n - 1];
  if (token === "month_of_year" && Number.isInteger(n) && n >= 1 && n <= 12) return MON[n - 1];
  if (token === "quarter_of_year" && Number.isInteger(n) && n >= 1 && n <= 4) return `Q${n}`;
  return String(value);
}

/** Keep only the units whose column is in `dims` — the backend rejects a temporal_unit keyed to a
 *  column the query does not select (orphan key → BadRequest). */
export function pruneUnits(
  units: Record<string, string>,
  dims: string[],
): Record<string, string> {
  return Object.fromEntries(Object.entries(units).filter(([k]) => dims.includes(k)));
}
```

- [ ] **Step 4: Run tests**

Run: `npx vitest run src/components/chart/__tests__/temporal-units.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/chart/temporal-units.ts src/components/chart/__tests__/temporal-units.test.ts
git commit -m "feat(chart): temporal-units module (tokens, labels, prune helper)"
```

---

### Task 2: `TemporalUnitSelect` component

**Files:**
- Create: `frontend/src/components/chart/TemporalUnitSelect.tsx`
- Test: `frontend/src/components/chart/__tests__/TemporalUnitSelect.test.tsx`

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/__tests__/TemporalUnitSelect.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { TemporalUnitSelect } from "../TemporalUnitSelect";

describe("TemporalUnitSelect", () => {
  it("renders the current unit label", () => {
    render(<TemporalUnitSelect value="month" onChange={() => {}} />);
    expect(screen.getByText("Month")).toBeTruthy();
  });

  it("shows 'None' as the placeholder when no unit is set", () => {
    render(<TemporalUnitSelect value={undefined} onChange={() => {}} />);
    expect(screen.getByText("None")).toBeTruthy();
  });
});
```

(Radix `Select` does not open its listbox in jsdom without pointer events, so the test asserts the
closed-state label only — the open/select interaction is covered by the live Playwright pass in Task 7.)

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/__tests__/TemporalUnitSelect.test.tsx`
Expected: FAIL — cannot resolve `../TemporalUnitSelect`.

- [ ] **Step 3: Implement the component**

`frontend/src/components/chart/TemporalUnitSelect.tsx`:

```tsx
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { TEMPORAL_UNITS } from "./temporal-units";

const NONE = "__none";

/** Picks a temporal grouping unit for one temporal dimension. `undefined` value = no grouping (raw). */
export function TemporalUnitSelect({
  value,
  onChange,
}: {
  value: string | undefined;
  onChange: (token: string | undefined) => void;
}) {
  const trunc = TEMPORAL_UNITS.filter((u) => u.group === "trunc");
  const extract = TEMPORAL_UNITS.filter((u) => u.group === "extract");
  return (
    <Select value={value ?? NONE} onValueChange={(v) => onChange(v === NONE ? undefined : v)}>
      <SelectTrigger className="h-7 text-xs">
        <SelectValue placeholder="None" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={NONE}>None</SelectItem>
        <SelectGroup>
          <SelectLabel>Group by</SelectLabel>
          {trunc.map((u) => (
            <SelectItem key={u.token} value={u.token}>{u.label}</SelectItem>
          ))}
        </SelectGroup>
        <SelectGroup>
          <SelectLabel>Extract</SelectLabel>
          {extract.map((u) => (
            <SelectItem key={u.token} value={u.token}>{u.label}</SelectItem>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
}
```

- [ ] **Step 4: Run tests**

Run: `npx vitest run src/components/chart/__tests__/TemporalUnitSelect.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/chart/TemporalUnitSelect.tsx src/components/chart/__tests__/TemporalUnitSelect.test.tsx
git commit -m "feat(chart): TemporalUnitSelect dropdown (grouped trunc/extract + None)"
```

---

### Task 3: Spec type + ChartExplore plumbing (state, hydrate, prune, auto-default)

**Files:**
- Modify: `frontend/src/api/charts.ts` (`QuerySpec` interface at lines 24–32)
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

No new UI yet — this task threads the data so units (currently always empty at runtime) flow into the
request and saved params, and auto-defaults newly-selected temporal dimensions. Typecheck/build prove it.

- [ ] **Step 1: Add the spec field**

In `frontend/src/api/charts.ts`, `QuerySpec` (after `rollup_groups: string[][];`):

```ts
  rollup_groups: string[][];
  /** dimension column name -> temporal unit token (e.g. "month", "day_of_week"). */
  temporal_units?: Record<string, string>;
}
```

- [ ] **Step 2: Add state + imports in ChartExplore**

At the top of `ChartExplore.tsx`, add to the existing import from `@/components/chart/...` (or a new import line):

```tsx
import { pruneUnits } from "@/components/chart/temporal-units";
```

Add `useEffect` to the React import if not already present.

Near the other `useState` hooks (e.g. after `const [tableCols, setTableCols] = useState<string[]>([]);` at line 60):

```tsx
  const [temporalUnits, setTemporalUnits] = useState<Record<string, string>>({});
```

- [ ] **Step 3: Hydrate from a saved chart**

In the hydration block, immediately after `setRowLimit(p.spec?.row_limit ?? 1000);` (line ~127):

```tsx
    setTemporalUnits(p.spec?.temporal_units ?? {});
```

- [ ] **Step 4: Add `setUnit` helper**

Near `specForRun` (before it), add:

```tsx
  const setUnit = (col: string, token: string | undefined) =>
    setTemporalUnits((prev) => {
      const next = { ...prev };
      if (token == null) delete next[col];
      else next[col] = token;
      return next;
    });
```

- [ ] **Step 5: Thread `temporal_units` into every aggregate branch of `specForRun()`**

In `specForRun()`, add `temporal_units: pruneUnits(temporalUnits, dims)` to each branch that returns a
`dimensions` array, using that branch's dimension list. Exact edits:

Cartesian branch:
```tsx
      return { metrics: mets, dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
```
Pie branch:
```tsx
      return { metrics: pie.metric ? [pie.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
```
Funnel branch (compute dims inline):
```tsx
    if (viz === "funnel") {
      const dims = funnel.dimension ? [funnel.dimension] : [];
      return { metrics: funnel.metric ? [funnel.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
```
Waterfall branch:
```tsx
    if (viz === "waterfall") {
      const dims = waterfall.xAxis ? [waterfall.xAxis] : [];
      return { metrics: waterfall.metric ? [waterfall.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
```
Smartscalar branch:
```tsx
      return { metrics: smart.metric ? [smart.metric] : [], dimensions: dims, columns: [], filters, orderby: smart.dimension ? [{ field: smart.dimension, desc: false }] : [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
```
Sankey branch:
```tsx
    if (viz === "sankey") {
      const dims = [sankey.source, sankey.target].filter((d): d is string => !!d);
      return { metrics: sankey.metric ? [sankey.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
```
Pivot branch (after `const dims = [...pRows, ...pCols];`):
```tsx
        metrics: pVals, dimensions: dims, columns: [], filters, orderby: [],
        row_limit: rowLimit, rollup_groups: groups, temporal_units: pruneUnits(temporalUnits, dims),
```
Table branch (final return):
```tsx
    const agg = tableMetrics.length > 0;
    const dims = agg ? tableCols : [];
    return {
      metrics: tableMetrics, dimensions: dims, columns: agg ? [] : tableCols,
      filters, orderby: [], row_limit: rowLimit, rollup_groups: [],
      temporal_units: pruneUnits(temporalUnits, dims),
    };
```
(scalar/progress/gauge/boxplot branches have no temporal dimension — leave them unchanged; they omit `temporal_units`, which serializes as absent = empty.)

- [ ] **Step 6: Auto-default newly-selected temporal dimensions to `month`**

After `specForRun` is defined, add:

```tsx
  // A temporal dimension defaults to monthly grouping (Metabase-style) until the user changes it.
  const activeDims = specForRun().dimensions;
  useEffect(() => {
    setTemporalUnits((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const name of activeDims) {
        const temporal = columns.find((c) => c.column_name === name)?.is_temporal;
        if (temporal && !(name in next)) {
          next[name] = "month";
          changed = true;
        }
      }
      return changed ? next : prev;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeDims.join("|"), columns]);
```

- [ ] **Step 7: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green (no behavior visible yet — units stay `{}` without the selector, so existing tests/SQL are unchanged).

- [ ] **Step 8: Commit**

```bash
git add src/api/charts.ts src/routes/explore/ChartExplore.tsx
git commit -m "feat(chart): thread temporal_units through the builder spec + auto-default month"
```

---

### Task 4: Inline unit selector for single-dimension viz

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

Add `<TemporalUnitSelect>` directly under each single-dimension `<Select>`, shown only when the chosen
column is temporal. Import it first.

- [ ] **Step 1: Import the component**

```tsx
import { TemporalUnitSelect } from "@/components/chart/TemporalUnitSelect";
```

- [ ] **Step 2: Add a small helper to render the unit row**

Before the `return (` of the component JSX, add:

```tsx
  const unitRow = (col: string | undefined) =>
    col && columns.find((c) => c.column_name === col)?.is_temporal ? (
      <div className="flex items-center gap-2 pl-2">
        <span className="text-[11px] uppercase tracking-wide text-muted-foreground">Unit</span>
        <TemporalUnitSelect value={temporalUnits[col]} onChange={(u) => setUnit(col, u)} />
      </div>
    ) : null;
```

- [ ] **Step 3: Render it after each single-dimension selector**

Insert `{unitRow(<field>)}` immediately after the closing `</Select>`'s wrapping `</div>` of each
control:
- After the cartesian X-axis control (`<Select value={cartesian.xAxis ?? ""} …>` ~line 427): `{unitRow(cartesian.xAxis)}`
- After the pie dimension control (`pie.dimension` ~line 468): `{unitRow(pie.dimension)}`
- After the funnel dimension control (`funnel.dimension` ~line 551): `{unitRow(funnel.dimension)}`
- After the waterfall X-axis control (`waterfall.xAxis` ~line 574): `{unitRow(waterfall.xAxis)}`
- After the smartscalar dimension control (`smart.dimension` ~line 601): `{unitRow(smart.dimension)}`

(Locate each by its `value={…}` binding; place `{unitRow(...)}` as a sibling right after the field's
own `<div className="space-y-1">…</div>` block.)

- [ ] **Step 4: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/routes/explore/ChartExplore.tsx
git commit -m "feat(chart): inline temporal-unit selector for cartesian/pie/funnel/waterfall/smartscalar"
```

---

### Task 5: Unit controls for table + pivot (multi-dimension)

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

Table and pivot have multi-column dimension lists, so render a compact "Date grouping" sub-block that
lists each selected temporal dimension with a unit selector.

- [ ] **Step 1: Add a reusable sub-block helper**

Near `unitRow` (from Task 4), add:

```tsx
  const unitList = (dims: string[]) => {
    const temporal = dims.filter((d) => columns.find((c) => c.column_name === d)?.is_temporal);
    if (temporal.length === 0) return null;
    return (
      <div className="space-y-1">
        <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Date grouping</Label>
        {temporal.map((col) => (
          <div key={col} className="flex items-center justify-between gap-2">
            <span className="truncate text-xs">{col}</span>
            <TemporalUnitSelect value={temporalUnits[col]} onChange={(u) => setUnit(col, u)} />
          </div>
        ))}
      </div>
    );
  };
```

- [ ] **Step 2: Render it in the pivot controls**

After the pivot `FieldZone` for "Columns" (the `<FieldZone label="Columns" …>` at ~line 416), add:

```tsx
                {unitList([...pRows, ...pCols])}
```

- [ ] **Step 3: Render it in the table controls**

In the table column section (the checkbox list using `tableCols` ~line 401–403), after that list's
container, add:

```tsx
                {unitList(tableMetrics.length > 0 ? tableCols : [])}
```

(Only aggregate-mode table columns are dimensions; raw-mode table sends no units.)

- [ ] **Step 4: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/routes/explore/ChartExplore.tsx
git commit -m "feat(chart): date-grouping unit controls for table + pivot dimensions"
```

---

### Task 6: Friendly extraction labels in the renderer (cartesian + pie)

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Modify: `frontend/src/components/chart/echarts/options/pie.ts`
- Modify: `frontend/src/components/chart/ChartRenderer.tsx`

The dimension value for an extraction unit comes back as an integer (1–7, 1–12, …). Map it to a label
via `temporalLabel` at the point each builder turns the dimension column into a category/slice name.

- [ ] **Step 1: cartesian.ts — accept and apply `xAxisUnit`**

Read `cartesian.ts`. In `CartesianBuildConfig`, add `xAxisUnit?: string;`. Import at the top:
```ts
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";
```
Where the x-axis category values are derived from the xAxis column (the array of category names built
from `rows` at the `cfg.xAxis` index), wrap each value: when `isExtractUnit(cfg.xAxisUnit)`, replace the
category value with `temporalLabel(cfg.xAxisUnit, value)`. Apply the same mapping anywhere the x value is
shown to the user (axis label formatter, tooltip x label). Do NOT change the underlying numeric value
used for sorting/`applyXScale` — map only the displayed string. The simplest correct placement is to map
the category array once, immediately after it is built, since categories are what ECharts renders on the
axis.

- [ ] **Step 2: pie.ts — accept and apply `dimensionUnit`**

Read `pie.ts`. In `PieBuildConfig`, add `dimensionUnit?: string;`. Import `isExtractUnit, temporalLabel`
from `@/components/chart/temporal-units`. Where each slice `name` is built (`String(r[di] ?? "")`), wrap:
`isExtractUnit(cfg.dimensionUnit) ? temporalLabel(cfg.dimensionUnit, r[di]) : String(r[di] ?? "")`.

- [ ] **Step 3: ChartRenderer.ts — pass the units in**

In `ChartRenderer.tsx`, the cartesian `buildCartesianOption(columns, rows, { … })` call: add
```tsx
              xAxisUnit: params.spec.temporal_units?.[cartesian.xAxis ?? ""],
```
The pie `buildPieOption(columns, rows, { … })` call: add
```tsx
              dimensionUnit: params.spec.temporal_units?.[pie.dimension ?? ""],
```

- [ ] **Step 4: Add a focused test for the mapping wiring**

Append to `frontend/src/components/chart/__tests__/temporal-units.test.ts` a test proving the builder uses
the label (import `buildPieOption`, assert a slice name is mapped):

```ts
import { buildPieOption } from "../echarts/options/pie";

it("pie slice names use the extraction label", () => {
  const opt = buildPieOption(
    [{ name: "dow", type: "int" }, { name: "n", type: "int" }],
    [[1, 10], [7, 3]],
    { dimension: "dow", metric: "n", dimensionUnit: "day_of_week" },
  ) as { series: { data: { name: string }[] }[] };
  const names = opt.series[0].data.map((d) => d.name);
  expect(names).toContain("Mon");
  expect(names).toContain("Sun");
});
```

- [ ] **Step 5: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green (the new pie test passes; existing chart tests unaffected since `dimensionUnit` is optional/undefined for non-temporal charts).

- [ ] **Step 6: Commit**

```bash
git add src/components/chart/echarts/options/cartesian.ts src/components/chart/echarts/options/pie.ts src/components/chart/ChartRenderer.tsx src/components/chart/__tests__/temporal-units.test.ts
git commit -m "feat(chart): friendly extraction labels (Mon/Jan/Q1) in cartesian + pie"
```

---

### Task 7: Live verify + PR

**Files:** none (verification + PR)

- [ ] **Step 1: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green.

- [ ] **Step 2: Live verify (Playwright, dev stack running, throwaway chart — do NOT touch the user's "a1" dashboard)**

Log in `admin@egret.local` / `Admin12345!`, open Explore on a dataset with a timestamp column (e.g. the
`eng` workspace `ecom_orders`-style data with `created_at`). Confirm, with 0 console errors:
1. Pick the timestamp column as the bar/line **X-axis** → the "Unit" selector appears showing **Month**, and the chart groups by month (not per-second).
2. Change the unit to **Day of week** → the x-axis shows **Mon…Sun** (not 1–7).
3. Change the X-axis to a **non-temporal** column → the request succeeds with **no 400** (orphan key pruned), and the Unit selector disappears.
4. Switch to a **pie** chart grouped by the timestamp with **Month of year** → slices labelled **Jan…Dec**.
5. Save the chart, reload → the unit selection is restored (hydration).

- [ ] **Step 3: Push + open PR**

```bash
git push -u origin feat/date-grouping-builder-ui
gh pr create --base main --title "feat(chart): builder temporal-unit breakout (Date Grouping B)" --body "<summary + screenshots/notes from the live verify>"
```

---

## Self-Review

**Spec coverage:** shared module + tokens/labels/prune (Task 1) ✓; TemporalUnitSelect (Task 2) ✓; spec type + state + hydrate + prune + auto-default-month (Task 3) ✓; selector for all single-dimension temporal viz (Task 4) ✓; table + pivot dimensions (Task 5) ✓; friendly extraction labels in cartesian + pie (Task 6) ✓; live verify incl. orphan-key pruning + hydration (Task 7) ✓. Non-goals (table/funnel/waterfall integer labels, dashboard param) respected — no task adds them.

**Placeholder scan:** every code step shows full code or, for the two complex ECharts builders (Task 6 steps 1–2), names the exact config field, import, and transformation expression with the precise placement rule (map the displayed category/slice string via `temporalLabel`, leave numeric sort values intact). No TBD/TODO.

**Type consistency:** `temporal_units?: Record<string,string>` (charts.ts) ↔ `pruneUnits(units, dims): Record<string,string>` ↔ `setUnit(col, token?)` ↔ `TemporalUnitSelect({value?: string, onChange: (token?:string)=>void})` ↔ `temporalLabel(token?, value)` ↔ cartesian `xAxisUnit?: string` / pie `dimensionUnit?: string` — all consistent across tasks. Tokens identical to sub-project A.
