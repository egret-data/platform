# Tier 1 — Dashboard Metabase-style Restyle + Filter Pills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make egret's dashboard look and feel like Metabase — a muted canvas with soft-shadow cards, a row of filter **pills** with per-type popovers (replacing the raw native `<select>` widgets), brand-coloured tabs, required-param enforcement, and an empty-tab state.

**Architecture:** Frontend-only. A new `FilterPill` component (Radix Popover) replaces the native widgets in `DashboardFilterBar`; the value→filter encodings (`paramToFilters`) and option loading (`useColumnValues`) are unchanged. `DashboardView`/`DashboardCardView` get a class restyle + required-gating; `useChartData` gains an `enabled` gate.

**Tech Stack:** React 18 + TypeScript + Vite + Vitest + @testing-library/react; existing Radix UI primitives (`popover`, `checkbox`, `input`, `button`, `scroll-area`) — no new deps; lucide-react icons.

**Spec:** `docs/en/specs/2026-06-22-tier1-dashboard-metabase-restyle-design.md`

**Branch:** `fix/dashboard-parity-gaps` (already checked out; already has the category-dropdown fix + audit doc).

## Conventions (every task)

- TDD: failing test → run (fail) → implement → run (pass) → commit.
- **Full sweep before each commit (from `frontend/`):** `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Configured git identity (never `--author`); commit body ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Stage explicit paths (never `git add -A`).
- Metabase as the visual standard; clean-room (no AGPL code).

## File structure

- `frontend/src/components/chart/dashboard/params.ts` (Modify) — export `isBlank`, add `summarize`.
- `frontend/src/components/chart/dashboard/FilterPill.tsx` (Create) — the pill + per-type popover.
- `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx` (Modify) — pill row + required notice.
- `frontend/src/components/chart/dashboard/DashboardTabBar.tsx` (Modify) — brand-coloured active tab.
- `frontend/src/api/useChartData.ts` (Modify) — `enabled` gate.
- `frontend/src/components/chart/dashboard/DashboardCardView.tsx` (Modify) — `blocked` → placeholder.
- `frontend/src/routes/explore/DashboardView.tsx` (Modify) — canvas/card restyle, RGL margin, `requiredUnset` gating, empty-tab state.
- Tests under `frontend/src/components/chart/dashboard/__tests__/` and `frontend/src/api/__tests__/`.

---

## Task 1: `summarize` helper + export `isBlank` (`params.ts`)

**Files:**
- Modify: `frontend/src/components/chart/dashboard/params.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/params.summarize.test.ts`

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/dashboard/__tests__/params.summarize.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { summarize, isBlank } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("isBlank", () => {
  it("treats undefined/null/empty-string as blank", () => {
    expect(isBlank(undefined)).toBe(true);
    expect(isBlank(null)).toBe(true);
    expect(isBlank("")).toBe(true);
    expect(isBlank("x")).toBe(false);
    expect(isBlank(0)).toBe(false);
  });
});

describe("summarize", () => {
  it("blank → empty string", () => {
    expect(summarize(p(), undefined)).toBe("");
    expect(summarize(p(), [])).toBe("");
  });
  it("category single vs multi", () => {
    expect(summarize(p(), ["EU"])).toBe("EU");
    expect(summarize(p(), ["EU", "US"])).toBe("2 selections");
  });
  it("number between → a – b", () => {
    expect(summarize(p({ type: "number", operator: "between" }), [1, 5])).toBe("1 – 5");
  });
  it("date relative → label", () => {
    expect(summarize(p({ type: "date", operator: "relative" }), "last7")).toBe("Last 7 days");
  });
  it("scalar → string", () => {
    expect(summarize(p({ type: "text", operator: "is" }), "hello")).toBe("hello");
  });
});
```

- [ ] **Step 2: Run it — fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/params.summarize.test.ts`
Expected: FAIL — `summarize` is not exported (and `isBlank` is currently a private `const`).

- [ ] **Step 3: Implement**

In `params.ts`, change the private `const isBlank = …` to an exported function and add `summarize`. Replace the line `const isBlank = (v: unknown) => v === undefined || v === null || v === "";` with:

```ts
export const isBlank = (v: unknown) => v === undefined || v === null || v === "";

const RELATIVE_LABELS: Record<string, string> = {
  last7: "Last 7 days", last30: "Last 30 days", last90: "Last 90 days", last365: "Last 365 days",
};

