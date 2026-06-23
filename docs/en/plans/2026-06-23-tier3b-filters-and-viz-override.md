# Tier 3b — Date picker, value sources, per-card viz override — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Metabase-style unified date filter picker, custom/linked filter value sources, and per-card viz-settings overrides to egret dashboards.

**Architecture:** Frontend-only — parameters live in `dashboards.parameters` jsonb, per-card overrides in `dashboard_cards.options` jsonb; no backend or migration. Three groups: (A) a new `all-options` date operator + `DateFilterPicker`; (B) a `valuesSource` on a parameter resolved in the option loader; (C) a display-only `vizOverride` on a card, a shared `ChartSettingsSections` (extracted from the chart builder), and a dashboard viz panel that merges the override over the saved chart at render.

**Tech Stack:** React 18 + TS + Vite, Vitest + @testing-library/react. Canonical spec: `docs/en/specs/2026-06-23-tier3b-filters-and-viz-override-design.md`.

**Reused contracts (verified):**
- `ChartParams` (`api/charts.ts`): `{ spec: QuerySpec; columnSettings?; conditionalFormatting?; tableState?; cartesian?; pie?; scalar?; progress?; gauge?; funnel?; waterfall?; smartscalar?; boxplot?; sankey?; rows?; cols?; metrics?; showRowTotals?; showColTotals?; condense?; collapsedRows?; pivotSort? }`. Every field except `spec` is **display**. `Chart { viz_type: VizType; params: ChartParams; ... }`.
- Section components in `components/chart/settings/`: `CartesianDisplaySection`, `PieDisplaySection`, `FormatSection`, `ConditionalFormattingSection`, `TableColumnsSection`. The chart builder's **Display tab** (`ChartExplore.tsx:719-755`) composes them.
- `params.ts`: `paramToFilters(param, value, column, now?)`, `summarize(param, value)`, `isBlank`, `applyParamsToChart`, `computeLinkedFilters`. `mkf(op, v) => ({col, op, value:v})`. Ops: `eq|ne|gt|ge|lt|le|like|in|not_in|is_null|is_not_null` (no `not_like`).
- `DashboardCardView.tsx` renders `<ChartRenderer vizType={chart.viz_type} params={chart.params} columns rows onValueClick />`; the dashboard injects param filters via `applyParamsToChart` upstream (in `DashboardView`).
- `useColumnValues(ws, datasetId, column, enabled, filters?)`. `ParamOptions` loader + `editPanel` in `DashboardView.tsx`. `EditSidebar` `EditPanel` union. `CardEditToolbar`.

**Conventions for every task:** TDD (red → green). Full sweep before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Configured git identity. End every commit message with:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Branch `feat/dashboards-tier3b` (spec already committed). All commands run from `frontend/`.

**Deferred (do not implement):** any backend change; `does-not-contain`; date "Exclude" tab; value-source "from connected fields" traversal; overriding the query per card (vizOverride is display-only).

---

## Task order
A: Tasks 1–4 (date all-options) · B: Tasks 5–7 (value sources) · C: Tasks 8–11 (viz override) · D: Task 12 (live + PR).

---

### Task 1: Date `all-options` operator (data model)

**Files:**
- Modify: `frontend/src/api/dashboards.ts` (`ParamOperator` union)
- Modify: `frontend/src/components/chart/dashboard/ParameterEditor.tsx` (`OPERATORS.date`)
- Test: `frontend/src/components/chart/dashboard/__tests__/operators.test.ts` (extend the existing file)

- [ ] **Step 1: Write the failing test** — add to `operators.test.ts`:

```ts
it("date includes the all-options operator", () => {
  expect(OPERATORS.date).toContain("all-options");
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/operators.test.ts` — FAIL.

- [ ] **Step 3:** In `api/dashboards.ts`, add `"all-options"` to the `ParamOperator` union's date group (the line ending `| "range" | "relative" | "single" | "month-year" | "quarter-year"`) → append `| "all-options"`.

- [ ] **Step 4:** In `ParameterEditor.tsx`, change `date:` in `OPERATORS` to include it:
```ts
  date: ["range", "relative", "single", "month-year", "quarter-year", "all-options"],
```

- [ ] **Step 5: Run** the test — PASS.

- [ ] **Step 6: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/dashboards.ts src/components/chart/dashboard/ParameterEditor.tsx src/components/chart/dashboard/__tests__/operators.test.ts
git commit -m "feat(dashboards): add all-options date operator

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `paramToFilters` + `summarize` for `all-options`

**Files:**
- Modify: `frontend/src/components/chart/dashboard/params.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/params.alloptions.test.ts` (create)

**Value shape (structured object stored in the param value map):**
`{ mode: "relative", dir: "past"|"next"|"current", n: number, unit: "day"|"week"|"month"|"quarter"|"year" }`
or `{ mode: "specific", op: "on"|"before"|"after"|"between", from: string /*YYYY-MM-DD*/, to?: string }`.

