# Phase 3d-D1 — Dashboard click behaviour Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make dashboard cards interactive — clicking a value cross-filters the dashboard (sets Phase-C parameters), opens a custom URL, or shows the default drill menu — on table, pivot, and ECharts (bar/line/area/scatter/combo/pie) cards.

**Architecture:** A click on a card yields a `DrillContext` (reuse the existing drill primitives); `ChartRenderer` gains an opt-in `onValueClick` that wires table/pivot `onCellDrill` and a new ECharts `onPointClick`. `DashboardView` resolves each card's `options.clickBehavior` (a pure `resolveClick`) into url / cross-filter (Phase C `setValue`) / drill menu. No backend.

**Tech Stack:** React 18 / TS / Vite / Vitest, `echarts/core`, existing `drill.ts` / `DrillMenu` / `DrillRecordsDialog` / `useDashboardParams` / `interpolateParams`.

**Spec:** `docs/en/specs/2026-06-22-phase-3d-d1-click-behaviour-design.md`

---

## Conventions

- Frontend commands from `frontend/`. Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Configured git identity, never `--author`. Footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Stage explicit paths. Branch `feat/dashboards-phase-d` (checked out).
- **No backend change** — `clickBehavior` lives in the existing `dashboard_cards.options` jsonb.

## Existing code you build on (verified)

- `@/components/chart/drill`: `DrillContext = { dims: { col: string; value: unknown }[] }`, `drillFilters(ctx) → ChartFilter[]`, `tableCellDrill`, `pivotCellDrill`.
- `ChartTable` / `PivotTable` accept `onCellDrill?: (ctx: DrillContext, x: number, y: number) => void` (table requires `dimensionColumns?.length`; pivot fires on leaf value cells). `x,y` are the click's `clientX/clientY`.
- `DrillMenu` props: `{ ctx, x, y, onFilterBy: (f: ChartFilter) => void, onSeeRecords: (fs: ChartFilter[]) => void, onClose: () => void }`.
- `DrillRecordsDialog` props: `{ open, onOpenChange, ws, datasetId, columns: ResultColumn[], filters: ChartFilter[], rowLimit? }`.
- `EChartsRenderer({ option, className })` — init/setOption/resize/dispose. No event support yet.
- `ChartRenderer({ vizType, params, columns, rows, metricTypes })` — read-only; table/pivot branches render `ChartTable`/`PivotTable` WITHOUT `onCellDrill`; cartesian/pie branches render `<EChartsRenderer option={…}/>`.
- `DashboardCardView` → `ChartCardTile` renders `<ChartRenderer .../>`.
- `DashboardView`: `parameters`, `paramsState` (`useDashboardParams`, has `setValue(slug, value)`), per-card `options.paramMappings: {paramId: column}`, `updateCardOptions(cardId, patch)`, renders `<DashboardCardView ws card chart refreshNonce parameters paramValues />` per grid child.
- `@/components/chart/dashboard/params`: `interpolateParams(text, parameters, values)`.
- `@/api/dashboards`: `DashboardParameter { id, name, slug, type, operator, default?, required? }`.

## File structure

```
frontend/src/components/chart/dashboard/
  click-behavior.ts                         NEW  (ClickBehavior type, resolveClick, interpolateUrl)
  __tests__/click-behavior.test.ts          NEW
  DashboardCardView.tsx                      MODIFY (onValueClick passthrough)
frontend/src/components/chart/echarts/
  EChartsRenderer.tsx                        MODIFY (onPointClick)
  __tests__/EChartsRenderer.test.tsx         NEW
frontend/src/components/chart/
  ChartRenderer.tsx                          MODIFY (onValueClick → onCellDrill + onPointClick)
  __tests__/ChartRenderer.test.tsx           MODIFY (onValueClick tests)
frontend/src/routes/explore/
  DashboardView.tsx                          MODIFY (click handling, DrillMenu, DrillRecordsDialog, On-click config)
  __tests__/DashboardView.test.tsx           MODIFY
```

---

## Task 1: `click-behavior.ts` — ClickBehavior type + `resolveClick`

**Files:** Create `frontend/src/components/chart/dashboard/click-behavior.ts` + `__tests__/click-behavior.test.ts`.

- [ ] **Step 1: Write the failing test (verbatim)**