/** A short human summary of a parameter's current value for the filter pill. */
export function summarize(param: DashboardParameter, value: unknown): string {
  if (isBlank(value)) return "";
  if (Array.isArray(value)) {
    const vals = value.filter((v) => !isBlank(v));
    if (vals.length === 0) return "";
    if (vals.length === 1) return String(vals[0]);
    if (vals.length === 2 && (param.operator === "between" || param.operator === "range")) {
      return `${vals[0]} – ${vals[1]}`;
    }
    return `${vals.length} selections`;
  }
  if (param.type === "date" && param.operator === "relative") {
    return RELATIVE_LABELS[String(value)] ?? String(value);
  }
  return String(value);
}
```

(`DashboardParameter` is already imported in `params.ts`.)

- [ ] **Step 4: Run it — pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/params.summarize.test.ts`
Expected: PASS (3 describes).

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/params.ts frontend/src/components/chart/dashboard/__tests__/params.summarize.test.ts
git commit -m "feat(dashboards): summarize() filter-value helper + export isBlank

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `FilterPill` component

**Files:**
- Create: `frontend/src/components/chart/dashboard/FilterPill.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/FilterPill.test.tsx`

A pill (Radix Popover) per parameter. The pill wrapper is a `<div>` holding the trigger button (icon + name + value summary) and, when a value is set, a separate clear (×) button — two sibling buttons (valid HTML, not nested). The popover content branches on `param.type`.

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/dashboard/__tests__/FilterPill.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterPill } from "../FilterPill";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("FilterPill", () => {
  it("shows the param name and value summary, and a clear button when set", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p()} value={["EU", "US"]} options={["EU", "US", "APAC"]} onChange={onChange} />);
    expect(screen.getByRole("button", { name: /^Region/ }).textContent).toContain("2 selections");
    fireEvent.click(screen.getByRole("button", { name: "clear Region" }));
    expect(onChange).toHaveBeenCalledWith([]);
  });

  it("category popover: search filters options and checking one calls onChange with an array", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p()} value={[]} options={["EU", "US", "APAC"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Region/ }));
    fireEvent.change(screen.getByLabelText("search Region"), { target: { value: "ap" } });
    expect(screen.queryByLabelText("option EU")).toBeNull();
    fireEvent.click(screen.getByLabelText("option APAC"));
    expect(onChange).toHaveBeenCalledWith(["APAC"]);
  });

  it("id popover: single-select calls onChange with a scalar", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "id", name: "User", slug: "user", operator: "is" })} value="" options={["1", "2"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^User/ }));
    fireEvent.click(screen.getByLabelText("option 2"));
    expect(onChange).toHaveBeenCalledWith("2");
  });

  it("text popover: typing calls onChange with the string", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "text", name: "Name", slug: "name", operator: "contains" })} value="" options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Name/ }));
    fireEvent.change(screen.getByLabelText("Name value"), { target: { value: "ann" } });
    expect(onChange).toHaveBeenCalledWith("ann");
  });

  it("number between: two inputs call onChange with [min, max]", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "number", name: "Price", slug: "price", operator: "between" })} value={[null, null]} options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Price/ }));
    fireEvent.change(screen.getByLabelText("Price min"), { target: { value: "3" } });
    expect(onChange).toHaveBeenCalledWith([3, null]);
  });

  it("date relative: choosing an option calls onChange with the code", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "date", name: "When", slug: "when", operator: "relative" })} value="" options={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^When/ }));
    fireEvent.click(screen.getByLabelText("Last 30 days"));
    expect(onChange).toHaveBeenCalledWith("last30");
  });
});
```

- [ ] **Step 2: Run it — fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.test.tsx`
Expected: FAIL (module not found).

- [ ] **Step 3: Implement `frontend/src/components/chart/dashboard/FilterPill.tsx`**