- [ ] **Step 1: Write the failing test** `params.alloptions.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { paramToFilters, summarize } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "D", slug: "d", type: "date", operator: "all-options", ...over });
const NOW = new Date("2026-06-15T00:00:00Z");

describe("paramToFilters all-options", () => {
  it("relative past 7 days → ge/le around now", () => {
    const f = paramToFilters(p({}), { mode: "relative", dir: "past", n: 7, unit: "day" }, "d", NOW);
    expect(f).toEqual([{ col: "d", op: "ge", value: "2026-06-08" }, { col: "d", op: "le", value: "2026-06-15" }]);
  });
  it("relative next 2 months → ge now / le now+2mo", () => {
    const f = paramToFilters(p({}), { mode: "relative", dir: "next", n: 2, unit: "month" }, "d", NOW);
    expect(f).toEqual([{ col: "d", op: "ge", value: "2026-06-15" }, { col: "d", op: "le", value: "2026-08-15" }]);
  });
  it("specific on → eq; before → lt; after → gt; between → ge+le", () => {
    expect(paramToFilters(p({}), { mode: "specific", op: "on", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "eq", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "before", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "lt", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "after", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "gt", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" }, "d", NOW))
      .toEqual([{ col: "d", op: "ge", value: "2026-01-01" }, { col: "d", op: "le", value: "2026-03-31" }]);
  });
  it("blank/partial → []", () => {
    expect(paramToFilters(p({}), undefined, "d", NOW)).toEqual([]);
    expect(paramToFilters(p({}), { mode: "specific", op: "between", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "ge", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "on", from: "" }, "d", NOW)).toEqual([]);
  });
});

describe("summarize all-options", () => {
  it("labels relative + specific", () => {
    expect(summarize(p({}), { mode: "relative", dir: "past", n: 7, unit: "day" })).toBe("Past 7 days");
    expect(summarize(p({}), { mode: "relative", dir: "next", n: 1, unit: "month" })).toBe("Next 1 month");
    expect(summarize(p({}), { mode: "relative", dir: "current", n: 0, unit: "week" })).toBe("Current week");
    expect(summarize(p({}), { mode: "specific", op: "after", from: "2026-01-01" })).toBe("After 2026-01-01");
    expect(summarize(p({}), { mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" })).toBe("2026-01-01 – 2026-03-31");
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/params.alloptions.test.ts` — FAIL.

- [ ] **Step 3: Implement.** In `params.ts`, add a helper to shift a date by a unit and an `all-options` branch.

Add near the top (after `iso`):
```ts
type DateUnit = "day" | "week" | "month" | "quarter" | "year";
function shift(d: Date, n: number, unit: DateUnit): Date {
  const r = new Date(d);
  if (unit === "day") r.setUTCDate(r.getUTCDate() + n);
  else if (unit === "week") r.setUTCDate(r.getUTCDate() + n * 7);
  else if (unit === "month") r.setUTCMonth(r.getUTCMonth() + n);
  else if (unit === "quarter") r.setUTCMonth(r.getUTCMonth() + n * 3);
  else r.setUTCFullYear(r.getUTCFullYear() + n);
  return r;
}
```

In `paramToFilters`, inside `case "date":`, add **before** the existing `single`/`range`/`relative`/`month-year`/`quarter-year` handling (so the operator branch is reached) — guard on the operator:
```ts
      if (param.operator === "all-options") {
        const v = value as { mode?: string; dir?: string; n?: number; unit?: DateUnit; op?: string; from?: string; to?: string } | undefined;
        if (!v || !v.mode) return [];
        if (v.mode === "relative") {
          const unit = v.unit ?? "day";
          if (v.dir === "current") {
            // current period: from start..end is approximated as [now, now]; keep simple → [eq now] omitted, use bounds now..now
            return [mkf("ge", iso(now)), mkf("le", iso(now))];
          }
          const n = Number(v.n) || 0;
          const other = v.dir === "next" ? shift(now, n, unit) : shift(now, -n, unit);
          const [lo, hi] = v.dir === "next" ? [now, other] : [other, now];
          return [mkf("ge", iso(lo)), mkf("le", iso(hi))];
        }
        // specific
        if (isBlank(v.from)) return [];
        const from = String(v.from);
        switch (v.op) {
          case "on": return [mkf("eq", from)];
          case "before": return [mkf("lt", from)];
          case "after": return [mkf("gt", from)];
          case "between": {
            const out = [mkf("ge", from)];
            if (!isBlank(v.to)) out.push(mkf("le", String(v.to)));
            return out;
          }
          default: return [mkf("eq", from)];
        }
      }
```

In `summarize`, add **before** the existing date-relative branch:
```ts
  if (param.type === "date" && param.operator === "all-options") {
    const v = value as { mode?: string; dir?: string; n?: number; unit?: string; op?: string; from?: string; to?: string } | undefined;
    if (!v || !v.mode) return "";
    if (v.mode === "relative") {
      const unit = v.unit ?? "day";
      if (v.dir === "current") return `Current ${unit}`;
      const word = v.dir === "next" ? "Next" : "Past";
      const plural = (v.n ?? 0) === 1 ? unit : `${unit}s`;
      return `${word} ${v.n} ${plural}`;
    }
    if (v.op === "between") return `${v.from} – ${v.to ?? ""}`.trim();
    const opWord = v.op === "after" ? "After" : v.op === "before" ? "Before" : "On";
    return v.from ? `${opWord} ${v.from}` : "";
  }
```

- [ ] **Step 4: Run** the test — PASS. Also run the whole `__tests__` dir to confirm no regression.

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/params.ts src/components/chart/dashboard/__tests__/params.alloptions.test.ts
git commit -m "feat(dashboards): translate all-options date values to filters

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `DateFilterPicker` component

**Files:**
- Create: `frontend/src/components/chart/dashboard/DateFilterPicker.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DateFilterPicker.test.tsx` (create)

- [ ] **Step 1: Write the failing test** `DateFilterPicker.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DateFilterPicker } from "../DateFilterPicker";

describe("DateFilterPicker", () => {
  it("relative tab emits a relative value", () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={undefined} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Relative" }));
    fireEvent.change(screen.getByLabelText("relative n"), { target: { value: "7" } });
    fireEvent.change(screen.getByLabelText("relative unit"), { target: { value: "day" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "relative", dir: "past", n: 7, unit: "day" }));
  });
  it("specific tab between emits from+to", () => {
    const onChange = vi.fn();
    render(<DateFilterPicker value={undefined} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Specific" }));
    fireEvent.change(screen.getByLabelText("specific op"), { target: { value: "between" } });
    fireEvent.change(screen.getByLabelText("specific from"), { target: { value: "2026-01-01" } });
    fireEvent.change(screen.getByLabelText("specific to"), { target: { value: "2026-03-31" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" }));
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/DateFilterPicker.test.tsx` — FAIL (module missing).

