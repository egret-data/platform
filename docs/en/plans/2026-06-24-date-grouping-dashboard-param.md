# Date Grouping — Sub-project C (Dashboard Date-Grouping Parameter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dashboard parameter (a filter-bar pill) that overrides the temporal grouping unit on the cards it is mapped to, re-gridding all of them together.

**Architecture:** Add a `"temporal_unit"` parameter type. The view-time hook `applyParamsToChart` overrides `spec.temporal_units[mappedColumn]` — but only when that column is already a key in the card's `temporal_units` (already time-grouped by sub-project B), which keeps the backend's orphan-key / non-temporal guards satisfied. Reuses the existing per-card `paramMappings` infrastructure and the unit metadata from `temporal-units.ts`.

**Tech Stack:** React 18 + TS + Vite + Vitest + shadcn.

**Spec:** `docs/en/specs/2026-06-24-date-grouping-dashboard-param-design.md`
**Branch:** `feat/date-grouping-dashboard-param` (already checked out; spec committed).
**Depends on:** A (PR #47, backend `temporal_units`) and B (PR #48, `temporal-units.ts` exporting `TEMPORAL_UNITS` = `{token,label,group}[]`).

**Conventions (every task):** all commands from `frontend/`. Sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Commit footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Do NOT push until Task 5.

---

### Task 1: `temporal_unit` param type + override + summary

**Files:**
- Modify: `frontend/src/api/dashboards.ts` (`ParamType` union, line 26)
- Modify: `frontend/src/components/chart/dashboard/params.ts` (`summarize` ~line 20, `applyParamsToChart` ~line 250)
- Test: `frontend/src/components/chart/dashboard/__tests__/params.temporal.test.ts` (new)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/params.temporal.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { applyParamsToChart, summarize } from "../params";
import type { DashboardParameter } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

const tuParam = (over: Partial<DashboardParameter> = {}): DashboardParameter => ({
  id: "p1", name: "Date grouping", slug: "date_grouping", type: "temporal_unit",
  operator: "is", default: "month", ...over,
});

const chart = (temporal_units: Record<string, string>): Chart => ({
  id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "",
  viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: ["created_at"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [], temporal_units } },
});

describe("applyParamsToChart — temporal_unit", () => {
  it("overrides the unit of an already-grouped column", () => {
    const out = applyParamsToChart(chart({ created_at: "month" }), [tuParam()], { date_grouping: "quarter" }, { p1: "created_at" });
    expect(out.params.spec.temporal_units).toEqual({ created_at: "quarter" });
  });

  it("is a no-op when the mapped column is not already grouped", () => {
    const c = chart({}); // created_at has no unit
    const out = applyParamsToChart(c, [tuParam()], { date_grouping: "quarter" }, { p1: "created_at" });
    expect(out).toBe(c); // unchanged, same reference
  });

  it("falls back to the param default when the value is blank", () => {
    const out = applyParamsToChart(chart({ created_at: "month" }), [tuParam({ default: "year" })], { date_grouping: "" }, { p1: "created_at" });
    expect(out.params.spec.temporal_units).toEqual({ created_at: "year" });
  });

  it("does nothing when the param is not mapped", () => {
    const c = chart({ created_at: "month" });
    const out = applyParamsToChart(c, [tuParam()], { date_grouping: "quarter" }, {});
    expect(out).toBe(c);
  });
});