```tsx
import { useState } from "react";
import { Calendar, Hash, Tag, Type, X } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { isBlank, summarize } from "./params";

const RELATIVE = [
  { v: "last7", label: "Last 7 days" }, { v: "last30", label: "Last 30 days" },
  { v: "last90", label: "Last 90 days" }, { v: "last365", label: "Last 365 days" },
];

function iconFor(p: DashboardParameter) {
  if (p.type === "date") return Calendar;
  if (p.type === "number" || p.type === "id") return Hash;
  if (p.type === "category") return Tag;
  return Type;
}

/** Cleared value for a parameter: [] for multi-value category, "" otherwise. */
function clearedValue(p: DashboardParameter): unknown {
  return p.type === "category" || (p.type === "number" && p.operator === "between") ||
    (p.type === "date" && p.operator === "range") ? [] : "";
}

function hasValue(p: DashboardParameter, value: unknown): boolean {
  if (Array.isArray(value)) return value.filter((v) => !isBlank(v)).length > 0;
  return !isBlank(value);
}

interface Props {
  param: DashboardParameter;
  value: unknown;
  options: string[];
  onChange: (v: unknown) => void;
}

export function FilterPill({ param, value, options, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const Icon = iconFor(param);
  const summary = summarize(param, value);
  const set = hasValue(param, value);
  return (
    <div className={`inline-flex items-center rounded-full border text-sm transition-colors ${set ? "border-primary/40 bg-primary/5" : "bg-background hover:bg-accent/40"}`}>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button type="button" aria-label={param.name} className="flex items-center gap-1.5 rounded-full px-3 py-1.5">
            <Icon className="h-3.5 w-3.5 text-muted-foreground" />
            <span>{param.name}{param.required ? " *" : ""}{set && summary ? <span className="text-muted-foreground">: {summary}</span> : null}</span>
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-64 p-2">
          <FilterControl param={param} value={value} options={options} onChange={onChange} />
        </PopoverContent>
      </Popover>
      {set && (
        <button type="button" aria-label={`clear ${param.name}`} className="pr-2.5 text-muted-foreground hover:text-foreground"
          onClick={() => onChange(clearedValue(param))}>
          <X className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

function FilterControl({ param, value, options, onChange }: Props) {
  const [search, setSearch] = useState("");
  const input = "h-8 w-full rounded border bg-background px-2 text-sm";

  if (param.type === "category" || (param.type === "id" && options.length > 0)) {
    const multi = param.type === "category";
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

  // text (and any fallback)
  return <Input aria-label={`${param.name} value`} className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
}
```

- [ ] **Step 4: Run it — pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/FilterPill.test.tsx`
Expected: PASS (6 tests). If a Radix Popover portal makes content invisible in jsdom, it still renders into the DOM (queryable) — the tests use role/label queries that work with the portal.

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/FilterPill.tsx frontend/src/components/chart/dashboard/__tests__/FilterPill.test.tsx
git commit -m "feat(dashboards): FilterPill — Metabase-style filter pill + per-type popover

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `DashboardFilterBar` → pill row + required notice

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx`

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardFilterBar } from "../DashboardFilterBar";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "p2", name: "User", slug: "user", type: "id", operator: "is", required: true },
];

describe("DashboardFilterBar", () => {
  it("renders one pill per parameter", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.getByRole("button", { name: /^Region/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^User/ })).toBeTruthy();
  });
  it("shows a required notice when a required param is blank", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.getByText(/required filter/i).textContent).toContain("User");
  });
  it("no notice when required params are set", () => {
    render(<DashboardFilterBar parameters={params} values={{ user: "7" }} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.queryByText(/required filter/i)).toBeNull();
  });
});
```

- [ ] **Step 2: Run it — fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx`
Expected: FAIL — the current bar renders native widgets (no required notice; pill buttons are actually present via `aria-label` on selects, but the notice test fails).

- [ ] **Step 3: Replace `DashboardFilterBar.tsx`**

```tsx
import type { DashboardParameter } from "@/api/dashboards";
import { FilterPill } from "./FilterPill";
import { isBlank } from "./params";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  onChange: (slug: string, value: unknown) => void;
  optionsBySlug: Record<string, string[]>;
}

export function DashboardFilterBar({ parameters, values, onChange, optionsBySlug }: Props) {
  if (parameters.length === 0) return null;
  const missingRequired = parameters.filter(
    (p) => p.required && (isBlank(values[p.slug]) || (Array.isArray(values[p.slug]) && (values[p.slug] as unknown[]).filter((v) => !isBlank(v)).length === 0)),
  );
  return (
    <div className="mb-3 flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        {parameters.map((p) => (
          <FilterPill key={p.id} param={p} value={values[p.slug]} options={optionsBySlug[p.slug] ?? []} onChange={(v) => onChange(p.slug, v)} />
        ))}
      </div>
      {missingRequired.length > 0 && (
        <p className="text-xs text-muted-foreground">Set the required filter(s): {missingRequired.map((p) => p.name).join(", ")}</p>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run it — pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/DashboardFilterBar.tsx frontend/src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx
git commit -m "feat(dashboards): filter pill row + required-filter notice

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `useChartData` `enabled` gate

**Files:**
- Modify: `frontend/src/api/useChartData.ts`
- Test: `frontend/src/api/__tests__/useChartData.enabled.test.tsx`

- [ ] **Step 1: Write the failing test**

`frontend/src/api/__tests__/useChartData.enabled.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { Chart } from "@/api/charts";