- [ ] **Step 3: Implement** `DateFilterPicker.tsx`:

```tsx
import { useState } from "react";

type RelVal = { mode: "relative"; dir: "past" | "next" | "current"; n: number; unit: "day" | "week" | "month" | "quarter" | "year" };
type SpecVal = { mode: "specific"; op: "on" | "before" | "after" | "between"; from: string; to?: string };
export type DateAllOptionsValue = RelVal | SpecVal;

interface Props { value: DateAllOptionsValue | undefined; onChange: (v: DateAllOptionsValue) => void; }
const input = "h-8 rounded border bg-background px-2 text-sm";

export function DateFilterPicker({ value, onChange }: Props) {
  const [tab, setTab] = useState<"relative" | "specific">(value?.mode === "specific" ? "specific" : "relative");
  const rel: RelVal = value?.mode === "relative" ? value : { mode: "relative", dir: "past", n: 7, unit: "day" };
  const spec: SpecVal = value?.mode === "specific" ? value : { mode: "specific", op: "on", from: "" };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-1">
        <button type="button" aria-label="Relative" className={`rounded px-2 py-1 text-sm ${tab === "relative" ? "bg-accent" : ""}`}
          onClick={() => setTab("relative")}>Relative</button>
        <button type="button" aria-label="Specific" className={`rounded px-2 py-1 text-sm ${tab === "specific" ? "bg-accent" : ""}`}
          onClick={() => setTab("specific")}>Specific</button>
      </div>
      {tab === "relative" ? (
        <div className="flex items-center gap-1">
          <select aria-label="relative dir" className={input} value={rel.dir} onChange={(e) => onChange({ ...rel, dir: e.target.value as RelVal["dir"] })}>
            <option value="past">Past</option><option value="next">Next</option><option value="current">Current</option>
          </select>
          {rel.dir !== "current" && (
            <input aria-label="relative n" type="number" min={1} className={`${input} w-16`} value={rel.n}
              onChange={(e) => onChange({ ...rel, n: Number(e.target.value) || 0 })} />
          )}
          <select aria-label="relative unit" className={input} value={rel.unit} onChange={(e) => onChange({ ...rel, unit: e.target.value as RelVal["unit"] })}>
            <option value="day">days</option><option value="week">weeks</option><option value="month">months</option>
            <option value="quarter">quarters</option><option value="year">years</option>
          </select>
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <select aria-label="specific op" className={input} value={spec.op} onChange={(e) => onChange({ ...spec, op: e.target.value as SpecVal["op"] })}>
            <option value="on">On</option><option value="before">Before</option><option value="after">After</option><option value="between">Between</option>
          </select>
          <input aria-label="specific from" type="date" className={input} value={spec.from} onChange={(e) => onChange({ ...spec, from: e.target.value })} />
          {spec.op === "between" && (
            <input aria-label="specific to" type="date" className={input} value={spec.to ?? ""} onChange={(e) => onChange({ ...spec, to: e.target.value })} />
          )}
        </div>
      )}
    </div>
  );
}
```

Note: on the "relative" tab, changing the dir to `current` emits `{...rel, dir:"current"}` (n is ignored by `paramToFilters`). Switching tabs alone does not emit; an actual control change does — matching the test (it changes a control after switching).

- [ ] **Step 4: Run** the test — PASS.

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/DateFilterPicker.tsx src/components/chart/dashboard/__tests__/DateFilterPicker.test.tsx
git commit -m "feat(dashboards): DateFilterPicker (relative + specific tabs)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Wire `DateFilterPicker` into `FilterPill`

**Files:**
- Modify: `frontend/src/components/chart/dashboard/FilterPill.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/FilterPill.alloptions.test.tsx` (create)

- [ ] **Step 1: Write the failing test** `FilterPill.alloptions.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p: DashboardParameter = { id: "1", name: "When", slug: "when", type: "date", operator: "all-options" };

describe("FilterPill all-options", () => {
  it("opens the DateFilterPicker and emits a relative value", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p} value={undefined} options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "When" }));
    fireEvent.click(screen.getByRole("button", { name: "Relative" }));
    fireEvent.change(screen.getByLabelText("relative n"), { target: { value: "3" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "relative", n: 3 }));
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.alloptions.test.tsx` — FAIL.

- [ ] **Step 3: Implement.** In `FilterPill.tsx`:
1. Import: `import { DateFilterPicker } from "./DateFilterPicker";`
2. In `FilterControl`, add a branch BEFORE the existing `date` branches:
```tsx
  if (param.type === "date" && param.operator === "all-options") {
    return <DateFilterPicker value={value as never} onChange={(v) => onChange(v)} />;
  }
```
3. In `clearedValue`, `all-options` should clear to `""` (the default `"" else`) — no change needed since it's not category/location/between/range; `hasValue`/`summarize` handle the object value (an object is truthy and non-blank). Confirm `hasValue(value)` returns true for a non-empty object: it does (`!isBlank(object)` is true, not an array). Good.

- [ ] **Step 4: Run** the test — PASS, plus the whole `__tests__` dir (no regression).

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/FilterPill.tsx src/components/chart/dashboard/__tests__/FilterPill.alloptions.test.tsx
git commit -m "feat(dashboards): FilterPill renders DateFilterPicker for all-options

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `ParamValuesSource` data model

**Files:**
- Modify: `frontend/src/api/dashboards.ts`
- Test: none (pure type addition; covered by Task 6/7 tests)

- [ ] **Step 1:** In `api/dashboards.ts`, add:
```ts
export interface ParamValuesSource {
  type: "distinct" | "static" | "card";
  static?: string[];
  cardId?: string;
  column?: string;
}
```
and add `valuesSource?: ParamValuesSource;` to the `DashboardParameter` interface.