```ts
// frontend/src/components/chart/dashboard/__tests__/click-behavior.test.ts
import { describe, it, expect } from "vitest";
import { resolveClick } from "../click-behavior";
import type { DashboardParameter } from "@/api/dashboards";
import type { DrillContext } from "@/components/chart/drill";

const ctx: DrillContext = { dims: [{ col: "region", value: "EU" }, { col: "status", value: "paid" }] };
const params: DashboardParameter[] = [
  { id: "pr", name: "Region", slug: "region", type: "text", operator: "is" },
  { id: "ps", name: "Status", slug: "stat", type: "category", operator: "is" },
];

describe("resolveClick", () => {
  it("menu when no click behaviour", () => {
    expect(resolveClick(undefined, ctx, params, {})).toEqual({ kind: "menu", ctx });
    expect(resolveClick({ type: "menu" }, ctx, params, {})).toEqual({ kind: "menu", ctx });
  });

  it("crossfilter resolves each mapped column to its parameter slug", () => {
    const cb = { type: "crossfilter" as const, parameterMapping: { pr: "region", ps: "status" } };
    expect(resolveClick(cb, ctx, params, {}).kind).toBe("crossfilter");
    const r = resolveClick(cb, ctx, params, {}) as { kind: "crossfilter"; sets: { slug: string; value: unknown }[] };
    expect(r.sets).toEqual([{ slug: "region", value: "EU" }, { slug: "stat", value: "paid" }]);
  });

  it("crossfilter skips a parameter whose mapped column was not clicked", () => {
    const cb = { type: "crossfilter" as const, parameterMapping: { pr: "missing" } };
    const r = resolveClick(cb, ctx, params, {}) as { kind: "crossfilter"; sets: unknown[] };
    expect(r.sets).toEqual([]);
  });

  it("url interpolates {{column}}, {{value}}, and {{slug}}, then guards http(s)", () => {
    const cb = { type: "url" as const, template: "https://x.com/{{region}}?v={{value}}&from={{stat}}" };
    const r = resolveClick(cb, ctx, params, { stat: "open" }) as { kind: "url"; href: string | null };
    expect(r.href).toBe("https://x.com/EU?v=EU&from=open");
  });

  it("url rejects a non-http(s) result (XSS guard)", () => {
    const cb = { type: "url" as const, template: "javascript:alert({{region}})" };
    const r = resolveClick(cb, ctx, params, {}) as { kind: "url"; href: string | null };
    expect(r.href).toBeNull();
  });
});
```

- [ ] **Step 2: Run → FAIL** (`npx vitest run src/components/chart/dashboard/__tests__/click-behavior.test.ts` — module not found)

- [ ] **Step 3: Implement `click-behavior.ts`**

```ts
// frontend/src/components/chart/dashboard/click-behavior.ts
import type { DashboardParameter } from "@/api/dashboards";
import type { DrillContext } from "@/components/chart/drill";
import { interpolateParams } from "./params";

export type ClickBehavior =
  | { type: "menu" }
  | { type: "crossfilter"; parameterMapping: Record<string, string> }   // { [parameterId]: clickedColumn }
  | { type: "url"; template: string };

export type ResolvedClick =
  | { kind: "url"; href: string | null }
  | { kind: "crossfilter"; sets: { slug: string; value: unknown }[] }
  | { kind: "menu"; ctx: DrillContext };

const SAFE_URL = /^https?:\/\//i;
const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** {{value}} = primary clicked value; {{column_name}} = that column's clicked value; {{slug}} = dashboard param value. */
function interpolateUrl(template: string, ctx: DrillContext, parameters: DashboardParameter[], values: Record<string, unknown>): string {
  let out = template;
  const primary = ctx.dims[0];
  out = out.replace(/\{\{\s*value\s*\}\}/gi, primary ? String(primary.value ?? "") : "");
  for (const d of ctx.dims) {
    out = out.replace(new RegExp(`\\{\\{\\s*${escapeRe(d.col)}\\s*\\}\\}`, "g"), String(d.value ?? ""));
  }
  return interpolateParams(out, parameters, values);   // {{slug}} for dashboard params
}

export function resolveClick(
  cb: ClickBehavior | undefined,
  ctx: DrillContext,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
): ResolvedClick {
  if (cb?.type === "url") {
    const href = interpolateUrl(cb.template, ctx, parameters, values);
    return { kind: "url", href: SAFE_URL.test(href) ? href : null };
  }
  if (cb?.type === "crossfilter") {
    const byId = new Map(parameters.map((p) => [p.id, p]));
    const sets: { slug: string; value: unknown }[] = [];
    for (const [paramId, col] of Object.entries(cb.parameterMapping ?? {})) {
      const dim = ctx.dims.find((d) => d.col === col);
      const param = byId.get(paramId);
      if (dim && param) sets.push({ slug: param.slug, value: dim.value });
    }
    return { kind: "crossfilter", sets };
  }
  return { kind: "menu", ctx };
}
```

