# Tier 2d — Filter UX Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring egret's dashboard filters to Metabase's UX — in-card column mapping, value widgets, default values, linked (chained) filters, and new boolean / location / month-year / quarter-year parameter types.

**Architecture:** Frontend-only. Parameters already live in `dashboards.parameters` (jsonb), so no backend/migration change. We extend the parameter type model (`api/dashboards.ts`), the value→`ChartFilter` translation (`params.ts`), the option-query hook (`useColumnValues.ts`), the in-view value control (`FilterPill.tsx`), the filter settings sidebar (`FilterSettingsPanel.tsx`), and the dashboard edit flow (`DashboardView.tsx` + a new `CardFilterMapOverlay`), and a pure `computeLinkedFilters` helper for chained option queries.

**Tech Stack:** React 18 + TypeScript + Vite, Vitest + @testing-library/react, TanStack Query. Canonical spec: `docs/en/specs/2026-06-22-tier2d-filter-ux-design.md`.

**Reused contracts (do not re-invent):**
- `ChartFilter = { col, op, value? }` with ops `eq | ne | gt | ge | lt | le | like | in | not_in | is_null | is_not_null`. **There is no `not_like`** — that is why text "does-not-contain" is deferred.
- `paramToFilters(param, value, column, now?)`, `summarize(param, value)`, `isBlank(v)`, `slugify` in `src/components/chart/dashboard/params.ts`.
- `OPERATORS: Record<ParamType, ParamOperator[]>` in `src/components/chart/dashboard/ParameterEditor.tsx`.
- `useDashboardParams` already applies `param.default` on load — **no change** to default application.

**Conventions for every task:** TDD (red → green). Before each commit run the full sweep from `frontend/`:
`npm run typecheck && npm run lint && npm run build && npx vitest run`.
Stage explicit paths only. Use the configured git identity. End every commit message with:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
Branch: `feat/dashboards-tier2d-filter-ux` (already created off main). All `npm`/`npx` commands run from `frontend/`.

**Deferred (do not implement):** text does-not-contain (needs backend `not_like`), date all-options picker, per-card viz-settings override, link-to-dashboard/question.

---

### Task 1: Data model — new types, operators, widget, linked-filter ids

**Files:**
- Modify: `frontend/src/api/dashboards.ts:24-37`
- Modify: `frontend/src/components/chart/dashboard/ParameterEditor.tsx:3-9`
- Test: `frontend/src/components/chart/dashboard/__tests__/operators.test.ts` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/operators.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { OPERATORS } from "../ParameterEditor";