- [ ] **Step 2: Full sweep + commit** (typecheck proves it compiles):
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/dashboards.ts
git commit -m "feat(dashboards): ParamValuesSource type for filter value sources

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `FilterSettingsPanel` — value source control

**Files:**
- Modify: `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`
- Modify: `frontend/src/components/chart/dashboard/EditSidebar.tsx` (pass cards/charts so the panel can offer card+column — see note)
- Test: `frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.valuesource.test.tsx` (create)

**Note on inputs:** the "From a card column" sub-control needs the list of dashboard chart cards and their columns. To keep `FilterSettingsPanel` presentational, add an optional prop `cardColumns?: { cardId: string; label: string; columns: string[] }[]` (default `[]`); `EditSidebar`/`DashboardView` build it. The "From a card column" radio is only fully usable when `cardColumns` is non-empty; otherwise show the radio but with an empty card select.

- [ ] **Step 1: Write the failing test** `FilterSettingsPanel.valuesource.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("FilterSettingsPanel value sources", () => {
  it("Custom list writes a static array (one value per line)", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("values from custom"));
    fireEvent.change(screen.getByLabelText("custom values"), { target: { value: "West\nEast\n" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ valuesSource: { type: "static", static: ["West", "East"] } }));
  });
  it("From a card writes card + column", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()}
      cardColumns={[{ cardId: "c1", label: "Sales", columns: ["region", "city"] }]} />);
    fireEvent.click(screen.getByLabelText("values from card"));
    fireEvent.change(screen.getByLabelText("values card"), { target: { value: "c1" } });
    fireEvent.change(screen.getByLabelText("values column"), { target: { value: "city" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ valuesSource: { type: "card", cardId: "c1", column: "city" } }));
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/FilterSettingsPanel.valuesource.test.tsx` — FAIL.

- [ ] **Step 3: Implement.** In `FilterSettingsPanel.tsx`:
1. Extend `Props` with `cardColumns?: { cardId: string; label: string; columns: string[] }[]` (default `[]` in the destructure).
2. Add a value-source section, shown when `showWidget` is true (category/id/location/text). Place it after the widget radio block:
```tsx
      {showWidget && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">Where do values come from?</div>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`vs-${param.id}`} aria-label="values from distinct" checked={(param.valuesSource?.type ?? "distinct") === "distinct"}
              onChange={() => onChange({ ...param, valuesSource: { type: "distinct" } })} /> From the data (distinct)
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`vs-${param.id}`} aria-label="values from custom" checked={param.valuesSource?.type === "static"}
              onChange={() => onChange({ ...param, valuesSource: { type: "static", static: param.valuesSource?.static ?? [] } })} /> Custom list
          </label>
          {param.valuesSource?.type === "static" && (
            <textarea aria-label="custom values" className="h-20 w-full rounded border bg-background px-2 py-1 text-sm"
              value={(param.valuesSource.static ?? []).join("\n")}
              onChange={(e) => onChange({ ...param, valuesSource: { type: "static", static: e.target.value.split("\n").map((s) => s.trim()).filter(Boolean) } })} />
          )}
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`vs-${param.id}`} aria-label="values from card" checked={param.valuesSource?.type === "card"}
              onChange={() => onChange({ ...param, valuesSource: { type: "card", cardId: cardColumns[0]?.cardId, column: undefined } })} /> From a card column
          </label>
          {param.valuesSource?.type === "card" && (
            <div className="flex gap-1">
              <select aria-label="values card" className={cls} value={param.valuesSource.cardId ?? ""}
                onChange={(e) => onChange({ ...param, valuesSource: { type: "card", cardId: e.target.value, column: param.valuesSource?.column } })}>
                <option value="">card…</option>
                {cardColumns.map((c) => <option key={c.cardId} value={c.cardId}>{c.label}</option>)}
              </select>
              <select aria-label="values column" className={cls} value={param.valuesSource.column ?? ""}
                onChange={(e) => onChange({ ...param, valuesSource: { type: "card", cardId: param.valuesSource?.cardId, column: e.target.value } })}>
                <option value="">column…</option>
                {(cardColumns.find((c) => c.cardId === param.valuesSource?.cardId)?.columns ?? []).map((col) => <option key={col} value={col}>{col}</option>)}
              </select>
            </div>
          )}
        </div>
      )}
```
   (`cls` is the existing input class const in the file.)

3. In `EditSidebar.tsx`, thread an optional `cardColumns` prop through to `FilterSettingsPanel` (add to `Props`, default `[]`, pass it). `DashboardView` will supply it in Task 7; for now `EditSidebar` passes `props.cardColumns ?? []`.

- [ ] **Step 4: Run** the test — PASS, plus the existing `FilterSettingsPanel.test.tsx` (no regression).

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/FilterSettingsPanel.tsx src/components/chart/dashboard/EditSidebar.tsx src/components/chart/dashboard/__tests__/FilterSettingsPanel.valuesource.test.tsx
git commit -m "feat(dashboards): filter value-source control (distinct/custom/from-card)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Resolve value sources in the option loader

**Files:**
- Create: `frontend/src/components/chart/dashboard/valuesSource.ts` (pure resolver)
- Test: `frontend/src/components/chart/dashboard/__tests__/valuesSource.test.ts` (create)
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (`ParamOptions` loader + supply `cardColumns` to the sidebar)