- [ ] **Step 4: Run → PASS**, then `npm run typecheck`.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/dashboard/click-behavior.ts frontend/src/components/chart/dashboard/__tests__/click-behavior.test.ts
git commit -m "feat(dashboard): click-behaviour model + resolveClick

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `EChartsRenderer` — `onPointClick`

**Files:** Modify `frontend/src/components/chart/echarts/EChartsRenderer.tsx`; create `__tests__/EChartsRenderer.test.tsx`.

- [ ] **Step 1: Write the failing test**

jsdom can't run ECharts SVG layout; mock `echarts/core` so we can assert the click handler is registered and forwards normalized fields.

```tsx
// frontend/src/components/chart/echarts/__tests__/EChartsRenderer.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";

const handlers: Record<string, (p: unknown) => void> = {};
const chart = {
  setOption: vi.fn(),
  resize: vi.fn(),
  isDisposed: () => false,
  dispose: vi.fn(),
  on: vi.fn((evt: string, cb: (p: unknown) => void) => { handlers[evt] = cb; }),
};
vi.mock("echarts/core", () => ({ init: () => chart, EChartsCoreOption: {} }));
vi.mock("../register", () => ({ registerEChartsModules: () => {} }));
// ResizeObserver shim
beforeEach(() => { (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class { observe() {} disconnect() {} }; });

import { EChartsRenderer } from "../EChartsRenderer";

describe("EChartsRenderer onPointClick", () => {
  it("forwards a click as { name, seriesName, value, x, y }", () => {
    const onPointClick = vi.fn();
    render(<EChartsRenderer option={{}} onPointClick={onPointClick} />);
    handlers["click"]?.({ name: "EU", seriesName: "rev", value: 10, event: { event: { clientX: 5, clientY: 7 } } });
    expect(onPointClick).toHaveBeenCalledWith({ name: "EU", seriesName: "rev", value: 10, x: 5, y: 7 });
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — add the prop + a ref so the init effect needn't depend on the callback, and register `chart.on("click", …)`.

```tsx
import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";

registerEChartsModules();

export interface EChartsPointClick { name?: string; seriesName?: string; value?: unknown; x: number; y: number }

export function EChartsRenderer({ option, className, onPointClick }: {
  option: EChartsCoreOption;
  className?: string;
  onPointClick?: (p: EChartsPointClick) => void;
}) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);
  const clickRef = useRef(onPointClick);
  clickRef.current = onPointClick;

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const chart = init(el, null, { renderer: "svg" });
    chartRef.current = chart;
    chart.setOption(option, true);
    chart.on("click", (params: unknown) => {
      const p = params as { name?: string; seriesName?: string; value?: unknown; event?: { event?: { clientX?: number; clientY?: number } } };
      clickRef.current?.({ name: p.name, seriesName: p.seriesName, value: p.value, x: p.event?.event?.clientX ?? 0, y: p.event?.event?.clientY ?? 0 });
    });
    const ro = new ResizeObserver(() => chart.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (!chart.isDisposed()) chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    chartRef.current?.setOption(option, true);
  }, [option]);

  return <div ref={elRef} className={className ?? "h-full w-full"} />;
}
```

- [ ] **Step 4: Run → PASS** (also confirm the existing cartesian/pie option-builder tests still pass: `npx vitest run src/components/chart/echarts`), then `npm run typecheck`.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/echarts/EChartsRenderer.tsx frontend/src/components/chart/echarts/__tests__/EChartsRenderer.test.tsx
git commit -m "feat(chart): EChartsRenderer onPointClick

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `ChartRenderer` — `onValueClick`

Add an opt-in `onValueClick?: (ctx: DrillContext, x: number, y: number) => void`. When set: table/pivot pass it as `onCellDrill` (same signature); cartesian/pie wire `EChartsRenderer.onPointClick` to build a `DrillContext` from the clicked point's dimension column. Other viz types ignore it.

**Files:** Modify `frontend/src/components/chart/ChartRenderer.tsx`; modify `__tests__/ChartRenderer.test.tsx`.

- [ ] **Step 1: Append failing tests** to `src/components/chart/__tests__/ChartRenderer.test.tsx`

(The file already mocks `EChartsRenderer`. Update that mock so it can fire `onPointClick`, and add tests.)

```tsx
// Update the existing EChartsRenderer mock to expose onPointClick:
vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ onPointClick }: { onPointClick?: (p: { name?: string; seriesName?: string; value?: unknown; x: number; y: number }) => void }) => (
    <button data-testid="echarts" onClick={() => onPointClick?.({ name: "EU", seriesName: "rev", value: 10, x: 3, y: 4 })} />
  ),
}));