const apiJson = vi.fn();
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useChartData } from "../useChartData";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

const chart = { id: "c1", dataset_id: "ds1", viz_type: "bar", params: { spec: { metrics: ["m"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as unknown as Chart;

beforeEach(() => { apiJson.mockReset(); apiJson.mockResolvedValue({ sql: "S", database_id: "db1" }); });

describe("useChartData enabled gate", () => {
  it("fires no request and is not loading when enabled=false", () => {
    const { result } = renderHook(() => useChartData("eng", chart, undefined, false), { wrapper });
    expect(apiJson).not.toHaveBeenCalled();
    expect(result.current.isLoading).toBe(false);
  });
});
```

- [ ] **Step 2: Run it — fail**

Run (from `frontend/`): `npx vitest run src/api/__tests__/useChartData.enabled.test.tsx`
Expected: FAIL — `useChartData` ignores the 4th arg and still builds (apiJson called) / is loading.

- [ ] **Step 3: Implement the gate in `useChartData.ts`**

Change the signature and gate the effect + the loading flag:

```ts
export function useChartData(ws: string, chart: Chart | null, nonce?: number, enabled = true): ChartData {
```

In the effect, add the gate at the top:

```ts
  useEffect(() => {
    let cancelled = false;
    if (!chart || !enabled) return;
    setQueryId(null);
    setError(null);
    // …existing build/submit…
  }, [ws, specKey, nonce, enabled]);
```

And fold `enabled` into `isLoading` so a gated card isn't stuck "loading":

```ts
    isLoading: enabled && !!chart && !error && !failed && !(finished && results.data),
```

- [ ] **Step 4: Run it — pass**

Run (from `frontend/`): `npx vitest run src/api/__tests__/useChartData.enabled.test.tsx`
Expected: PASS.

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/api/useChartData.ts frontend/src/api/__tests__/useChartData.enabled.test.tsx
git commit -m "feat(dashboards): useChartData enabled gate (for required-filter blocking)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Required-gating + empty-tab wiring (`DashboardCardView`, `DashboardView`)

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.required.test.tsx`

- [ ] **Step 1: Thread `blocked` through `DashboardCardView`**

In `DashboardCardView.tsx`, add a `blocked` prop to both `ChartCardTile` and `DashboardCardView` and render a placeholder when blocked (chart cards only):

`ChartCardTile` — add `blocked` to its props and gate the data hook + render:

```tsx
function ChartCardTile({ ws, card, chart, refreshNonce, onValueClick, blocked }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean }) {
  const data = useChartData(ws, chart ?? null, refreshNonce, !blocked);
  const titleOverride = asString(card.options.title);
  const headerText = titleOverride || chart?.name || "";
  const showHeader = titleOverride !== "" && headerText !== "";
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div className="flex h-full flex-col">
      {showHeader && <div className="px-3 pt-2 pb-1 text-sm font-medium">{headerText}</div>}
      <div className="min-h-0 flex-1 overflow-auto px-2 pb-2">
        {blocked ? <div className="flex h-full items-center justify-center px-3 text-center text-sm text-muted-foreground">Set the required filters to load this card</div>
          : data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />}
      </div>
    </div>
  );
}
```

(Note this also applies the Task 6 header restyle — inline title `px-3 pt-2 pb-1`, no `border-b`, content `px-2 pb-2`.)

`DashboardCardView` — add `blocked` to its props and pass it to `ChartCardTile`:

```tsx
export function DashboardCardView({ ws, card, chart, refreshNonce, parameters, paramValues, onValueClick, blocked }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean }) {
  // …text/heading/link branches unchanged…
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} onValueClick={onValueClick} blocked={blocked} />;
}
```

- [ ] **Step 2: Write the failing DashboardView test**

`frontend/src/routes/explore/__tests__/DashboardView.required.test.tsx` (mirror the mock setup from `DashboardView.clickbehavior.test.tsx`; mock `DashboardCardView` so its `blocked` prop is observable, and `@/api/charts`):

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card, blocked }: { card: { id: string }; blocked?: boolean }) => <div data-testid={`card-${card.id}`} data-blocked={blocked ? "1" : "0"} />,
}));
vi.mock("@/api/charts", () => ({ useCharts: () => ({ data: [] }) }));

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "",
  parameters: [{ id: "pr", name: "Region", slug: "region", type: "category", operator: "is", required: true }],
  tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }) };
});