- [ ] **Step 1: Write the failing test** `valuesSource.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { resolveValuesSource } from "../valuesSource";
import type { DashboardParameter } from "@/api/dashboards";

const param = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "p", name: "P", slug: "p", type: "category", operator: "is", ...over });

// minimal card/chart fixtures
const cards = [
  { id: "c1", card_type: "chart", chart_id: "ch1", options: { paramMappings: { p: "region" } } },
  { id: "c2", card_type: "chart", chart_id: "ch2", options: {} },
] as never[];
const chartById = new Map<string, { dataset_id: string }>([["ch1", { dataset_id: "ds1" }], ["ch2", { dataset_id: "ds2" }]]);

describe("resolveValuesSource", () => {
  it("static → returns the static list", () => {
    const r = resolveValuesSource(param({ valuesSource: { type: "static", static: ["A", "B"] } }), cards, chartById);
    expect(r).toEqual({ kind: "static", values: ["A", "B"] });
  });
  it("distinct (default) → the mapped card's dataset + column", () => {
    const r = resolveValuesSource(param({}), cards, chartById);
    expect(r).toEqual({ kind: "query", datasetId: "ds1", column: "region" });
  });
  it("card → the chosen card's dataset + chosen column", () => {
    const r = resolveValuesSource(param({ valuesSource: { type: "card", cardId: "c2", column: "city" } }), cards, chartById);
    expect(r).toEqual({ kind: "query", datasetId: "ds2", column: "city" });
  });
  it("card with missing card → null", () => {
    const r = resolveValuesSource(param({ valuesSource: { type: "card", cardId: "nope", column: "x" } }), cards, chartById);
    expect(r).toBeNull();
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/valuesSource.test.ts` — FAIL.

- [ ] **Step 3: Implement** `valuesSource.ts`:

```ts
import type { DashboardParameter } from "@/api/dashboards";

type CardLike = { id: string; card_type: string; chart_id: string | null; options: Record<string, unknown> };
type ChartLike = { dataset_id: string };

export type ResolvedSource =
  | { kind: "static"; values: string[] }
  | { kind: "query"; datasetId: string; column: string }
  | null;

/** Resolve a parameter's value source to either a static list or a (dataset, column) query target. */
export function resolveValuesSource(
  param: DashboardParameter,
  cards: CardLike[],
  chartById: Map<string, ChartLike>,
): ResolvedSource {
  const src = param.valuesSource;
  if (src?.type === "static") return { kind: "static", values: src.static ?? [] };
  if (src?.type === "card") {
    if (!src.cardId || !src.column) return null;
    const card = cards.find((c) => c.id === src.cardId && c.card_type === "chart");
    const chart = card?.chart_id ? chartById.get(card.chart_id) : undefined;
    if (!chart) return null;
    return { kind: "query", datasetId: chart.dataset_id, column: src.column };
  }
  // distinct (default): the card that maps this param → (dataset, column)
  const card = cards.find((c) => c.card_type === "chart" && (((c.options.paramMappings as Record<string, string>) ?? {})[param.id]));
  const chart = card?.chart_id ? chartById.get(card.chart_id) : undefined;
  const column = card ? ((card.options.paramMappings as Record<string, string>) ?? {})[param.id] : undefined;
  if (!chart || !column) return null;
  return { kind: "query", datasetId: chart.dataset_id, column };
}
```

- [ ] **Step 4: Run** the test — PASS.

- [ ] **Step 5: Wire into `DashboardView.tsx`.**
1. Import `resolveValuesSource`.
2. Create a `StaticOptions` helper component (returns null, emits on mount) next to `ParamOptions`:
```tsx
function StaticOptions({ slug, values, onLoaded }: { slug: string; values: string[]; onLoaded: (slug: string, v: string[]) => void }) {
  useEffect(() => { onLoaded(slug, values); }, [slug, values, onLoaded]);
  return null;
}
```
3. Replace the option-loader `.map` body so it resolves the source. For each param `p` (keep the same `type` filter — category/id/location), compute `const resolved = resolveValuesSource(p, cards, chartById);` then:
   - `null` → `return null;`
   - `static` → `return <StaticOptions key={p.id} slug={p.slug} values={resolved.values} onLoaded={onOptionsLoaded} />;`
   - `query` → compute `filters` via `computeLinkedFilters(p, cardMappings, parameters, paramsState.values)` (cardMappings from the param's mapped card — for `card`-source you may pass `{}` since linked filters only meaningfully apply to the param's own card; keep distinct's existing linked behaviour by using the mapped card's mappings when source is distinct, else `[]`) and `return <ParamOptions key={p.id} ws={ws} datasetId={resolved.datasetId} column={resolved.column} slug={p.slug} filters={filters} onLoaded={onOptionsLoaded} />;`
   Keep the existing `ParamOptions` component as-is.
4. Build `cardColumns` for the sidebar and pass it to `EditSidebar`: from chart cards, `{ cardId: c.id, label: chartName, columns: [...dimensions, ...columns] }`. Pass `cardColumns={cardColumns}` to `<EditSidebar .../>` (which forwards to `FilterSettingsPanel` from Task 6).

- [ ] **Step 6: Run** the whole dashboard + explore test dirs (no regression): `npx vitest run src/components/chart/dashboard/__tests__ src/routes/explore/__tests__`.

- [ ] **Step 7: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/valuesSource.ts src/components/chart/dashboard/__tests__/valuesSource.test.ts src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): resolve filter value sources (static/card/distinct) in the option loader

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Extract `ChartSettingsSections` (shared display-settings editor)

**Files:**
- Create: `frontend/src/components/chart/settings/ChartSettingsSections.tsx`
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (use the new component; behaviour identical)
- Test: `frontend/src/components/chart/settings/__tests__/ChartSettingsSections.test.tsx` (create)

This is a **refactor**: move the chart builder's Display-tab section composition (`ChartExplore.tsx:719-755` — the `CartesianDisplaySection` / `PieDisplaySection` / `TableColumnsSection` / `FormatSection` / `ConditionalFormattingSection` blocks, including the derived `cartesianSeries` / `pieSlices` / `resultColumns` / `styleColumns` they consume) into one reusable component keyed by `vizType` + `params` + result `columns`/`rows`, emitting a new `ChartParams` via `onChange`. ChartExplore must behave **identically** — its existing tests are the guard.