// add inside the describe:
import { fireEvent } from "@testing-library/react";

it("calls onValueClick with the xAxis dim when a cartesian point is clicked", () => {
  const onValueClick = vi.fn();
  render(
    <ChartRenderer
      vizType="bar"
      params={params({ cartesian: { xAxis: "region", yMetrics: ["rev"] } })}
      columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]}
      rows={[["EU", 10]]}
      onValueClick={onValueClick}
    />,
  );
  fireEvent.click(screen.getByTestId("echarts"));
  expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "region", value: "EU" }] }, 3, 4);
});

it("threads onValueClick to a table card's onCellDrill (passed through)", () => {
  // ChartTable is real here; assert ChartRenderer forwards the prop without throwing.
  const onValueClick = vi.fn();
  render(
    <ChartRenderer
      vizType="table"
      params={params({ spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } })}
      columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]}
      rows={[["EU", 10]]}
      onValueClick={onValueClick}
    />,
  );
  // a dimension cell becomes clickable; clicking it calls onValueClick
  fireEvent.click(screen.getByText("EU"));
  expect(onValueClick).toHaveBeenCalled();
});
```
(If the table test's cell click is fiddly in jsdom, assert instead that `screen.getByText("EU").closest("td")` has the `cursor-pointer` class — i.e. `onCellDrill` was wired. Keep one of the two assertions that reliably passes.)

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — add the prop and wire it.

Signature:
```tsx
import type { DrillContext } from "@/components/chart/drill";
export interface ChartRendererProps {
  vizType: VizType; params: ChartParams; columns: ResultColumn[]; rows: unknown[][];
  metricTypes?: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
}
```
- **Table** branch: add `onCellDrill={onValueClick}` to `<ChartTable …/>` (it already receives `dimensionColumns`).
- **Pivot** branch: add `onCellDrill={onValueClick}` to `<PivotTable …/>`.
- **Cartesian** branch: pass an `onPointClick` to `EChartsRenderer` that builds a `DrillContext` from the clicked point using `cartesian.xAxis` (and `cartesian.series` as a 2nd dim when set):
```tsx
onPointClick={onValueClick && cartesian.xAxis ? (p) => {
  const dims = [{ col: cartesian.xAxis as string, value: p.name }];
  if (cartesian.series && p.seriesName != null) dims.push({ col: cartesian.series, value: p.seriesName });
  onValueClick({ dims }, p.x, p.y);
} : undefined}
```
- **Pie** branch: similarly with `pie.dimension`:
```tsx
onPointClick={onValueClick && pie.dimension ? (p) => onValueClick({ dims: [{ col: pie.dimension as string, value: p.name }] }, p.x, p.y) : undefined}
```
- Leave gauge/funnel/waterfall/boxplot/sankey/scalar/progress/smartscalar unchanged (no click — out of D1 scope).

- [ ] **Step 4: Run → PASS** (new tests + all existing ChartRenderer tests), then `npm run typecheck && npm run lint`.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/ChartRenderer.tsx frontend/src/components/chart/__tests__/ChartRenderer.test.tsx
git commit -m "feat(chart): ChartRenderer onValueClick (table/pivot + cartesian/pie)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `DashboardCardView` — pass `onValueClick` through

**Files:** Modify `frontend/src/components/chart/dashboard/DashboardCardView.tsx`; add a test to its existing test file.

- [ ] **Step 1: Failing test** — append to `src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx`

```tsx
it("threads onValueClick to a chart card", () => {
  const onValueClick = vi.fn();
  // the ChartRenderer mock in this file should call onValueClick; if it doesn't, extend the mock:
  render(<DashboardCardView ws="eng" card={card({ chart_id: "c1" })} chart={chart} onValueClick={onValueClick} />);
  // assertion depends on the mock; see Step 3
});
```
Update the existing `ChartRenderer` mock in this test file to expose the prop:
```tsx
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: ({ onValueClick }: { onValueClick?: (ctx: unknown, x: number, y: number) => void }) =>
    <button data-testid="chart" onClick={() => onValueClick?.({ dims: [{ col: "region", value: "EU" }] }, 1, 2)} />,
}));
```
and the test body:
```tsx
  fireEvent.click(screen.getByTestId("chart"));
  expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "region", value: "EU" }] }, 1, 2);
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — add `onValueClick?` to `DashboardCardView` and `ChartCardTile`, threading to `ChartRenderer`:
```tsx
function ChartCardTile({ ws, card, chart, refreshNonce, onValueClick }: { …; onValueClick?: (ctx: DrillContext, x: number, y: number) => void }) {
  …
  : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />}
}
export function DashboardCardView({ …, onValueClick }: { …; onValueClick?: (ctx: DrillContext, x: number, y: number) => void }) {
  … // text/heading/link unchanged
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} onValueClick={onValueClick} />;
}
```
(Import `DrillContext` type.)