describe("summarize — temporal_unit", () => {
  it("shows the unit label", () => {
    expect(summarize(tuParam(), "month")).toBe("Month");
    expect(summarize(tuParam(), "day_of_week")).toBe("Day of week");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/params.temporal.test.ts`
Expected: FAIL — `"temporal_unit"` is not assignable to `ParamType`, and override/summary logic absent.

- [ ] **Step 3: Add the type**

In `frontend/src/api/dashboards.ts` line 26, add `"temporal_unit"` to the union:

```ts
export type ParamType = "text" | "number" | "date" | "id" | "category" | "boolean" | "location" | "temporal_unit";
```

- [ ] **Step 4: Override in `applyParamsToChart`**

In `frontend/src/components/chart/dashboard/params.ts`, replace the body of `applyParamsToChart` (currently builds `extra` and appends to `spec.filters`) with:

```ts
export function applyParamsToChart(
  chart: Chart,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  mappings: Record<string, string>,
  now: Date = new Date(),
): Chart {
  const extra: ChartFilter[] = [];
  const temporalOverrides: Record<string, string> = {};
  for (const param of parameters) {
    const column = mappings[param.id];
    if (!column) continue;
    if (param.type === "temporal_unit") {
      const raw = values[param.slug];
      const unit = typeof raw === "string" && raw ? raw : param.default;
      const current = chart.params.spec.temporal_units ?? {};
      if (typeof unit === "string" && unit && column in current) {
        temporalOverrides[column] = unit; // re-grid an already-grouped column
      }
      continue;
    }
    extra.push(...paramToFilters(param, values[param.slug], column, now));
  }
  if (extra.length === 0 && Object.keys(temporalOverrides).length === 0) return chart;
  const spec = { ...chart.params.spec, filters: [...chart.params.spec.filters, ...extra] };
  if (Object.keys(temporalOverrides).length > 0) {
    spec.temporal_units = { ...(chart.params.spec.temporal_units ?? {}), ...temporalOverrides };
  }
  return { ...chart, params: { ...chart.params, spec } };
}
```

(Keep the existing imports; `ChartFilter` and `paramToFilters` are already in scope in this file.)

- [ ] **Step 5: Add the `summarize` branch + import**

At the top of `params.ts`, add the import:

```ts
import { TEMPORAL_UNITS } from "@/components/chart/temporal-units";
```

In `summarize`, immediately after the `if (isBlank(value)) return "";` line, add:

```ts
  if (param.type === "temporal_unit") {
    return TEMPORAL_UNITS.find((u) => u.token === value)?.label ?? String(value);
  }
```

- [ ] **Step 6: Run tests**

Run: `npx vitest run src/components/chart/dashboard/__tests__/params.temporal.test.ts && npx vitest run src/components/chart/dashboard/__tests__/params.test.ts`
Expected: PASS (new temporal tests + the existing `params.test.ts` filter tests, which are unchanged because `temporal_units` is only added to the returned spec when there is an override).

- [ ] **Step 7: Commit**

```bash
git add src/api/dashboards.ts src/components/chart/dashboard/params.ts src/components/chart/dashboard/__tests__/params.temporal.test.ts
git commit -m "feat(dashboards): temporal_unit param type — override temporal_units on mapped grouped columns"
```

---

### Task 2: ParameterEditor — create a Date-grouping parameter

**Files:**
- Modify: `frontend/src/components/chart/dashboard/ParameterEditor.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/ParameterEditor.test.tsx`

- [ ] **Step 1: Write the failing test**

Append to `ParameterEditor.test.tsx` (inside its top-level `describe`, or add one). It selects the type and asserts the default is set and the operator row is hidden:

```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { ParameterEditor } from "../ParameterEditor";
import type { DashboardParameter } from "@/api/dashboards";

it("temporal_unit type sets default month and hides the operator", () => {
  let p: DashboardParameter = { id: "p1", name: "P", slug: "p", type: "text", operator: "is" };
  const onChange = (n: DashboardParameter) => { p = n; };
  const { rerender } = render(<ParameterEditor param={p} onChange={onChange} onDelete={() => {}} />);
  fireEvent.change(screen.getByLabelText("parameter type"), { target: { value: "temporal_unit" } });
  expect(p.type).toBe("temporal_unit");
  expect(p.default).toBe("month");
  rerender(<ParameterEditor param={p} onChange={onChange} onDelete={() => {}} />);
  expect(screen.queryByLabelText("parameter operator")).toBeNull();
  expect(screen.getByLabelText("default unit")).toBeTruthy();
});
```

(If the file already imports `render`/`screen`/`fireEvent`, do not duplicate the imports — add only the `it(...)`.)

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/ParameterEditor.test.tsx`
Expected: FAIL — `temporal_unit` is not an option / default not set / no "default unit" control.

- [ ] **Step 3: Add `temporal_unit` to OPERATORS + the default-unit control**

In `ParameterEditor.tsx`:

Add the import:
```ts
import { TEMPORAL_UNITS } from "@/components/chart/temporal-units";
```

Add a `temporal_unit` entry to `OPERATORS` (operator is unused for this type; a placeholder keeps the `Record<ParamType, ParamOperator[]>` shape and `OPERATORS[t][0]` valid):
```ts
export const OPERATORS: Record<ParamType, ParamOperator[]> = {
  text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
  number: ["=", "!=", "between", ">=", "<="],
  date: ["range", "relative", "single", "month-year", "quarter-year", "all-options"],
  id: ["is"],
  category: ["is"],
  boolean: ["is"],
  location: ["is", "is-not"],
  temporal_unit: ["is"],
};
```

Change the type `<select>` onChange (line 25) to also default the unit when switching to temporal_unit:
```tsx
        onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0], ...(t === "temporal_unit" ? { default: "month" } : {}) }); }}>
```

After the operator `<select>` block (the `{showOp && (… )}` expression), add the default-unit picker:
```tsx
      {param.type === "temporal_unit" && (
        <select aria-label="default unit" className={cls} value={String(param.default ?? "month")}
          onChange={(e) => onChange({ ...param, default: e.target.value })}>
          {TEMPORAL_UNITS.map((u) => <option key={u.token} value={u.token}>{u.label}</option>)}
        </select>
      )}
```

- [ ] **Step 4: Run tests**

Run: `npx vitest run src/components/chart/dashboard/__tests__/ParameterEditor.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/chart/dashboard/ParameterEditor.tsx src/components/chart/dashboard/__tests__/ParameterEditor.test.tsx
git commit -m "feat(dashboards): create a Date-grouping (temporal_unit) parameter in the editor"
```

---

### Task 3: FilterPill — unit dropdown widget + Clock icon

**Files:**
- Modify: `frontend/src/components/chart/dashboard/FilterPill.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/FilterPill.test.tsx`

- [ ] **Step 1: Write the failing test**

Append to `FilterPill.test.tsx`:

```tsx
it("temporal_unit renders a unit dropdown and selects a unit", () => {
  const onChange = vi.fn();
  render(<FilterPill
    param={{ id: "p", name: "Date grouping", slug: "dg", type: "temporal_unit", operator: "is", default: "month" }}
    value="month" options={[]} onChange={onChange} />);
  fireEvent.click(screen.getByLabelText("Date grouping"));
  const sel = screen.getByLabelText("Date grouping value") as HTMLSelectElement;
  expect(sel).toBeTruthy();
  fireEvent.change(sel, { target: { value: "quarter" } });
  expect(onChange).toHaveBeenCalledWith("quarter");
});
```

(Reuse the existing imports in the file — `render`, `screen`, `fireEvent`, `vi`. If any are missing, add them to the existing import line.)

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.test.tsx`
Expected: FAIL — no unit dropdown for `temporal_unit`.

- [ ] **Step 3: Add the Clock icon**

In `FilterPill.tsx`, change the lucide import to include `Clock`:
```ts
import { Calendar, Clock, Hash, MapPin, Tag, ToggleLeft, Type, X } from "lucide-react";
```
Add to `iconFor` (before `return Type;`):
```ts
  if (p.type === "temporal_unit") return Clock;
```

- [ ] **Step 4: Add the widget + import**

Add the import near the top:
```ts
import { TEMPORAL_UNITS } from "@/components/chart/temporal-units";
```
In `FilterControl`, add this branch immediately after the `boolean` branch (right after its closing `}`):
```tsx
  if (param.type === "temporal_unit") {
    return (
      <select aria-label={`${param.name} value`} className={input}
        value={String(value ?? param.default ?? "month")}
        onChange={(e) => onChange(e.target.value)}>
        <optgroup label="Group by">
          {TEMPORAL_UNITS.filter((u) => u.group === "trunc").map((u) => (
            <option key={u.token} value={u.token}>{u.label}</option>
          ))}
        </optgroup>
        <optgroup label="Extract">
          {TEMPORAL_UNITS.filter((u) => u.group === "extract").map((u) => (
            <option key={u.token} value={u.token}>{u.label}</option>
          ))}
        </optgroup>
      </select>
    );
  }
```

- [ ] **Step 5: Run tests**

Run: `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.test.tsx`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/components/chart/dashboard/FilterPill.tsx src/components/chart/dashboard/__tests__/FilterPill.test.tsx
git commit -m "feat(dashboards): temporal_unit filter pill — grouped unit dropdown + Clock icon"
```

---

### Task 4: Mapping picker — offer only grouped columns

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (the `CardFilterMapOverlay` `columns` prop, ~line 436)

- [ ] **Step 1: Restrict the column options for a temporal_unit param**

Find the `CardFilterMapOverlay` render (inside the cards map, guarded by `activeParamId && c.card_type === "chart" && baseChart`). Replace its `columns={[...baseChart.params.spec.dimensions, ...baseChart.params.spec.columns]}` prop with:

```tsx
                    columns={
                      parameters.find((pp) => pp.id === activeParamId)?.type === "temporal_unit"
                        ? Object.keys(baseChart.params.spec.temporal_units ?? {})
                        : [...baseChart.params.spec.dimensions, ...baseChart.params.spec.columns]
                    }
```

(So when the active parameter is a date-grouping parameter, the per-card column picker lists only that card's already-time-grouped columns — the valid targets. Other parameter types are unchanged.)

- [ ] **Step 2: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green (existing dashboard tests unaffected; this only narrows the option list for one param type).

- [ ] **Step 3: Commit**

```bash
git add src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): map a Date-grouping param only to a card's time-grouped columns"
```

---

### Task 5: Live verify + PR

**Files:** none (verification + PR)

- [ ] **Step 1: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green.

- [ ] **Step 2: Live verify (Playwright, dev backend rebuilt with A+B, throwaway dashboard NOT `a1`, 0 console errors)**

1. Create a throwaway dashboard with two bar cards, each grouped by a timestamp column (X-axis = `created_at`, auto-defaulted to Month by sub-project B).
2. Enter edit mode → add a parameter, set its type to **temporal_unit** (default Month) → map it to each card's `created_at` column (the mapping picker should offer only `created_at`).
3. Save, exit edit → the filter bar shows a **Clock** pill "Date grouping: Month".
4. Change the pill to **Quarter** → both cards re-grid to quarterly buckets together.
5. A card NOT mapped (add a third card, leave it unmapped) is unaffected.
6. Reload → the parameter, its mapping, and the selected unit persist.
Delete the throwaway dashboard afterward.

- [ ] **Step 3: Push + open PR**

```bash
git push -u origin feat/date-grouping-dashboard-param
gh pr create --base main --title "feat(dashboards): Date-grouping parameter (Date Grouping C)" --body "<summary + live-verify notes>"
```

---

## Self-Review

**Spec coverage:** ParamType `temporal_unit` (Task 1) ✓; override-only-already-grouped rule in `applyParamsToChart` (Task 1) ✓; `summarize` label (Task 1) ✓; editor create + default month + hidden operator + default-unit picker (Task 2) ✓; view-mode unit dropdown + Clock icon (Task 3) ✓; mapping picker filtered to grouped columns (Task 4) ✓; live verify of re-grid-together + persistence (Task 5) ✓. Non-goals (no backend, no raw-column grouping, no new mapping infra) respected.

**Placeholder scan:** every code step shows full code; no TBD/TODO.

**Type consistency:** `ParamType` += `"temporal_unit"` ↔ `OPERATORS.temporal_unit` ↔ `applyParamsToChart` override keyed on `chart.params.spec.temporal_units` (the optional `Record<string,string>` added in sub-project B) ↔ `summarize` using `TEMPORAL_UNITS` `{token,label,group}` ↔ FilterPill/editor reading `param.default`. The override writes `spec.temporal_units` only when an override exists, so existing filter-only `applyParamsToChart` tests stay green. All consistent with B's `temporal-units.ts` exports.