- [ ] **Step 1: Read** `ChartExplore.tsx` fully to identify: the Display-tab JSX (≈719-755), the derived values `cartesianSeries` (≈222), `pieSlices` (≈226), `resultColumns` (≈210-214), `styleColumns` (≈214-217), and the local state `cartesian`/`pie`/`tableState`/`columnSettings`/`conditionalFormatting`. Note `SectionLabel` (a small local component) — either move a copy into the new file or import it if exported.

- [ ] **Step 2: Write the failing test** `ChartSettingsSections.test.tsx` (proves the component renders the cartesian section and round-trips an onChange):

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartSettingsSections } from "../ChartSettingsSections";
import type { ChartParams } from "@/api/charts";

const base: ChartParams = { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] }, cartesian: { yMetrics: ["rev"], xAxis: "month" } };
const columns = [{ name: "month", type: "VARCHAR" }, { name: "rev", type: "DOUBLE" }] as never;

describe("ChartSettingsSections", () => {
  it("renders the cartesian display section for a bar viz and emits onChange", () => {
    const onChange = vi.fn();
    render(<ChartSettingsSections vizType={"bar" as never} params={base} columns={columns} rows={[]} onChange={onChange} />);
    // CartesianDisplaySection renders some labelled control; flipping any control should call onChange with a ChartParams
    const toggles = screen.getAllByRole("switch");
    expect(toggles.length).toBeGreaterThan(0);
    fireEvent.click(toggles[0]);
    expect(onChange).toHaveBeenCalled();
    expect(onChange.mock.calls[0][0]).toHaveProperty("spec"); // emits a full ChartParams
  });
});
```
(If `CartesianDisplaySection` exposes no `role="switch"`, adapt the assertion to a control it does render — read the component first. The load-bearing assertion is: a control change calls `onChange` with an object that still has `spec`.)

- [ ] **Step 3: Run** the test — FAIL (module missing).

- [ ] **Step 4: Create `ChartSettingsSections.tsx`.** It takes:
```tsx
interface Props {
  vizType: VizType;
  params: ChartParams;
  columns: { name: string; type: string }[];   // result columns from the card/chart data
  rows?: unknown[][];
  onChange: (next: ChartParams) => void;
}
```
Inside, derive `cartesianSeries`, `pieSlices`, `resultColumns` (= `columns`), `styleColumns` using the SAME logic relocated from `ChartExplore` (compute from `columns`/`rows`/`params`). Render the same five section blocks (cartesian/pie/table-columns/format/conditional-formatting) gated by `vizType`, reading their config from `params` (`params.cartesian ?? {}`, `params.pie ?? {}`, `params.tableState`, `params.columnSettings ?? {}`, `params.conditionalFormatting ?? []`) and, on each section's `onChange`, calling the prop `onChange` with a new `ChartParams` that replaces only that field (e.g. `onChange({ ...params, cartesian: { ...params.cartesian, ...patch } })`). Move `SectionLabel` (or a copy) into this file.

- [ ] **Step 5: Refactor `ChartExplore.tsx`** to render `<ChartSettingsSections vizType={viz} params={assembledParams} columns={resultColumns} rows={results.data?.rows} onChange={applyParamsBack} />` in place of the extracted Display-tab blocks, where `assembledParams` is the ChartParams it already assembles for save, and `applyParamsBack(next)` distributes `next.cartesian`/`next.pie`/`next.tableState`/`next.columnSettings`/`next.conditionalFormatting` back into the existing `setCartesian`/`setPie`/`setTableState`/`setColumnSettings`/`setConditionalFormatting` setters. Keep ALL other ChartExplore behaviour (field zones, query building, viz picker, save) untouched.

- [ ] **Step 6: Run the guard** — the existing ChartExplore + settings tests MUST stay green:
`npx vitest run src/routes/explore/__tests__ src/components/chart/settings/__tests__ src/components/chart/__tests__`
plus the new `ChartSettingsSections.test.tsx`. If any ChartExplore test fails, the refactor changed behaviour — fix until identical.

- [ ] **Step 7: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/settings/ChartSettingsSections.tsx src/components/chart/settings/__tests__/ChartSettingsSections.test.tsx src/routes/explore/ChartExplore.tsx
git commit -m "refactor(chart): extract ChartSettingsSections; reuse in the chart builder

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

If, after reading `ChartExplore.tsx`, the extraction proves too entangled to keep behaviour identical in one task, STOP and report — do not ship a behaviour change to the chart builder.

---

### Task 9: `applyVizOverride` helper

**Files:**
- Modify: `frontend/src/components/chart/dashboard/params.ts` (append helper)
- Test: `frontend/src/components/chart/dashboard/__tests__/applyVizOverride.test.ts` (create)

- [ ] **Step 1: Write the failing test** `applyVizOverride.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { applyVizOverride } from "../params";
import type { Chart } from "@/api/charts";

const chart = {
  id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "",
  viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [{ col: "x", op: "eq", value: 1 }], orderby: [], row_limit: 100, rollup_groups: [] }, cartesian: { yMetrics: ["rev"], stacked: false } },
} as unknown as Chart;