- [ ] **Step 4: Run → PASS**, typecheck/lint.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx
git commit -m "feat(dashboard): DashboardCardView onValueClick passthrough

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `DashboardView` — click handling (resolve → url / cross-filter / menu)

Wire each card's `onValueClick` to `resolveClick`, and add the `DrillMenu` + `DrillRecordsDialog` (cross-filter on "Filter by", records on "See these records").

**Files:** Modify `frontend/src/routes/explore/DashboardView.tsx`; create a NEW test file `frontend/src/routes/explore/__tests__/DashboardView.clickbehavior.test.tsx`.

**Why a new test file:** the cross-filter test needs `DashboardCardView` mocked to a clickable button that fires `onValueClick`. Mocking it inside the existing `DashboardView.test.tsx` would break the Phase A/B card-content assertions. A separate file gets its own module mocks, isolating the conflict.

- [ ] **Step 1: Write the failing test — `DashboardView.clickbehavior.test.tsx`**

Self-contained (copy the mock + `renderAt` shape from `DashboardView.test.tsx`, but mock `DashboardCardView` to a per-card click button):
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

const dashboard = {
  id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
  parameters: [{ id: "pr", name: "Region", slug: "region", type: "text", operator: "is" }],
  cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4,
    options: { clickBehavior: { type: "crossfilter", parameterMapping: { pr: "region" } } } }],
};
vi.mock("@/api/dashboards", () => ({
  useDashboard: () => ({ data: dashboard, isLoading: false }),
  useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
    params: { spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
    created_by: null, updated_at: "" }] }),
}));
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card, onValueClick }: { card: { id: string }; onValueClick?: (ctx: unknown, x: number, y: number) => void }) =>
    <button data-testid={`cardclick-${card.id}`} onClick={() => onValueClick?.({ dims: [{ col: "region", value: "EU" }] }, 0, 0)} />,
}));