const renderView = (path = "/explore/eng/dashboards/d1") => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={[path]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

describe("DashboardView required-filter gating", () => {
  it("blocks cards while a required filter is unset, unblocks once set", () => {
    renderView();
    expect(screen.getByTestId("card-c1").getAttribute("data-blocked")).toBe("1");
    renderView("/explore/eng/dashboards/d1?region=EU");
    expect(screen.getAllByTestId("card-c1")[1].getAttribute("data-blocked")).toBe("0");
  });
});
```

- [ ] **Step 3: Run it — fail**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.required.test.tsx`
Expected: FAIL — `DashboardView` doesn't compute `blocked` yet.

- [ ] **Step 4: Compute `requiredUnset` + pass `blocked` in `DashboardView.tsx`**

After the `parameters`/`paramsState` are available (and after the early return), add:

```ts
  const requiredUnset = parameters.some(
    (p) => p.required && (isBlank(paramsState.values[p.slug]) || (Array.isArray(paramsState.values[p.slug]) && (paramsState.values[p.slug] as unknown[]).filter((v) => !isBlank(v)).length === 0)),
  );
```

Import `isBlank`: add it to the existing `import { … } from "@/components/chart/dashboard/params"` line (currently imports `slugify, applyParamsToChart`).

In the card render, pass `blocked` to `DashboardCardView` (the existing `<DashboardCardView … />` near the end of the grid children):

```tsx
                <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values}
                  onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} blocked={!editing && requiredUnset} />
```

(Only block in view mode — editing should still show cards.)

- [ ] **Step 5: Run it — pass**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.required.test.tsx`
Expected: PASS.

- [ ] **Step 6: Add the empty-tab state + write its test**

Append to the same test file:

```tsx
describe("DashboardView empty tab", () => {
  it("shows 'No cards on this tab.' when the active tab has no cards", async () => {
    const d2 = { ...detail, parameters: [], tabs: [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }],
      cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t1" }] };
    vi.mocked((await import("@/api/dashboards")).useDashboard).mockReturnValue({ data: d2, isLoading: false } as never);
    render(<QueryClientProvider client={new QueryClient()}><MemoryRouter initialEntries={["/explore/eng/dashboards/d1?tab=1"]}><Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes></MemoryRouter></QueryClientProvider>);
    expect(screen.getByText("No cards on this tab.")).toBeTruthy();
  });
});
```

If mocking the already-mocked `useDashboard` per-test is awkward, instead make `detail` a `let` the test can reassign before render (simpler). Use whichever keeps the suite green; the assertion (`No cards on this tab.` present on a 2-tab dashboard whose tab B has no cards) is the requirement.

In `DashboardView.tsx`, in the grid region, when there are tabs and the active tab has no visible cards, render the message. Find the block that renders `<Grid>` (guarded today by `cards.length === 0 ? <p>No cards yet…</p> : <Grid>…`). Change the inner content so that, when `cards.length > 0` but `visibleCards.length === 0` and `tabs.length > 0`, a message shows instead of an empty grid:

```tsx
      {cards.length === 0 ? (
        <p className="text-muted-foreground">No cards yet. Click Edit to add charts.</p>
      ) : tabs.length > 0 && visibleCards.length === 0 ? (
        <p className="py-12 text-center text-muted-foreground">No cards on this tab.</p>
      ) : (
        <Grid …>{visibleCards.map(…)}</Grid>
      )}
```

- [ ] **Step 7: Run it — pass**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.required.test.tsx`
Expected: PASS (gating + empty-tab). Also run `DashboardView.test.tsx`, `DashboardView.clickbehavior.test.tsx`, `DashboardView.tabs.test.tsx` — all still green.

- [ ] **Step 8: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.required.test.tsx
git commit -m "feat(dashboards): required-filter gating + empty-tab state

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Canvas + card restyle + RGL spacing (`DashboardView.tsx`)

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

(The `ChartCardTile` inline-title restyle already landed in Task 5 Step 1. This task does the canvas + card-shell classes + grid margin.)

- [ ] **Step 1: Muted canvas**

In `DashboardView.tsx`'s returned root wrapper, add the muted canvas bg. The current root is:

```tsx
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-background p-4" : "p-4"}>
```

Change to:

```tsx
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-muted/30 p-4" : "min-h-full bg-muted/30 p-4"}>
```

- [ ] **Step 2: Card shell**

The grid card `<div>` currently is:

```tsx
            <div key={c.id} className={`relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-md border bg-background"}`}>
```

Change the chart/text/link card styling to a Metabase-like shell (heading stays chrome-less):

```tsx
            <div key={c.id} className={`relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-lg border bg-card shadow-sm"}`}>
```

- [ ] **Step 3: Grid spacing**

Add an explicit margin to the RGL `<Grid>` (find the `<Grid className="layout" cols={GRID_COLS} rowHeight={ROW_PX} …>` element) — add the prop:

```tsx
          margin={[12, 12]}
```

- [ ] **Step 4: Verify build + existing tests**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all pass (restyle is class-only; no test asserts the old classes).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx
git commit -m "style(dashboards): Metabase-like canvas, card shell, grid spacing

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Brand-coloured active tab (`DashboardTabBar.tsx`)

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardTabBar.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx` (extend)

- [ ] **Step 1: Add a failing assertion**

Append to the existing `DashboardTabBar.test.tsx`:

```ts
  it("active tab uses the brand colour", () => {
    render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={false}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect(screen.getByRole("tab", { name: "A" }).className).toContain("border-primary");
  });
```

- [ ] **Step 2: Run it — fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`
Expected: FAIL — active class is `border-foreground`, not `border-primary`.

- [ ] **Step 3: Implement**

In `DashboardTabBar.tsx`, change the active-tab class from `border-foreground font-medium` to `border-primary text-primary font-medium` (in the `cls` ternary).

- [ ] **Step 4: Run it — pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`
Expected: PASS (all, incl. the new one).

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/DashboardTabBar.tsx frontend/src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx
git commit -m "style(dashboards): brand-coloured active tab underline

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Live verification (Playwright MCP, side-by-side with Metabase)

**Files:** none (manual verification).

- [ ] **Step 1: Confirm the stack** — egret (frontend `npm run dev`, backend `target/debug/egret serve`) + Metabase (localhost:3001). Log in `admin@egret.local` / `Admin12345!`, ws `eng`.

- [ ] **Step 2: Drive the egret `PARITY — all features` dashboard** (Playwright MCP) and verify, watching for **0 console errors**:
  1. The canvas is muted-gray, cards have rounded corners + soft shadow + inline title (no divider), grid has Metabase-like spacing.
  2. The filter bar is a row of **pills** (no bordering box). Open the **Region** pill → search box + checkbox list; type to filter; check 2 values → pill shows "Region: 2 selections"; the mapped cards refetch filtered and the URL gains `?region=…`. Clear (×) resets.
  3. Open the **Priority** pill similarly. (text/number/date pills: smoke-test on a param of each type if present.)
  4. The active tab underline is brand-coloured.
  5. Mark a param **required** (edit mode, the `req` checkbox) and save; in view mode with it unset, chart cards show "Set the required filters to load this card"; set it → cards load.
  6. An empty second tab shows "No cards on this tab."

- [ ] **Step 3: Compare side-by-side** with Metabase `localhost:3001/dashboard/1` — pills, cards, canvas, tabs should read as the same visual language.

- [ ] **Step 4: Full sweep** — from `frontend/`: `npm run typecheck && npm run lint && npm run build && npx vitest run`. All green.

(The branch `fix/dashboard-parity-gaps` accumulates this tier + the earlier dropdown fix + audit doc; open/refresh the PR when the user asks.)

---

## Notes for the implementer

- **No new deps** — use the existing `@/components/ui/{popover,checkbox,input,button,scroll-area}` and `lucide-react`.
- **Value encodings are unchanged** — `FilterPill` emits exactly what `paramToFilters` already consumes: text → string; number → number or `[min,max]`; date → string / `[start,end]` / relative code; id → scalar; category → string[]. Don't change `paramToFilters`.
- **Rules of Hooks** in `DashboardView`: `requiredUnset` is a plain derivation (not a hook) computed after the early return — no new hooks added, so the existing hook order is untouched.
- **Tabless dashboards** must stay unchanged: no tabs → no empty-tab branch, `visibleCards === cards`.