describe("applyVizOverride", () => {
  it("no override → identity (same viz_type + params)", () => {
    const r = applyVizOverride(chart, undefined);
    expect(r.viz_type).toBe("bar");
    expect(r.params).toEqual(chart.params);
  });
  it("merges display fields and can override viz_type, never touching spec", () => {
    const r = applyVizOverride(chart, { viz_type: "line", cartesian: { stacked: true } } as never);
    expect(r.viz_type).toBe("line");
    expect(r.params.cartesian).toEqual({ yMetrics: ["rev"], stacked: true }); // deep-merged
    expect(r.params.spec).toEqual(chart.params.spec); // query untouched
  });
  it("ignores a spec field in the override", () => {
    const r = applyVizOverride(chart, { spec: { metrics: [] } } as never);
    expect(r.params.spec).toEqual(chart.params.spec);
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/dashboard/__tests__/applyVizOverride.test.ts` — FAIL.

- [ ] **Step 3: Implement.** Append to `params.ts`:

```ts
import type { Chart, ChartParams, VizType } from "@/api/charts";

/** Merge a display-only override over a chart's params (never the query `spec`); optionally override viz_type. */
export function applyVizOverride(
  chart: Chart,
  override?: Record<string, unknown>,
): { viz_type: VizType; params: ChartParams } {
  if (!override) return { viz_type: chart.viz_type, params: chart.params };
  const { viz_type: ovViz, spec: _ignoredSpec, ...display } = override as { viz_type?: VizType; spec?: unknown };
  const params: ChartParams = { ...chart.params };
  for (const [k, v] of Object.entries(display)) {
    const base = (chart.params as Record<string, unknown>)[k];
    // deep-merge plain objects (config sub-objects like cartesian/pie); replace arrays/primitives
    (params as Record<string, unknown>)[k] =
      base && typeof base === "object" && !Array.isArray(base) && v && typeof v === "object" && !Array.isArray(v)
        ? { ...(base as object), ...(v as object) }
        : v;
  }
  params.spec = chart.params.spec; // query is always the chart's spec (+ dashboard injection upstream)
  return { viz_type: ovViz ?? chart.viz_type, params };
}
```
(Adjust the imports at the top of `params.ts` to include `Chart`, `ChartParams`, `VizType` from `@/api/charts` if not already imported.)

- [ ] **Step 4: Run** the test — PASS.

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/params.ts src/components/chart/dashboard/__tests__/applyVizOverride.test.ts
git commit -m "feat(dashboards): applyVizOverride display-only merge helper

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: `DashboardCardView` applies the override

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.vizoverride.test.tsx` (create)

- [ ] **Step 1: Write the failing test** `DashboardCardView.vizoverride.test.tsx`. Mock `ChartRenderer` to capture the `vizType`/`params` it receives:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";

const seen: { vizType?: string; params?: unknown } = {};
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: (props: { vizType: string; params: unknown }) => { seen.vizType = props.vizType; seen.params = props.params; return null; },
}));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] }, cartesian: { yMetrics: ["rev"], stacked: false } } } as never;

// minimal card with a vizOverride
const card = { id: "k", card_type: "chart", chart_id: "c", options: { vizOverride: { viz_type: "line", cartesian: { stacked: true } } } } as never;

describe("DashboardCardView vizOverride", () => {
  it("renders ChartRenderer with the overridden viz_type + merged display, spec intact", () => {
    // NOTE: DashboardCardView needs its data hook to return columns/rows; mock as the file requires (read it first).
    render(<DashboardCardView ws="eng" card={card} chart={chart} refreshNonce={0} parameters={[]} paramValues={{}} />);
    expect(seen.vizType).toBe("line");
    expect((seen.params as { cartesian: { stacked: boolean } }).cartesian.stacked).toBe(true);
    expect((seen.params as { spec: unknown }).spec).toEqual(chart.params.spec);
  });
});
```
(Read `DashboardCardView.tsx` first — it fetches data via a hook; mock that hook to return `{ columns, rows, isLoading:false, error:null }` so `ChartRenderer` is reached. Adapt the test's mocks to the file's actual data hook.)

- [ ] **Step 2: Run** — FAIL (override not applied; vizType is "bar").

- [ ] **Step 3: Implement.** In `DashboardCardView.tsx`, import `applyVizOverride` from `./params`, and at the chart-render branch replace:
```tsx
<ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />
```
with:
```tsx
{(() => {
  const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
  return <ChartRenderer vizType={eff.viz_type} params={eff.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />;
})()}
```
The dashboard already passes the param-injected `chart` (via `applyParamsToChart` upstream), so `eff.params.spec` carries the injected filters and the override only changes display. Confirm the upstream call order in `DashboardView` is unchanged.

- [ ] **Step 4: Run** the test — PASS, plus the existing `DashboardCardView.test.tsx`.

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/DashboardCardView.tsx src/components/chart/dashboard/__tests__/DashboardCardView.vizoverride.test.tsx
git commit -m "feat(dashboards): DashboardCardView applies per-card vizOverride

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Dashboard viz-settings panel

**Files:**
- Create: `frontend/src/components/chart/dashboard/CardVizPanel.tsx`
- Modify: `frontend/src/components/chart/dashboard/EditSidebar.tsx` (new `viz` panel kind)
- Modify: `frontend/src/components/chart/dashboard/CardEditToolbar.tsx` (a "Visualization" action)
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (editPanel `viz`, wiring, data for the panel)
- Test: `frontend/src/components/chart/dashboard/__tests__/CardVizPanel.test.tsx` (create)

- [ ] **Step 1: Write the failing test** `CardVizPanel.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardVizPanel } from "../CardVizPanel";
import type { Chart } from "@/api/charts";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] }, cartesian: { yMetrics: ["rev"] } } } as unknown as Chart;

describe("CardVizPanel", () => {
  it("Reset clears the override", () => {
    const onChange = vi.fn();
    render(<CardVizPanel chart={chart} override={{ cartesian: { stacked: true } }} columns={[{ name: "rev", type: "DOUBLE" }] as never} rows={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "reset visualization" }));
    expect(onChange).toHaveBeenCalledWith(undefined);
  });
});
```

- [ ] **Step 2: Run** — FAIL (module missing).

- [ ] **Step 3: Create `CardVizPanel.tsx`.** It renders `ChartSettingsSections` bound to the EFFECTIVE params (chart.params ⊕ override) and writes the full edited display params back as the override; plus a Reset button:

```tsx
import { applyVizOverride } from "./params";
import { ChartSettingsSections } from "@/components/chart/settings/ChartSettingsSections";
import type { Chart, ChartParams } from "@/api/charts";

interface Props {
  chart: Chart;
  override: Record<string, unknown> | undefined;
  columns: { name: string; type: string }[];
  rows?: unknown[][];
  onChange: (next: Record<string, unknown> | undefined) => void;
}