describe("OPERATORS", () => {
  it("has boolean and location types", () => {
    expect(OPERATORS.boolean).toEqual(["is"]);
    expect(OPERATORS.location).toEqual(["is", "is-not"]);
  });
  it("adds month-year and quarter-year to date, keeps does-not-contain absent", () => {
    expect(OPERATORS.date).toContain("month-year");
    expect(OPERATORS.date).toContain("quarter-year");
    expect(OPERATORS.text).not.toContain("does-not-contain");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/operators.test.ts`
Expected: FAIL — `OPERATORS.boolean` is `undefined`.

- [ ] **Step 3: Extend the data model in `api/dashboards.ts`**

Replace lines 24-37 (the `ParamType` / `ParamOperator` / `DashboardParameter` block) with:

```ts
export type ParamType = "text" | "number" | "date" | "id" | "category" | "boolean" | "location";
export type ParamOperator =
  | "is" | "is-not" | "contains" | "starts-with" | "ends-with"        // text / category / location
  | "=" | "!=" | "between" | ">=" | "<="                              // number
  | "range" | "relative" | "single" | "month-year" | "quarter-year";  // date
export type ParamWidget = "dropdown" | "search" | "input";
export interface DashboardParameter {
  id: string;
  name: string;
  slug: string;
  type: ParamType;
  operator: ParamOperator;
  default?: unknown;
  required?: boolean;
  widget?: ParamWidget;            // how the value is picked in view mode
  filteringParameters?: string[];  // ids of params whose value constrains THIS param's option list
}
```

- [ ] **Step 4: Extend `OPERATORS` in `ParameterEditor.tsx`**

Replace lines 3-9 (the `OPERATORS` const) with:

```ts
export const OPERATORS: Record<ParamType, ParamOperator[]> = {
  text: ["is", "is-not", "contains", "starts-with", "ends-with"],
  number: ["=", "!=", "between", ">=", "<="],
  date: ["range", "relative", "single", "month-year", "quarter-year"],
  id: ["is"],
  category: ["is"],
  boolean: ["is"],
  location: ["is", "is-not"],
};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/operators.test.ts`
Expected: PASS.

- [ ] **Step 6: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/dashboards.ts src/components/chart/dashboard/ParameterEditor.tsx src/components/chart/dashboard/__tests__/operators.test.ts
git commit -m "feat(dashboards): add boolean/location types, date month/quarter-year, widget + linked-filter fields

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `paramToFilters` + `summarize` for the new types

**Files:**
- Modify: `frontend/src/components/chart/dashboard/params.ts:20-129`
- Test: `frontend/src/components/chart/dashboard/__tests__/params.tier2d.test.ts` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/params.tier2d.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { paramToFilters, summarize } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "is", ...over });

describe("paramToFilters — tier2d types", () => {
  it("boolean 'true' → eq true; 'false' → eq false; blank → []", () => {
    const param = p({ type: "boolean", operator: "is" });
    expect(paramToFilters(param, "true", "active")).toEqual([{ col: "active", op: "eq", value: true }]);
    expect(paramToFilters(param, "false", "active")).toEqual([{ col: "active", op: "eq", value: false }]);
    expect(paramToFilters(param, "", "active")).toEqual([]);
    expect(paramToFilters(param, false, "active")).toEqual([{ col: "active", op: "eq", value: false }]);
  });
  it("location is → in array; is-not → not_in array; blank → []", () => {
    expect(paramToFilters(p({ type: "location", operator: "is" }), ["US", "CA"], "country"))
      .toEqual([{ col: "country", op: "in", value: ["US", "CA"] }]);
    expect(paramToFilters(p({ type: "location", operator: "is-not" }), ["US"], "country"))
      .toEqual([{ col: "country", op: "not_in", value: ["US"] }]);
    expect(paramToFilters(p({ type: "location", operator: "is" }), [], "country")).toEqual([]);
  });
  it("month-year 'YYYY-MM' → ge first-of-month + le last-of-month", () => {
    expect(paramToFilters(p({ type: "date", operator: "month-year" }), "2024-03", "d"))
      .toEqual([{ col: "d", op: "ge", value: "2024-03-01" }, { col: "d", op: "le", value: "2024-03-31" }]);
    expect(paramToFilters(p({ type: "date", operator: "month-year" }), "", "d")).toEqual([]);
  });
  it("quarter-year 'YYYY-Q' → ge quarter-start + le quarter-end", () => {
    expect(paramToFilters(p({ type: "date", operator: "quarter-year" }), "2024-2", "d"))
      .toEqual([{ col: "d", op: "ge", value: "2024-04-01" }, { col: "d", op: "le", value: "2024-06-30" }]);
    expect(paramToFilters(p({ type: "date", operator: "quarter-year" }), "2024-1", "d"))
      .toEqual([{ col: "d", op: "ge", value: "2024-01-01" }, { col: "d", op: "le", value: "2024-03-31" }]);
    expect(paramToFilters(p({ type: "date", operator: "quarter-year" }), "2024", "d")).toEqual([]);
  });
});

describe("summarize — tier2d types", () => {
  it("boolean → True/False", () => {
    expect(summarize(p({ type: "boolean", operator: "is" }), "true")).toBe("True");
    expect(summarize(p({ type: "boolean", operator: "is" }), false)).toBe("False");
    expect(summarize(p({ type: "boolean", operator: "is" }), "")).toBe("");
  });
  it("quarter-year → 'Q2 2024'", () => {
    expect(summarize(p({ type: "date", operator: "quarter-year" }), "2024-2")).toBe("Q2 2024");
  });
  it("month-year → the value string", () => {
    expect(summarize(p({ type: "date", operator: "month-year" }), "2024-03")).toBe("2024-03");
  });
  it("location multi → 'N selections'", () => {
    expect(summarize(p({ type: "location", operator: "is" }), ["US", "CA"])).toBe("2 selections");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/params.tier2d.test.ts`
Expected: FAIL — boolean/location/month-year cases fall through to `default: return []` and summarize returns the raw value.

- [ ] **Step 3: Add the date sub-operators inside the `date` case**

In `params.ts`, inside `case "date": {` (currently lines 94-112), insert these two blocks **before** the `// relative` fallback (i.e. right after the `range` block, before `const days = ...`):

```ts
      if (param.operator === "month-year") {
        if (isBlank(value)) return [];
        const [y, m] = String(value).split("-").map(Number);
        if (!y || !m) return [];
        const last = new Date(Date.UTC(y, m, 0)).getUTCDate();
        const mm = String(m).padStart(2, "0");
        return [mkf("ge", `${y}-${mm}-01`), mkf("le", `${y}-${mm}-${String(last).padStart(2, "0")}`)];
      }
      if (param.operator === "quarter-year") {
        if (isBlank(value)) return [];
        const [y, q] = String(value).split("-").map(Number);
        if (!y || !q || q < 1 || q > 4) return [];
        const startM = (q - 1) * 3 + 1;
        const endM = q * 3;
        const lastDay = new Date(Date.UTC(y, endM, 0)).getUTCDate();
        return [
          mkf("ge", `${y}-${String(startM).padStart(2, "0")}-01`),
          mkf("le", `${y}-${String(endM).padStart(2, "0")}-${String(lastDay).padStart(2, "0")}`),
        ];
      }
```

- [ ] **Step 4: Add the `boolean` and `location` cases**

In `params.ts`, add these two cases inside the `switch (param.type)` block, **before** `default: return [];` (after the `category` case, ~line 124):

```ts
    case "boolean": {
      if (isBlank(value)) return [];
      const b = value === true || value === "true";
      return [mkf("eq", b)];
    }

    case "location": {
      const arr = Array.isArray(value)
        ? value.filter((v) => !isBlank(v))
        : isBlank(value)
          ? []
          : [value];
      if (!arr.length) return [];
      return [mkf(param.operator === "is-not" ? "not_in" : "in", arr)];
    }
```

- [ ] **Step 5: Extend `summarize`**

In `summarize` (lines 20-35), add these branches **before** the existing `if (param.type === "date" && param.operator === "relative")` line, and after the `if (Array.isArray(value)) { ... }` block:

```ts
  if (param.type === "boolean") {
    return value === true || value === "true" ? "True" : "False";
  }
  if (param.type === "date" && param.operator === "quarter-year") {
    const [y, q] = String(value).split("-");
    return q ? `Q${q} ${y}` : String(value);
  }
```

(The existing `Array.isArray` branch already returns `"N selections"` for location multi-select, and the trailing `return String(value)` already handles month-year.)

- [ ] **Step 6: Run test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/params.tier2d.test.ts`
Expected: PASS. Also run the existing `params` tests to confirm no regression:
`npx vitest run src/components/chart/dashboard/__tests__` → all green.

- [ ] **Step 7: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/params.ts src/components/chart/dashboard/__tests__/params.tier2d.test.ts
git commit -m "feat(dashboards): translate boolean/location/month-year/quarter-year params to filters

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `useColumnValues` accepts an optional `filters` argument

**Files:**
- Modify: `frontend/src/api/useColumnValues.ts:1-43`
- Test: `frontend/src/api/__tests__/useColumnValues.test.ts` (create)

- [ ] **Step 1: Write the failing test**

The hook builds the option query via `useBuildChartData(...).mutateAsync({ dataset_id, query_spec })`. We assert `query_spec.filters` carries the passed-in filters. Create `frontend/src/api/__tests__/useColumnValues.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

const buildSpy = vi.fn().mockResolvedValue({ sql: "SELECT 1", database_id: "db1" });
const submitSpy = vi.fn().mockResolvedValue({ id: "q1" });

vi.mock("@/api/charts", () => ({
  useBuildChartData: () => ({ mutateAsync: buildSpy }),
}));
vi.mock("@/api/sqllab", () => ({
  useSubmitQuery: () => ({ mutateAsync: submitSpy }),
  useQueryStatus: () => ({ data: { status: "running" } }),
  useResults: () => ({ data: undefined }),
}));

import { useColumnValues } from "../useColumnValues";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, children);

describe("useColumnValues", () => {
  beforeEach(() => { buildSpy.mockClear(); submitSpy.mockClear(); });

  it("passes the filters arg into the option query spec", async () => {
    const filters = [{ col: "country", op: "in" as const, value: ["US"] }];
    renderHook(() => useColumnValues("ws", "ds1", "city", true, filters), { wrapper });
    await vi.waitFor(() => expect(buildSpy).toHaveBeenCalled());
    expect(buildSpy.mock.calls[0][0].query_spec.filters).toEqual(filters);
    expect(buildSpy.mock.calls[0][0].query_spec.columns).toEqual(["city"]);
  });

  it("defaults filters to [] when omitted", async () => {
    renderHook(() => useColumnValues("ws", "ds1", "city", true), { wrapper });
    await vi.waitFor(() => expect(buildSpy).toHaveBeenCalled());
    expect(buildSpy.mock.calls[0][0].query_spec.filters).toEqual([]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/api/__tests__/useColumnValues.test.ts`
Expected: FAIL — `useColumnValues` has only 4 params; the spec's `filters` is always `[]` so the first test's `toEqual(filters)` fails.

- [ ] **Step 3: Add the `filters` parameter**

In `useColumnValues.ts`:

1. Add the import for `ChartFilter` to the existing charts import (line 2-3):

```ts
import { useBuildChartData } from "@/api/charts";
import type { QuerySpec, ChartFilter } from "@/api/charts";
```

2. Change the signature (line 9) to accept `filters`:

```ts
export function useColumnValues(ws: string, datasetId: string | null, column: string | null, enabled: boolean, filters: ChartFilter[] = []): ColumnValues {
```

3. Fold `filters` into the effect key so it refetches when the chain changes (line 18):

```ts
  const key = enabled && datasetId && column ? `${datasetId}:${column}:${JSON.stringify(filters)}` : "";
```

4. Use `filters` in the spec (line 28):

```ts
        const spec: QuerySpec = { metrics: [], dimensions: [], columns: [column], filters, orderby: [], row_limit: 1000, rollup_groups: [] };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/api/__tests__/useColumnValues.test.ts`
Expected: PASS.

- [ ] **Step 5: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/useColumnValues.ts src/api/__tests__/useColumnValues.test.ts
git commit -m "feat(dashboards): useColumnValues accepts filters for chained option queries

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `FilterPill` — honour `widget` and render new-type value controls

**Files:**
- Modify: `frontend/src/components/chart/dashboard/FilterPill.tsx:1-142`
- Test: `frontend/src/components/chart/dashboard/__tests__/FilterPill.tier2d.test.tsx` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/FilterPill.tier2d.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "is", ...over });

const openPill = (param: DashboardParameter, value: unknown, opts: string[], onChange = vi.fn()) => {
  render(<FilterPill param={param} value={value} options={opts} onChange={onChange} />);
  fireEvent.click(screen.getByRole("button", { name: param.name }));
  return onChange;
};

describe("FilterPill tier2d", () => {
  it("boolean renders a 3-state Any/True/False select and encodes the choice", () => {
    const onChange = openPill(p({ type: "boolean", operator: "is" }), "", []);
    const sel = screen.getByLabelText("F value") as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(["", "true", "false"]);
    fireEvent.change(sel, { target: { value: "true" } });
    expect(onChange).toHaveBeenCalledWith("true");
  });

  it("widget=input renders a text input for a category and writes a scalar string", () => {
    const onChange = openPill(p({ type: "category", operator: "is", widget: "input" }), "", ["a", "b"]);
    const input = screen.getByLabelText("F value") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "hello" } });
    expect(onChange).toHaveBeenCalledWith("hello");
  });

  it("location renders a multi-checkbox list", () => {
    const onChange = openPill(p({ type: "location", operator: "is" }), [], ["US", "CA"]);
    fireEvent.click(screen.getByLabelText("option US").querySelector("button,[role=checkbox]") ?? screen.getByLabelText("option US"));
    expect(onChange).toHaveBeenCalledWith(["US"]);
  });

  it("month-year renders an <input type=month>", () => {
    const onChange = openPill(p({ type: "date", operator: "month-year" }), "", []);
    const input = screen.getByLabelText("F value") as HTMLInputElement;
    expect(input.type).toBe("month");
    fireEvent.change(input, { target: { value: "2024-03" } });
    expect(onChange).toHaveBeenCalledWith("2024-03");
  });

  it("quarter-year renders a year input + a quarter select and encodes 'YYYY-Q'", () => {
    const onChange = openPill(p({ type: "date", operator: "quarter-year" }), "", []);
    fireEvent.change(screen.getByLabelText("F year"), { target: { value: "2024" } });
    expect(onChange).toHaveBeenCalledWith("2024-1");
    fireEvent.change(screen.getByLabelText("F quarter"), { target: { value: "3" } });
    expect(onChange).toHaveBeenCalledWith("2024-3");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.tier2d.test.tsx`
Expected: FAIL — boolean/location/month-year/quarter-year and the `widget=input` override do not exist yet.

- [ ] **Step 3: Update icons and cleared-value helpers**

In `FilterPill.tsx`, extend the lucide import (line 2) and the two helpers:

```ts
import { Calendar, Hash, MapPin, Tag, ToggleLeft, Type, X } from "lucide-react";
```

Replace `iconFor` (lines 14-19):

```ts
function iconFor(p: DashboardParameter) {
  if (p.type === "date") return Calendar;
  if (p.type === "number" || p.type === "id") return Hash;
  if (p.type === "category") return Tag;
  if (p.type === "location") return MapPin;
  if (p.type === "boolean") return ToggleLeft;
  return Type;
}
```

Replace `clearedValue` (lines 22-25) so location clears to `[]`:

```ts
function clearedValue(p: DashboardParameter): unknown {
  return p.type === "category" || p.type === "location" ||
    (p.type === "number" && p.operator === "between") ||
    (p.type === "date" && p.operator === "range") ? [] : "";
}
```

- [ ] **Step 4: Rewrite `FilterControl` to honour `widget` and new types**

Replace the whole `FilterControl` function (lines 67-142) with:

```tsx
function FilterControl({ param, value, options, onChange }: Props) {
  const [search, setSearch] = useState("");
  const input = "h-8 w-full rounded border bg-background px-2 text-sm";
  const widget = param.widget ?? "dropdown";
  const listType = param.type === "category" || param.type === "location";

  // boolean: 3-state Any / True / False
  if (param.type === "boolean") {
    return (
      <select aria-label={`${param.name} value`} className={input}
        value={value === true || value === "true" ? "true" : value === false || value === "false" ? "false" : ""}
        onChange={(e) => onChange(e.target.value)}>
        <option value="">Any</option>
        <option value="true">True</option>
        <option value="false">False</option>
      </select>
    );
  }

  // month-year
  if (param.type === "date" && param.operator === "month-year") {
    return <input aria-label={`${param.name} value`} type="month" className={input}
      value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  // quarter-year: year input + quarter select, encoded as "YYYY-Q"
  if (param.type === "date" && param.operator === "quarter-year") {
    const [yr = "", q = ""] = String(value ?? "").split("-");
    const emit = (y: string, quarter: string) => onChange(y ? `${y}-${quarter || "1"}` : "");
    return (
      <div className="flex items-center gap-1">
        <input aria-label={`${param.name} year`} type="number" placeholder="Year" className={input}
          value={yr} onChange={(e) => emit(e.target.value, q)} />
        <select aria-label={`${param.name} quarter`} className={input} value={q || "1"}
          onChange={(e) => emit(yr || String(new Date().getUTCFullYear()), e.target.value)}>
          <option value="1">Q1</option><option value="2">Q2</option><option value="3">Q3</option><option value="4">Q4</option>
        </select>
      </div>
    );
  }

  // widget=input on a list-ish type → single free-text value
  if (widget === "input" && (listType || param.type === "id" || param.type === "text")) {
    return <Input aria-label={`${param.name} value`} className={input}
      value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  // dropdown / search list for category, location, and id-with-options
  if (listType || (param.type === "id" && options.length > 0)) {
    const multi = listType;
    const selected = Array.isArray(value) ? (value as string[]) : isBlank(value) ? [] : [String(value)];
    const shown = options.filter((o) => o.toLowerCase().includes(search.toLowerCase()));
    const toggle = (o: string) => {
      if (!multi) { onChange(o); return; }
      onChange(selected.includes(o) ? selected.filter((x) => x !== o) : [...selected, o]);
    };
    return (
      <div className="flex flex-col gap-1">
        <Input aria-label={`search ${param.name}`} className={input} placeholder="Search…" value={search} onChange={(e) => setSearch(e.target.value)} />
        <div className="max-h-52 overflow-auto">
          {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">No values</p>
            : shown.map((o) => (
              <label key={o} aria-label={`option ${o}`} className="flex cursor-pointer items-center gap-2 rounded px-1 py-1 text-sm hover:bg-accent/40"
                onClick={() => { if (!multi) toggle(o); }}>
                {multi
                  ? <Checkbox checked={selected.includes(o)} onCheckedChange={() => toggle(o)} />
                  : <span className={`h-3 w-3 rounded-full border ${selected.includes(o) ? "bg-primary" : ""}`} />}
                <span>{o}</span>
              </label>
            ))}
        </div>
        <button type="button" className="self-start text-xs text-muted-foreground hover:text-foreground" onClick={() => onChange(multi ? [] : "")}>Clear</button>
      </div>
    );
  }

  if (param.type === "id") {
    return <Input aria-label={`${param.name} value`} className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  if (param.type === "number" && param.operator === "between") {
    const [min, max] = Array.isArray(value) ? (value as (number | null)[]) : [null, null];
    const num = (s: string) => (s === "" ? null : Number(s));
    return (
      <div className="flex items-center gap-1">
        <input aria-label={`${param.name} min`} type="number" className={input} value={min ?? ""} onChange={(e) => onChange([num(e.target.value), max ?? null])} />
        <span className="text-muted-foreground">–</span>
        <input aria-label={`${param.name} max`} type="number" className={input} value={max ?? ""} onChange={(e) => onChange([min ?? null, num(e.target.value)])} />
      </div>
    );
  }
  if (param.type === "number") {
    return <input aria-label={`${param.name} value`} type="number" className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value === "" ? "" : Number(e.target.value))} />;
  }

  if (param.type === "date" && param.operator === "range") {
    const [start, end] = Array.isArray(value) ? (value as string[]) : ["", ""];
    return (
      <div className="flex items-center gap-1">
        <input aria-label={`${param.name} min`} type="date" className={input} value={start ?? ""} onChange={(e) => onChange([e.target.value, end ?? ""])} />
        <span className="text-muted-foreground">–</span>
        <input aria-label={`${param.name} max`} type="date" className={input} value={end ?? ""} onChange={(e) => onChange([start ?? "", e.target.value])} />
      </div>
    );
  }
  if (param.type === "date" && param.operator === "relative") {
    return (
      <div className="flex flex-col">
        {RELATIVE.map((o) => (
          <button key={o.v} type="button" aria-label={o.label} className={`rounded px-2 py-1 text-left text-sm hover:bg-accent/40 ${value === o.v ? "font-medium text-primary" : ""}`} onClick={() => onChange(o.v)}>{o.label}</button>
        ))}
      </div>
    );
  }
  if (param.type === "date") {
    return <input aria-label={`${param.name} value`} type="date" className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  return <Input aria-label={`${param.name} value`} className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
}
```

Note: the `location` test clicks the option's checkbox; the Radix `Checkbox` renders a `role=checkbox` button. The test query handles both the wrapping label and the inner control, so a label-level click also toggles.

- [ ] **Step 5: Run test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.tier2d.test.tsx`
Expected: PASS. Also run the existing FilterPill tests if present:
`npx vitest run src/components/chart/dashboard/__tests__` → all green.

- [ ] **Step 6: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/FilterPill.tsx src/components/chart/dashboard/__tests__/FilterPill.tier2d.test.tsx
git commit -m "feat(dashboards): FilterPill widget support + boolean/location/month/quarter controls

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `FilterSettingsPanel` — widget radio, default value, linked filters

**Files:**
- Modify: `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx:1-35`
- Modify: `frontend/src/components/chart/dashboard/EditSidebar.tsx:46-48`
- Test: `frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("FilterSettingsPanel tier2d", () => {
  it("type select offers boolean and location", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    const sel = screen.getByLabelText("parameter type") as HTMLSelectElement;
    const opts = Array.from(sel.options).map((o) => o.value);
    expect(opts).toContain("boolean");
    expect(opts).toContain("location");
  });

  it("widget radio writes param.widget", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("widget search"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ widget: "search" }));
  });

  it("default control writes param.default", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("default value"), { target: { value: "West" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ default: "West" }));
  });

  it("linked-filters checkbox writes filteringParameters with other param ids", () => {
    const onChange = vi.fn();
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({})} allParameters={[p({}), other]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("link City"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ filteringParameters: ["2"] }));
  });

  it("does not offer the param itself as a linked filter", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("link Region")).toBeNull();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`
Expected: FAIL — `allParameters` prop, widget radio, default control, and linked section don't exist.

- [ ] **Step 3: Rewrite `FilterSettingsPanel`**

Replace the entire file `FilterSettingsPanel.tsx`:

```tsx
import type { DashboardParameter, ParamType, ParamOperator, ParamWidget } from "@/api/dashboards";
import { OPERATORS } from "./ParameterEditor";

interface Props {
  param: DashboardParameter;
  allParameters: DashboardParameter[];
  onChange: (next: DashboardParameter) => void;
  onDelete: () => void;
}

const WIDGETS: { v: ParamWidget; label: string }[] = [
  { v: "dropdown", label: "Dropdown" },
  { v: "search", label: "Search box" },
  { v: "input", label: "Input box" },
];

export function FilterSettingsPanel({ param, allParameters, onChange, onDelete }: Props) {
  const cls = "h-8 w-full rounded border bg-background px-2 text-sm";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  const showWidget = param.type === "category" || param.type === "id" || param.type === "location" || param.type === "text";
  const others = allParameters.filter((o) => o.id !== param.id);
  const linked = param.filteringParameters ?? [];
  const toggleLink = (id: string) =>
    onChange({ ...param, filteringParameters: linked.includes(id) ? linked.filter((x) => x !== id) : [...linked, id] });

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">Filter settings</div>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Name
        <input aria-label="parameter name" className={cls} value={param.name} onChange={(e) => onChange({ ...param, name: e.target.value })} />
      </label>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Type
        <select aria-label="parameter type" className={cls} value={param.type}
          onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
          {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
        </select>
      </label>
      {showOp && (
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">Operator
          <select aria-label="parameter operator" className={cls} value={param.operator}
            onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
            {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
          </select>
        </label>
      )}

      {showWidget && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">How should people filter on this?</div>
          {WIDGETS.map((w) => (
            <label key={w.v} className="flex items-center gap-2 text-sm">
              <input type="radio" name={`widget-${param.id}`} aria-label={`widget ${w.v}`}
                checked={(param.widget ?? "dropdown") === w.v}
                onChange={() => onChange({ ...param, widget: w.v })} /> {w.label}
            </label>
          ))}
        </div>
      )}

      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Default value
        <input aria-label="default value" className={cls}
          value={Array.isArray(param.default) ? (param.default as unknown[]).join(", ") : String(param.default ?? "")}
          onChange={(e) => onChange({ ...param, default: e.target.value === "" ? undefined : e.target.value })} />
      </label>

      {others.length > 0 && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">Linked filters</div>
          <p className="text-xs text-muted-foreground/70">Limit this filter's choices to values that match the selected filters.</p>
          {others.map((o) => (
            <label key={o.id} className="flex items-center gap-2 text-sm">
              <input type="checkbox" aria-label={`link ${o.name}`} checked={linked.includes(o.id)} onChange={() => toggleLink(o.id)} /> {o.name}
            </label>
          ))}
        </div>
      )}

      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" aria-label="parameter required" checked={!!param.required} onChange={(e) => onChange({ ...param, required: e.target.checked })} /> Required
      </label>
      <button type="button" aria-label="delete filter" className="self-start text-sm text-destructive hover:underline" onClick={onDelete}>Delete filter</button>
    </div>
  );
}
```

- [ ] **Step 4: Pass `allParameters` from `EditSidebar`**

In `EditSidebar.tsx` (lines 46-48), update the FilterSettingsPanel usage:

```tsx
      {panel?.kind === "filter" && props.paramForPanel && (
        <FilterSettingsPanel param={props.paramForPanel} allParameters={props.parameters} onChange={props.onChangeParam} onDelete={props.onDeleteParam} />
      )}
```

- [ ] **Step 5: Fix the existing `EditSidebar.test.tsx` fixture**

`EditSidebar` already passes `props.parameters`, so no test change is required there — but verify by running its test in Step 6. (The default-value control writes `undefined` when cleared, matching `useDashboardParams`, which only applies a `default` when present.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `npx vitest run src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx src/components/chart/dashboard/__tests__/EditSidebar.test.tsx`
Expected: PASS for both.

- [ ] **Step 7: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/FilterSettingsPanel.tsx src/components/chart/dashboard/EditSidebar.tsx src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx
git commit -m "feat(dashboards): filter settings widget radio, default value, linked filters

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: In-card "Column to filter on" overlay + remove per-param mapping from `CardSettingsPanel`

**Files:**
- Create: `frontend/src/components/chart/dashboard/CardFilterMapOverlay.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx` (create)
- Modify: `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx:57-71`
- Modify: `frontend/src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx` (if it asserts mapping — adjust)
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (card render block ~326-349)

- [ ] **Step 1: Write the failing test for the overlay component**

Create `frontend/src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardFilterMapOverlay } from "../CardFilterMapOverlay";

describe("CardFilterMapOverlay", () => {
  it("shows the columns and writes the picked column", () => {
    const onChange = vi.fn();
    render(<CardFilterMapOverlay columns={["country", "city"]} value="" onChange={onChange} />);
    expect(screen.getByText("Column to filter on")).toBeTruthy();
    const sel = screen.getByLabelText("column to filter on") as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(["", "country", "city"]);
    fireEvent.change(sel, { target: { value: "city" } });
    expect(onChange).toHaveBeenCalledWith("city");
  });
  it("reflects the current mapping", () => {
    render(<CardFilterMapOverlay columns={["country"]} value="country" onChange={vi.fn()} />);
    expect((screen.getByLabelText("column to filter on") as HTMLSelectElement).value).toBe("country");
  });
  it("carries dash-nodrag so the grid does not hijack the select", () => {
    const { container } = render(<CardFilterMapOverlay columns={["country"]} value="" onChange={vi.fn()} />);
    expect(container.querySelector(".dash-nodrag")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Create `CardFilterMapOverlay.tsx`**

```tsx
interface Props {
  columns: string[];
  value: string;
  onChange: (col: string) => void;
}

/** Overlay shown on each chart card while a filter is selected, to map the filter to a column. */
export function CardFilterMapOverlay({ columns, value, onChange }: Props) {
  return (
    <div className="dash-nodrag absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-card/85 p-3 text-center backdrop-blur-[1px]">
      <div className="text-xs font-medium text-muted-foreground">Column to filter on</div>
      <select aria-label="column to filter on" className="h-8 w-48 max-w-full rounded border bg-background px-2 text-sm"
        value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Select a column…</option>
        {columns.map((c) => <option key={c} value={c}>{c}</option>)}
      </select>
    </div>
  );
}
```

- [ ] **Step 4: Run the overlay test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx`
Expected: PASS.

- [ ] **Step 5: Remove the per-param mapping section from `CardSettingsPanel`**

In `CardSettingsPanel.tsx`, delete the entire `{parameters.length > 0 && ( ... )}` block (lines 57-71 — the "Filters" mapping section). Leave Title, On-click, crossfilter, and Move-to-tab untouched. The `parameters` prop is still used by the crossfilter section, so keep it in `Props`.

- [ ] **Step 6: Update `CardSettingsPanel.test.tsx` if needed**

Run the existing test first: `npx vitest run src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx`.
If it asserts the presence of a `map <param>` select (aria-label `map Region` etc.), change that assertion to confirm absence:

```tsx
expect(screen.queryByLabelText(/^map /)).toBeNull();
```

If no such assertion exists, no change is needed. (Do NOT remove crossfilter assertions — `crossfilter <param>` labels are a different feature and stay.)

- [ ] **Step 7: Wire the overlay into `DashboardView` card render**

In `DashboardView.tsx`, add an `activeParamId` near the top of the render (next to where `editing` is in scope, before the `return`), e.g. just before the `return (`:

```tsx
  const activeParamId = editing && editPanel?.kind === "filter" ? editPanel.paramId : null;
```

Import the overlay at the top with the other dashboard component imports:

```tsx
import { CardFilterMapOverlay } from "@/components/chart/dashboard/CardFilterMapOverlay";
```

Then inside the `visibleCards.map((c) => { ... })` body, in the inner card `<div className={...overflow-hidden...}>` (around line 341-346), render the overlay for chart cards when a filter is active. Replace the inner content block:

```tsx
              <div className={`relative h-full overflow-hidden ${c.card_type === "heading" ? "" : "rounded-lg border bg-card shadow-sm"} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
                {editing && <div className="dash-draghandle absolute inset-x-0 top-0 z-10 h-6 cursor-move" title="Drag to move" />}
                <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values}
                  blocked={!editing && requiredUnset}
                  onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} />
                {activeParamId && c.card_type === "chart" && baseChart && (
                  <CardFilterMapOverlay
                    columns={[...baseChart.params.spec.dimensions, ...baseChart.params.spec.columns]}
                    value={mappings[activeParamId] ?? ""}
                    onChange={(col) => updateCardOptions(c.id, { paramMappings: { ...mappings, [activeParamId]: col } })}
                  />
                )}
              </div>
```

(`baseChart` and `mappings` are already in scope from lines 327-328.)

- [ ] **Step 8: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/CardFilterMapOverlay.tsx src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx src/components/chart/dashboard/CardSettingsPanel.tsx src/routes/explore/DashboardView.tsx
# include CardSettingsPanel.test.tsx in the add ONLY if you changed it in Step 6
git commit -m "feat(dashboards): in-card column mapping during filter edit; drop per-param mapping from card settings

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: `computeLinkedFilters` helper + wire chained option queries through `ParamOptions`

**Files:**
- Modify: `frontend/src/components/chart/dashboard/params.ts` (add helper at end)
- Test: `frontend/src/components/chart/dashboard/__tests__/computeLinkedFilters.test.ts` (create)
- Modify: `frontend/src/routes/explore/DashboardView.tsx:36-40` (ParamOptions) and `:266-275` (the loader map)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/computeLinkedFilters.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { computeLinkedFilters } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const param = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "City", slug: "city", type: "category", operator: "is", ...over });

describe("computeLinkedFilters", () => {
  const country = param({ id: "c", name: "Country", slug: "country", type: "location", operator: "is" });
  const city = param({ id: "1", filteringParameters: ["c"] });

  it("builds filters from a linked param's value using the same card's mapping", () => {
    const mappings = { c: "country_col", "1": "city_col" };
    const values = { country: ["US"] };
    expect(computeLinkedFilters(city, mappings, [city, country], values))
      .toEqual([{ col: "country_col", op: "in", value: ["US"] }]);
  });

  it("ignores a linked param with no value", () => {
    const mappings = { c: "country_col", "1": "city_col" };
    expect(computeLinkedFilters(city, mappings, [city, country], {})).toEqual([]);
  });

  it("ignores a linked param not mapped on this card", () => {
    const mappings = { "1": "city_col" }; // no mapping for 'c'
    const values = { country: ["US"] };
    expect(computeLinkedFilters(city, mappings, [city, country], values)).toEqual([]);
  });

  it("returns [] when there are no filteringParameters", () => {
    expect(computeLinkedFilters(param({ id: "1" }), {}, [], {})).toEqual([]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/computeLinkedFilters.test.ts`
Expected: FAIL — `computeLinkedFilters` is not exported.

- [ ] **Step 3: Add the helper to `params.ts`**

Append to `params.ts`:

```ts
/**
 * Filters that constrain a parameter's option list to values consistent with its linked
 * parameters' current values. Each linked param must be mapped on the SAME card (so we can read
 * its column from `mappings`); linked params with no value or no mapping are ignored.
 */
export function computeLinkedFilters(
  param: DashboardParameter,
  mappings: Record<string, string>,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  now: Date = new Date(),
): ChartFilter[] {
  const out: ChartFilter[] = [];
  for (const linkedId of param.filteringParameters ?? []) {
    const linked = parameters.find((p) => p.id === linkedId);
    if (!linked) continue;
    const col = mappings[linkedId];
    if (!col) continue;
    const v = values[linked.slug];
    if (isBlank(v)) continue;
    out.push(...paramToFilters(linked, v, col, now));
  }
  return out;
}
```

- [ ] **Step 4: Run the helper test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/computeLinkedFilters.test.ts`
Expected: PASS.

- [ ] **Step 5: Thread `filters` through `ParamOptions`**

In `DashboardView.tsx`, update `ParamOptions` (lines 36-40) to accept and forward `filters`:

```tsx
function ParamOptions({ ws, datasetId, column, slug, filters, onLoaded }: { ws: string; datasetId: string | null; column: string | null; slug: string; filters: ChartFilter[]; onLoaded: (slug: string, values: string[]) => void }) {
  const { values } = useColumnValues(ws, datasetId, column, !!datasetId && !!column, filters);
  useEffect(() => { if (values.length) onLoaded(slug, values); }, [slug, values, onLoaded]);
  return null;
}
```

Ensure `ChartFilter` is imported in `DashboardView.tsx` (add to the existing `@/api/charts` import if not already present):

```tsx
import type { ChartFilter } from "@/api/charts";
```

And import `computeLinkedFilters` from params (extend the existing `./...params` / `@/components/chart/dashboard/params` import that already brings in `applyParamsToChart`):

```tsx
import { applyParamsToChart, computeLinkedFilters } from "@/components/chart/dashboard/params";
```

(If `applyParamsToChart` is imported via a relative path in this file, add `computeLinkedFilters` to that same import statement instead — match the existing path.)

- [ ] **Step 6: Pass linked filters from the loader map**

In `DashboardView.tsx`, update the parameter-options loader (lines 266-275). Now that `location` also needs distinct values, include it in the filter, and compute the chain:

```tsx
      {parameters
        .filter((p) => p.type === "category" || p.type === "id" || p.type === "location")
        .map((p) => {
          const card = cards.find((c) => c.card_type === "chart" && ((c.options.paramMappings as Record<string, string>) ?? {})[p.id]);
          if (!card) return null;
          const ch = chartById.get(card.chart_id ?? "");
          const cardMappings = (card.options.paramMappings as Record<string, string>) ?? {};
          const column = cardMappings[p.id] ?? null;
          if (!ch || !column) return null;
          const filters = computeLinkedFilters(p, cardMappings, parameters, paramsState.values);
          return <ParamOptions key={p.id} ws={ws} datasetId={ch.dataset_id} column={column} slug={p.slug} filters={filters} onLoaded={onOptionsLoaded} />;
        })}
```

- [ ] **Step 7: Full sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/params.ts src/components/chart/dashboard/__tests__/computeLinkedFilters.test.ts src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): chained option queries via computeLinkedFilters + location options

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final full sweep**

Run from `frontend/`: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green, build succeeds.

- [ ] **Step 2: Start the local stack and log in**

Per the seeded-stack memory: bring up docker + `cargo run serve` (backend) + `npm run dev` (frontend), log in as `admin@egret.local` / `Admin12345!`, open the eng workspace orders demo dashboard. **Do NOT touch the "a1" dashboard**; create a throwaway dashboard for testing and delete it afterward.

- [ ] **Step 3: Live-verify the Tier 2d flows (Playwright MCP, side-by-side with Metabase localhost:3001)**

Confirm, with **0 console errors**:
1. Add a filter → while it is selected, each chart card shows the "Column to filter on" overlay → pick columns → mapping persists (reopen the filter shows the same columns selected).
2. Set widget = Dropdown / Search / Input → the view-mode pill renders the matching control (list / search-first list / text input).
3. Add a boolean filter → Any/True/False select filters the card. Add a location filter → multi-select narrows the card. Add a month-year and a quarter-year filter → the card date range narrows.
4. Link two filters (e.g. City linked to Country, both mapped on the same card) → choosing a Country narrows City's dropdown to consistent values.
5. Set a default value → reload the dashboard → the default is applied on load.

- [ ] **Step 4: Clean up test artifacts**

Delete any throwaway dashboards created during verification. Leave "a1" and the seeded demo dashboards untouched.

- [ ] **Step 5: Push and open the PR**

```bash
git push -u origin feat/dashboards-tier2d-filter-ux
```

Open a PR titled `feat(dashboards): Tier 2d — Metabase-style filter UX overhaul` summarizing: in-card column mapping, value widgets, default values, linked filters, and the new boolean/location/month-year/quarter-year types; note the deferred items (does-not-contain, date all-options) and that there is no backend change. End the PR body with the `🤖 Generated with [Claude Code]` footer. Squash-merge once CI is green.

---

## Self-Review

**Spec coverage:**
- (A) in-card column mapping → Task 6 (overlay) + remove from CardSettingsPanel. ✓
- (B) value widget → Task 1 (`ParamWidget`), Task 5 (radio), Task 4 (FilterPill honours it). ✓
- (C) default value → Task 5 (default control); application already in `useDashboardParams` (unchanged, noted). ✓
- (D) linked filters → Task 5 (checkboxes write `filteringParameters`), Task 3 (`useColumnValues` filters arg), Task 7 (`computeLinkedFilters` + wiring). ✓
- (E) new types/operators → Task 1 (types + OPERATORS), Task 2 (paramToFilters + summarize), Task 4 (controls). ✓
- Deferred (does-not-contain, date all-options) → explicitly excluded; reason recorded (no `not_like`). ✓
- Error handling: linked param without same-card mapping ignored (Task 7 test); boolean bind sent as JSON bool with string fallback noted for live verify (Task 8 Step 3); partial month/quarter → blank (Task 2 tests). ✓
- Testing: vitest per task; live verify Task 8. ✓

**Type consistency:** `ParamWidget` used identically in Tasks 1/4/5. `computeLinkedFilters(param, mappings, parameters, values, now?)` signature matches between Task 7 definition and its DashboardView call. `useColumnValues(ws, datasetId, column, enabled, filters?)` matches between Task 3 and Task 7 ParamOptions. `CardFilterMapOverlay({columns, value, onChange})` matches between Task 6 definition and DashboardView usage. `FilterSettingsPanel` gains `allParameters` consistently in Task 5 and EditSidebar.

**Placeholder scan:** No TBD/TODO; every code step shows complete code.