function renderAt() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DashboardView click behaviour", () => {
  it("cross-filters when a card's click behaviour is crossfilter", () => {
    renderAt();
    fireEvent.click(screen.getByTestId("cardclick-k1"));
    expect((screen.getByLabelText("Region") as HTMLInputElement).value).toBe("EU");
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** in `DashboardView.tsx`

Imports: `resolveClick, type ClickBehavior` from `@/components/chart/dashboard/click-behavior`; `DrillMenu` from `@/components/chart/DrillMenu`; `DrillRecordsDialog` from `@/components/chart/DrillRecordsDialog`; `drillFilters, type DrillContext` from `@/components/chart/drill`; `type ChartFilter` from `@/api/charts`.

Add state ABOVE the early return (Rules of Hooks):
```tsx
const [drillMenu, setDrillMenu] = useState<{ ctx: DrillContext; x: number; y: number; datasetId: string | null } | null>(null);
const [records, setRecords] = useState<{ filters: ChartFilter[]; datasetId: string | null } | null>(null);
```

A handler (defined after `dashboard`/`parameters`/`paramsState` exist):
```tsx
function onCardValueClick(card: Card, ctx: DrillContext, x: number, y: number) {
  const cb = card.options.clickBehavior as ClickBehavior | undefined;
  const r = resolveClick(cb, ctx, parameters, paramsState.values);
  const datasetId = chartById.get(card.chart_id ?? "")?.dataset_id ?? null;
  if (r.kind === "url") { if (r.href) window.open(r.href, "_blank", "noreferrer"); return; }
  if (r.kind === "crossfilter") { for (const s of r.sets) paramsState.setValue(s.slug, s.value); return; }
  setDrillMenu({ ctx, x, y, datasetId });   // menu
}
```

Pass it to each chart card:
```tsx
<DashboardCardView … onValueClick={(ctx, x, y) => onCardValueClick(c, ctx, x, y)} />
```

Render the drill menu + records dialog near the end of the returned JSX (mirrors ChartExplore). "Filter by {col}" cross-filters any parameter mapped (Phase C) to that column on any card — simplest: set the parameter whose `slug`/mapping matches; here, find a parameter whose column appears in the filter and `setValue`:
```tsx
{drillMenu && (
  <DrillMenu ctx={drillMenu.ctx} x={drillMenu.x} y={drillMenu.y}
    onFilterBy={(f) => {
      // cross-filter: set any dashboard parameter that some card maps to this column
      const target = parameters.find((p) =>
        (dashboard.cards).some((c) => ((c.options.paramMappings as Record<string,string>) ?? {})[p.id] === f.col));
      if (target) paramsState.setValue(target.slug, f.value);
    }}
    onSeeRecords={(fs) => setRecords({ filters: fs, datasetId: drillMenu.datasetId })}
    onClose={() => setDrillMenu(null)} />
)}
<DrillRecordsDialog
  open={records !== null}
  onOpenChange={(o) => { if (!o) setRecords(null); }}
  ws={ws} datasetId={records?.datasetId ?? null}
  columns={[]}            /* DrillRecordsDialog re-derives columns from its query results */
  filters={records?.filters ?? []}
/>
```
(If `DrillRecordsDialog` requires a non-empty `columns` prop, pass the clicked card's chart result columns if readily available, else `[]` — read the component; it derives display columns from the query result, so `[]` is acceptable for the header.)

- [ ] **Step 4: Run tests + full sweep** (`npx vitest run`, then typecheck/lint/build). All Phase A/B/C/D tests green.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.clickbehavior.test.tsx
git commit -m "feat(dashboard): wire card clicks → cross-filter / url / drill menu

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Edit-mode "On click" config per chart card

A per-card control (edit mode) to set `options.clickBehavior`: Menu (default) / Cross-filter (rows of parameter→clicked-column) / Custom URL (template input).

**Files:** Modify `frontend/src/routes/explore/DashboardView.tsx`; append a test.

- [ ] **Step 1: Failing test (append)**

```tsx
describe("DashboardView (Phase D1 — on-click config)", () => {
  it("sets a card's click behaviour to a custom URL in edit mode", () => {
    dashState.current = {
      data: { ...(dashState.loaded.data as object),
        cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {} }] },
      isLoading: false,
    };
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.change(screen.getByLabelText(/on click/i), { target: { value: "url" } });
    const tpl = screen.getByLabelText(/url template/i);
    fireEvent.change(tpl, { target: { value: "https://x.com/{{region}}" } });
    expect((tpl as HTMLInputElement).value).toBe("https://x.com/{{region}}");
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — inside each chart card's edit-mode block (next to the title input / mapping dropdowns), render the On-click control. Read the current `clickBehavior`:
```tsx
{editing && c.card_type === "chart" && (() => {
  const cb = (c.options.clickBehavior as ClickBehavior | undefined) ?? { type: "menu" };
  const ch = chartById.get(c.chart_id ?? "");
  const cols = ch ? [...ch.params.spec.dimensions, ...ch.params.spec.columns] : [];
  return (
    <div className="m-1 flex flex-col gap-0.5 border-t pt-1 text-[10px] text-muted-foreground">
      <label className="flex items-center justify-between gap-1">
        <span>On click</span>
        <select aria-label="on click" className="h-6 rounded border bg-background text-[10px]"
          value={cb.type}
          onChange={(e) => {
            const t = e.target.value as ClickBehavior["type"];
            updateCardOptions(c.id, { clickBehavior:
              t === "url" ? { type: "url", template: "" }
              : t === "crossfilter" ? { type: "crossfilter", parameterMapping: {} }
              : { type: "menu" } });
          }}>
          <option value="menu">Menu</option>
          <option value="crossfilter">Cross-filter</option>
          <option value="url">Custom URL</option>
        </select>
      </label>
      {cb.type === "url" && (
        <input aria-label="url template" placeholder="https://…/{{column}}?p={{slug}}"
          className="rounded border bg-background px-1 text-[10px]"
          value={cb.template}
          onChange={(e) => updateCardOptions(c.id, { clickBehavior: { type: "url", template: e.target.value } })} />
      )}
      {cb.type === "crossfilter" && parameters.map((p) => {
        const mapping = (cb.parameterMapping ?? {});
        return (
          <label key={p.id} className="flex items-center justify-between gap-1">
            <span>{p.name} ←</span>
            <select aria-label={`crossfilter ${p.name}`} className="h-6 rounded border bg-background text-[10px]"
              value={mapping[p.id] ?? ""}
              onChange={(e) => updateCardOptions(c.id, { clickBehavior: { type: "crossfilter", parameterMapping: { ...mapping, [p.id]: e.target.value } } })}>
              <option value="">none</option>
              {cols.map((col) => <option key={col} value={col}>{col}</option>)}
            </select>
          </label>
        );
      })}
    </div>
  );
})()}
```
(`ClickBehavior` import already added in Task 5.)

- [ ] **Step 4: Run tests + full sweep.** All green.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): per-card On-click config (menu/crossfilter/url)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Live verification (Playwright MCP) + PR

- [ ] **Step 1: Stack** — backend `:8080` + frontend `:5173` already run on `feat/dashboards-phase-d` (no migration this phase). Log in `admin@egret.local` / `Admin12345!` if the session lapsed.

- [ ] **Step 2: Manual live flow (Playwright MCP browser, real Chrome):** open a dashboard, add a bar/row chart (region dimension) + a `text` parameter "Region" mapped (Phase C) to `region`; Edit → set the chart card's **On click = Cross-filter**, map the param ← `region`; Save. In view mode **click a bar/row** → the parameter value updates (the filter bar shows it, the URL gets `?region=…`, and the card refetches filtered). Then set another card's **On click = Custom URL** `https://example.com/{{region}}` → clicking opens it. Set a card to **Menu** → click → DrillMenu → "See these records" shows rows. Confirm `mcp__playwright__browser_console_messages` = **0 errors**. (Manual MCP browser driving — egret has no automated Playwright suite.)

- [ ] **Step 3: Clean up** any test dashboards created.

- [ ] **Step 4: Open the PR**
```bash
git push -u origin feat/dashboards-phase-d
gh pr create --base main --title "feat(dashboards): Phase D1 — click behaviour (cross-filter / URL / drill menu)" --body "<summary; reference docs/en/specs/2026-06-22-phase-3d-d1-click-behaviour-design.md; note manual Playwright-MCP verification (not an automated test); 🤖 Generated with [Claude Code](https://claude.com/claude-code)>"
```
Wait for CI; squash-merge.

---

## Acceptance (D1)

- A chart/table/pivot card can be configured (Menu / Cross-filter / Custom URL); clicking a value does the right thing.
- Cross-filter sets the mapped dashboard parameter(s) → mapped cards refetch (Phase C) and the URL syncs.
- Custom URL opens the `{{column}}`/`{{slug}}`-interpolated link (http(s) only).
- The default menu offers cross-filter "Filter by …" + "See these records".
- All Phase A/B/C behaviour intact; full sweep green; live MCP check has 0 console errors.