export function CardVizPanel({ chart, override, columns, rows, onChange }: Props) {
  const eff = applyVizOverride(chart, override);
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">Visualization</div>
        <button type="button" aria-label="reset visualization" className="text-xs text-muted-foreground hover:text-foreground"
          onClick={() => onChange(undefined)}>Reset to chart defaults</button>
      </div>
      <ChartSettingsSections
        vizType={eff.viz_type}
        params={eff.params}
        columns={columns}
        rows={rows}
        onChange={(next: ChartParams) => {
          // store the edited DISPLAY fields (drop spec) as the override
          const { spec: _s, ...display } = next;
          onChange({ ...(override ?? {}), ...display });
        }}
      />
    </div>
  );
}
```

- [ ] **Step 4: Run** the `CardVizPanel` test — PASS.

- [ ] **Step 5: Wire the panel into the dashboard edit flow.**
1. `CardEditToolbar.tsx`: add a "Visualization" button (only for `cardType === "chart"`), aria-label `card visualization`, calling a new `onViz` prop. Add `onViz` to its `Props`.
2. `EditSidebar.tsx`: extend `EditPanel` union with `| { kind: "viz"; cardId: string }`; add props to pass through the chart + override + columns/rows + an `onChangeVizOverride` handler; render `<CardVizPanel .../>` when `panel.kind === "viz"`.
3. `DashboardView.tsx`: the card toolbar's `onViz` sets `editPanel = { kind: "viz", cardId: c.id }`; supply `EditSidebar` with the chart for that card, its current `card.options.vizOverride`, the card's result `columns`/`rows` (from the card's data — reuse however the card's data is available; if not readily available in `DashboardView`, pass the chart's `spec` columns/dimensions as the `columns` fallback so the sections still render), and `onChangeVizOverride={(next) => updateCardOptions(cardId, { vizOverride: next })}`. (`updateCardOptions` exists.)

- [ ] **Step 6: Run** the dashboard + sidebar tests (no regression): `npx vitest run src/components/chart/dashboard/__tests__ src/routes/explore/__tests__`.

- [ ] **Step 7: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/CardVizPanel.tsx src/components/chart/dashboard/__tests__/CardVizPanel.test.tsx src/components/chart/dashboard/EditSidebar.tsx src/components/chart/dashboard/CardEditToolbar.tsx src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): per-card viz-settings panel with reset

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final sweep** (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` — all green.

- [ ] **Step 2: Run the stack & log in** (backend on :8080 already current; `npm run dev`), `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch "a1".** Use a throwaway dashboard for any edits and delete it after.

- [ ] **Step 3: Live-verify (Playwright MCP, side-by-side Metabase), 0 console errors:**
1. **Date all-options**: add a date filter, switch its operator to all-options, pick Relative (Past 7 days) then Specific (Between) → a date-bearing card narrows.
2. **Value sources**: set a filter's values to a Custom list → the dropdown shows exactly those; set another to From a card column → the dropdown shows that column's distinct values.
3. **Per-card viz override**: open a chart card's Visualization panel, change a display setting (e.g. series colour / stacked / axis) → the card re-renders with the override; **Reset to chart defaults** restores it. Confirm the underlying saved chart is unchanged (open it in the chart builder).
4. Confirm Tier 1/2d/3a behaviour (filter pills, in-card mapping, bookmark/trash/info) still works.

- [ ] **Step 4: Clean up & open PR**
```bash
git push -u origin feat/dashboards-tier3b
```
Open PR `feat(dashboards): Tier 3b — date picker, value sources, per-card viz override`; summarise the three features; note frontend-only (no migration) and the deferred items (Exclude date tab, does-not-contain, query override). End with the `🤖 Generated with [Claude Code]` footer. Squash-merge when CI is green.

---

## Self-Review

**Spec coverage:**
- A date all-options → Tasks 1 (operator), 2 (paramToFilters/summarize), 3 (DateFilterPicker), 4 (FilterPill wiring). ✓
- B value sources → Tasks 5 (type), 6 (FilterSettingsPanel control + EditSidebar prop), 7 (resolver + loader + cardColumns). ✓
- C viz override → Tasks 8 (ChartSettingsSections extraction), 9 (applyVizOverride), 10 (DashboardCardView merge), 11 (CardVizPanel + toolbar + editPanel). ✓
- Error handling: stale override inert (Task 9/10 merge ignores unknown fields per viz; spec stripped), missing card/column → resolver null → loader skips (Task 7), partial all-options → bounds-only (Task 2), extraction guarded by ChartExplore tests (Task 8). ✓
- Testing per task + live (Task 12). ✓

**Type/signature consistency:**
- `ParamValuesSource` (Task 5) used by `FilterSettingsPanel` (6), `resolveValuesSource` (7). `ResolvedSource` kinds `static|query|null` consistent between Task 7 helper and loader usage.
- `applyVizOverride(chart, override?) → { viz_type, params }` (Task 9) used identically in `DashboardCardView` (10) and `CardVizPanel` (11).
- `ChartSettingsSections({ vizType, params, columns, rows?, onChange })` (Task 8) consumed identically by `ChartExplore` (8) and `CardVizPanel` (11).
- `DateAllOptionsValue` shape (Task 3) matches the value `paramToFilters`/`summarize` parse (Task 2) and `FilterPill` passes through (Task 4).
- `cardColumns: { cardId, label, columns }[]` consistent between `FilterSettingsPanel` (6), `EditSidebar` (6), and `DashboardView` supply (7).

**Placeholder scan:** No TBD/TODO. Tasks 8 and 11 are refactor/integration tasks specified by interface + exact extraction targets + the regression guard (ChartExplore tests), with an explicit STOP-and-report clause if the extraction can't preserve behaviour — appropriate for a refactor rather than invented verbatim code.
