# Phase 3d — Dashboards (Phase A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the core of Metabase-style dashboards — a 24-column grid of saved-chart cards with view and edit modes — on top of the already-built backend.

**Architecture:** The render switch is lifted out of `ChartExplore.tsx` into a pure `<ChartRenderer>` and a `useChartData` hook (build→submit→poll). Dashboards reuse both: each card runs its chart's query independently and renders via `ChartRenderer`. Layout is a flat list of cards persisted with a whole-layout PUT; the grid uses `react-grid-layout`.

**Tech Stack:** React 18 + TypeScript + Vite + Vitest, `@tanstack/react-query`, `react-grid-layout`, existing `EChartsRenderer` / `ChartTable` / `BigNumber` and chart option builders, shadcn/ui.

**Spec:** `docs/en/specs/2026-06-21-phase-3d-dashboards-design.md`

---

## Conventions for the implementer

- **Run all frontend commands from `frontend/`.** (`cd frontend` first.)
- **Full sweep before each commit:** `npm run typecheck && npm run lint && npm run build && npx vitest run`. (Use `npx vitest run`, not the rtk-wrapped `npm test`, so output is visible. If a sandbox swallows vitest output, use `rtk proxy npx vitest run`.)
- **Commit** with the configured git identity — **never** `--author`. End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Stage only explicit paths** — never `git add -A` / `git add .`.
- Branch is already `feat/dashboards-phase-a` (off `main`). Do not create a new branch.
- Query result shape (from `@/api/sqllab`): `columns: { name: string; type: string }[]`, `rows: unknown[][]`.
- Clean-room: copy Metabase behaviour, never its AGPL source.

## Backend baseline (already built — do NOT re-implement)

These are merged into the branch and green; treat them as fixed API:

- Migration `migrations/0015_dashboard.sql` — `dashboards`, `dashboard_cards` (`grid_col, grid_row, size_x, size_y, card_type, options, ordinal`), perms `dashboard:read` / `dashboard:manage`.
- `crates/egret-bi/src/dashboard.rs` — `DashboardRepo` (`create`, `get`, `list_by_workspace` → `(Dashboard, card_count)`, `update_meta`, `delete`, `cards`, `replace_cards`, `dashboard_workspace`, `owner`).
- `crates/egret-bi/src/routes.rs` — `dashboard_router()`:
  - `GET  /ws/:w/dashboards` → `DashboardSummaryDto[]` `{ id, name, description, created_by, card_count, updated_at }`
  - `POST /ws/:w/dashboards` body `{ name, description? }` → `DashboardDetailDto`
  - `GET  /ws/:w/dashboards/:id` → `DashboardDetailDto` `{ id, name, description, created_by, updated_at, cards: DashboardCardDto[] }`
  - `PUT  /ws/:w/dashboards/:id` body `{ name, description?, cards: CardInput[] }` → `DashboardDetailDto`
  - `DELETE /ws/:w/dashboards/:id` → `{ ok: true }`
  - `DashboardCardDto` = `{ id, chart_id: string|null, card_type, grid_col, grid_row, size_x, size_y, options }`
  - `CardInput` = `{ chart_id?, card_type?, grid_col, grid_row, size_x, size_y, options? }`

The frontend types in Task 4 MUST match these field names exactly.

## File structure

```
frontend/src/
  components/chart/
    ChartRenderer.tsx                 NEW  — pure viz dispatch (lifted from ChartExplore)
    __tests__/ChartRenderer.test.tsx  NEW
  api/
    dashboards.ts                     NEW  — types + react-query hooks
    __tests__/dashboards.test.ts      NEW  — (type/shape smoke; optional)
    useChartData.ts                   NEW  — build→submit→poll for a saved chart
    __tests__/useChartData.test.tsx   NEW
  routes/explore/
    ChartExplore.tsx                  MODIFY — consume <ChartRenderer>
    Dashboards.tsx                    MODIFY — placeholder → list page
    DashboardView.tsx                 NEW  — the grid (view + edit)
    __tests__/Dashboards.test.tsx     NEW
    __tests__/DashboardView.test.tsx  NEW
  App.tsx                             MODIFY — add dashboards/:id route
  components/chart/dashboard-grid.ts  NEW  — grid constants (GRID_COLS=24, etc.)
```

---

## Task 1: Extract `<ChartRenderer>` (pure viz dispatch)

Lift the result-render switch (`ChartExplore.tsx:824-924`) into a pure component that takes a chart's `viz_type` + `params` + a `(columns, rows)` result and renders the right visualization. No data fetching, no live ChartExplore state.

**Files:**
- Create: `frontend/src/components/chart/ChartRenderer.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartRenderer.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/__tests__/ChartRenderer.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams } from "@/api/charts";

// jsdom has no layout; stub the ECharts renderer so cartesian/pie/etc. don't touch canvas.
vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: () => <div data-testid="echarts" />,
}));

const params = (p: Partial<ChartParams> = {}): ChartParams => ({
  spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
  ...p,
});

describe("ChartRenderer", () => {
  it("renders a table viz with formatted cells", () => {
    render(
      <ChartRenderer
        vizType="table"
        params={params()}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1234.5]]}
      />,
    );
    expect(screen.getByText("region")).toBeInTheDocument();
    expect(screen.getByText("1,234.5")).toBeInTheDocument();
  });

  it("renders a big number for scalar viz", () => {
    render(
      <ChartRenderer
        vizType="scalar"
        params={params({ scalar: { metric: "revenue" } })}
        columns={[{ name: "revenue", type: "double" }]}
        rows={[[42000]]}
      />,
    );
    expect(screen.getByText((t) => t.replace(/,/g, "") === "42000")).toBeInTheDocument();
  });

  it("dispatches cartesian viz to the ECharts renderer", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["revenue"] } })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 10], ["US", 20]]}
      />,
    );
    expect(screen.getByTestId("echarts")).toBeInTheDocument();
  });

  it("shows an empty state for cartesian with no rows", () => {
    render(
      <ChartRenderer
        vizType="bar"
        params={params({ cartesian: { xAxis: "region", yMetrics: ["revenue"] } })}
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[]}
      />,
    );
    expect(screen.getByText("No data")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/__tests__/ChartRenderer.test.tsx`
Expected: FAIL — `Cannot find module '../ChartRenderer'`.

- [ ] **Step 3: Implement `ChartRenderer.tsx`**

Create the component by lifting the JSX switch from `ChartExplore.tsx:824-924` **verbatim**, changing only where its inputs come from: instead of ChartExplore's live state (`viz`, `cartesian`, `pie`, `columnSettings`, …) read them from props/`params`. Use this exact signature and derivation preamble, then paste the existing branches (table, pivot_table, cartesian, pie, scalar, progress, gauge, funnel, waterfall, smartscalar, boxplot, sankey), swapping `results.data` → `{ columns, rows }`, `viz` → `vizType`, and the config locals → the destructured ones below.

```tsx
// frontend/src/components/chart/ChartRenderer.tsx
import type { ChartParams, VizType } from "@/api/charts";
import { isCartesianViz } from "@/api/charts";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import { buildGaugeOption } from "@/components/chart/echarts/options/gauge";
import { buildFunnelOption } from "@/components/chart/echarts/options/funnel";
import { buildWaterfallOption } from "@/components/chart/echarts/options/waterfall";
import { buildBoxplotOption } from "@/components/chart/echarts/options/boxplot";
import { buildSankeyOption } from "@/components/chart/echarts/options/sankey";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { BigNumber } from "@/components/chart/BigNumber";
import { ProgressChart } from "@/components/chart/ProgressChart";
import { SmartScalarChart } from "@/components/chart/SmartScalarChart";

type Col = { name: string; type: string };

export interface ChartRendererProps {
  vizType: VizType;
  params: ChartParams;
  columns: Col[];
  rows: unknown[][];
  /** metric_name -> data_type, for metric value formatting. Optional; defaults to {}. */
  metricTypes?: Record<string, string>;
}

export function ChartRenderer({ vizType, params, columns, rows, metricTypes = {} }: ChartRendererProps) {
  const cartesian = params.cartesian ?? { yMetrics: [] };
  const pie = params.pie ?? {};
  const scalar = params.scalar ?? {};
  const progress = params.progress ?? {};
  const gauge = params.gauge ?? {};
  const funnel = params.funnel ?? {};
  const waterfall = params.waterfall ?? {};
  const smart = params.smartscalar ?? {};
  const box = params.boxplot ?? {};
  const sankey = params.sankey ?? {};
  const columnSettings = params.columnSettings ?? {};
  const conditionalFormatting = params.conditionalFormatting ?? [];
  const tableState = params.tableState ?? {};
  // Table grouping columns mirror ChartExplore: aggregated table → its dimensions.
  const dimensionColumns = params.spec.metrics.length > 0 ? params.spec.dimensions : [];

  // ── paste the branches from ChartExplore.tsx:824-924 here, adapting inputs ──
  // Each branch becomes `if (vizType === "table") return (...)` etc., reading
  // `columns`/`rows` (not results.data), the locals above, and OMITTING the
  // onCellDrill / onTableStateChange / pivot-interaction handlers (dashboards
  // are read-only in Phase A — pass no-op or omit optional props).
  // Keep the same wrapper divs and "No data" empty states.
}
```

Notes for the paste:
- `table`: `<ChartTable columns={columns} rows={rows} columnSettings={columnSettings} conditionalFormatting={conditionalFormatting} tableState={tableState} dimensionColumns={dimensionColumns} />` (drop `onTableStateChange`/`onCellDrill`).
- `pivot_table`: render `<PivotTable .../>` from `params.rows/cols/metrics/showRowTotals/showColTotals/condense` with `collapsedRows`/`pivotSort` read from params; omit the interaction callbacks (`onToggleCollapse`, `onCycleSort`, `onCellDrill`).
- All `EChartsRenderer` branches: copy the builder-call objects verbatim from the source lines, replacing `results.data` with `{ columns, rows }` and keeping `columnSettings, metricTypes`.
- Default the final case (unknown viz) to `null`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/components/chart/__tests__/ChartRenderer.test.tsx`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/ChartRenderer.tsx frontend/src/components/chart/__tests__/ChartRenderer.test.tsx
git commit -m "refactor(chart): extract pure ChartRenderer viz dispatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Refactor `ChartExplore` to consume `<ChartRenderer>`

Replace the 824-924 switch in `ChartExplore.tsx` with a single `<ChartRenderer>` call so behaviour is unchanged and the existing ChartExplore/chart tests stay green. ChartExplore keeps its live config state but now passes it through `params`-shaped objects.

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx:824-924`

- [ ] **Step 1: Replace the switch**

Delete lines `824-924` (the `{finished && results.data && viz === "table" && (...)}` … sankey block) and the per-branch imports that are now only used by `ChartRenderer` (leave imports still used elsewhere in ChartExplore, e.g. the settings panels). Insert:

```tsx
{finished && results.data && (
  viz === "pivot_table"
    ? (/* keep the existing interactive PivotTable block verbatim — pivot
          interactions (collapse, sort, drill) remain live in Explore */)
    : viz === "table"
      ? (/* keep the existing interactive ChartTable block verbatim —
            Explore keeps onTableStateChange + onCellDrill */)
      : (
        <ChartRenderer
          vizType={viz}
          params={paramsForSave()}
          columns={results.data.columns}
          rows={results.data.rows}
          metricTypes={metricTypes}
        />
      )
)}
```

Rationale: Explore’s table/pivot keep their interactive callbacks (drill, sort, resize), which dashboards don’t use — so only the non-interactive vizzes route through `ChartRenderer` here. The duplication is intentional and small. (`paramsForSave()` already exists in ChartExplore and returns a `ChartParams`.)

Add the import: `import { ChartRenderer } from "@/components/chart/ChartRenderer";`

- [ ] **Step 2: Run the full frontend test suite**

Run: `npx vitest run`
Expected: PASS — all pre-existing tests (incl. cartesian/table/format) still green; no snapshot or behaviour change.

- [ ] **Step 3: Typecheck + lint + build**

Run: `npm run typecheck && npm run lint && npm run build`
Expected: clean. Remove any now-unused imports the linter flags in `ChartExplore.tsx`.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(explore): render non-interactive viz via ChartRenderer

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `useChartData` hook (build→submit→poll for a saved chart)

A hook that, given a saved chart, runs the existing pipeline and returns its result. Used by each dashboard card.

**Files:**
- Create: `frontend/src/api/useChartData.ts`
- Test: `frontend/src/api/__tests__/useChartData.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/api/__tests__/useChartData.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

const apiJson = vi.fn();
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useChartData } from "../useChartData";
import type { Chart } from "@/api/charts";

const chart: Chart = {
  id: "c1", dataset_id: "ds1", name: "S", description: "", viz_type: "table",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "",
};

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiJson.mockReset();
  // /charts/data -> sql + database_id ; /queries (POST) -> id ; /queries/:id -> finished ; /results -> data
  apiJson.mockImplementation((url: string, opts?: { method?: string }) => {
    if (url.endsWith("/charts/data")) return Promise.resolve({ sql: "SELECT 1", database_id: "db1" });
    if (url.endsWith("/queries") && opts?.method === "POST") return Promise.resolve({ id: "q1", status: "finished" });
    if (url.includes("/queries/q1/results")) return Promise.resolve({ columns: [{ name: "rev", type: "double" }], rows: [[5]], page: 0, page_size: 100, row_count: 1, truncated: false });
    if (url.includes("/queries/q1")) return Promise.resolve({ id: "q1", database_id: "db1", status: "finished", columns: null, preview: null, row_count: 1, truncated: false, error: null });
    return Promise.reject(new Error("unexpected " + url));
  });
});

describe("useChartData", () => {
  it("builds, submits, polls, and returns the result rows", async () => {
    const { result } = renderHook(() => useChartData("eng", chart), { wrapper });
    await waitFor(() => expect(result.current.columns.length).toBe(1));
    expect(result.current.rows).toEqual([[5]]);
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/api/__tests__/useChartData.test.tsx`
Expected: FAIL — `Cannot find module '../useChartData'`.

- [ ] **Step 3: Implement the hook**

```tsx
// frontend/src/api/useChartData.ts
import { useEffect, useState } from "react";
import type { Chart } from "@/api/charts";
import { useBuildChartData } from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";

type Col = { name: string; type: string };

export interface ChartData {
  columns: Col[];
  rows: unknown[][];
  isLoading: boolean;
  error: string | null;
}

/** Run a saved chart's query via the existing build→submit→poll pipeline. */
export function useChartData(ws: string, chart: Chart | null): ChartData {
  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  // Kick off once per (chart id + serialized spec).
  const specKey = chart ? `${chart.id}:${JSON.stringify(chart.params.spec)}` : "";
  useEffect(() => {
    let cancelled = false;
    if (!chart) return;
    setQueryId(null);
    setError(null);
    (async () => {
      try {
        const { sql, database_id } = await build.mutateAsync({ dataset_id: chart.dataset_id, query_spec: chart.params.spec });
        const q = await submit.mutateAsync({ database_id, sql, limit: chart.params.spec.row_limit });
        if (!cancelled) setQueryId(q.id);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ws, specKey]);

  const failed = status.data?.status === "failed" || status.data?.status === "cancelled";
  return {
    columns: results.data?.columns ?? [],
    rows: results.data?.rows ?? [],
    isLoading: !!chart && !error && !failed && !(finished && results.data),
    error: error ?? (failed ? (status.data?.error ?? "Query failed") : null),
  };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/api/__tests__/useChartData.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/useChartData.ts frontend/src/api/__tests__/useChartData.test.tsx
git commit -m "feat(api): useChartData hook to run a saved chart's query

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `api/dashboards.ts` (types + hooks)

Mirror `api/charts.ts`. Field names MUST match the backend DTOs (see baseline).

**Files:**
- Create: `frontend/src/api/dashboards.ts`

- [ ] **Step 1: Implement the module**

```tsx
// frontend/src/api/dashboards.ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export interface DashboardCard {
  id: string;
  chart_id: string | null;
  card_type: string;            // "chart" (Phase A)
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options: Record<string, unknown>;
}
export interface DashboardSummary {
  id: string; name: string; description: string;
  created_by: string | null; card_count: number; updated_at: string;
}
export interface DashboardDetail {
  id: string; name: string; description: string;
  created_by: string | null; updated_at: string;
  cards: DashboardCard[];
}
/** Card payload for a layout save (no server id). */
export interface CardInput {
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options?: Record<string, unknown>;
}

export const dashboardKeys = {
  list: (ws: string) => ["dashboards", ws] as const,
  one: (ws: string, id: string) => ["dashboards", ws, id] as const,
};

export function useDashboards(ws: string) {
  return useQuery<DashboardSummary[]>({
    queryKey: dashboardKeys.list(ws),
    queryFn: () => apiJson<DashboardSummary[]>(`/ws/${ws}/dashboards`),
  });
}

export function useDashboard(ws: string, id: string | null) {
  return useQuery<DashboardDetail>({
    queryKey: id ? dashboardKeys.one(ws, id) : ["dashboards", ws, "none"],
    queryFn: () => apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`),
    enabled: !!id,
  });
}

export function useDashboardMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = (id?: string) => {
    qc.invalidateQueries({ queryKey: dashboardKeys.list(ws) });
    if (id) qc.invalidateQueries({ queryKey: dashboardKeys.one(ws, id) });
  };
  const create = useMutation({
    mutationFn: (b: { name: string; description?: string }) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: () => invalidate(),
  });
  const update = useMutation({
    mutationFn: ({ id, ...b }: { id: string; name: string; description?: string; cards: CardInput[] }) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: (d) => invalidate(d.id),
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/dashboards/${id}`, { method: "DELETE" }),
    onSuccess: () => invalidate(),
  });
  return { create, update, remove };
}
```

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/api/dashboards.ts
git commit -m "feat(api): dashboards types + react-query hooks

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Grid constants

**Files:**
- Create: `frontend/src/components/chart/dashboard-grid.ts`

- [ ] **Step 1: Implement constants (mirroring Metabase)**

```ts
// frontend/src/components/chart/dashboard-grid.ts
// Mirrors Metabase's dashboard grid (frontend/src/metabase/utils/dashboard_grid.ts).
export const GRID_COLS = 24;             // GRID_WIDTH
export const GRID_ASPECT_RATIO = 10 / 9; // cell w:h
export const GRID_MIN_ROW_HEIGHT = 40;   // px (MIN_ROW_HEIGHT)
export const CARD_DEFAULT_SIZE = { w: 6, h: 4 };  // when a card is first added
export const CARD_MIN_SIZE = { w: 2, h: 2 };

/** rowHeight in px for a given container width (RGL needs a fixed px row height). */
export function gridRowHeight(containerWidth: number): number {
  return Math.max(Math.floor(containerWidth / GRID_COLS / GRID_ASPECT_RATIO), GRID_MIN_ROW_HEIGHT);
}
```

- [ ] **Step 2: Commit**

```bash
git add frontend/src/components/chart/dashboard-grid.ts
git commit -m "feat(dashboard): 24-col grid constants

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `Dashboards` list page

Replace the placeholder with a list (mirror `Charts.tsx`).

**Files:**
- Modify: `frontend/src/routes/explore/Dashboards.tsx`
- Test: `frontend/src/routes/explore/__tests__/Dashboards.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/routes/explore/__tests__/Dashboards.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import Dashboards from "../Dashboards";

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({ data: [{ id: "d1", name: "Ops", description: "", created_by: null, card_count: 3, updated_at: "2026-06-21T00:00:00Z" }], isLoading: false }),
  useDashboardMutations: () => ({ create: { mutateAsync: vi.fn() }, remove: { mutateAsync: vi.fn() } }),
}));

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards"]}>
        <Routes><Route path="/explore/:workspace/dashboards" element={<Dashboards />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Dashboards list", () => {
  it("lists dashboards with their card counts", () => {
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText((t) => t.includes("3"))).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/Dashboards.test.tsx`
Expected: FAIL (placeholder renders no "Ops").

- [ ] **Step 3: Implement the list page**

```tsx
// frontend/src/routes/explore/Dashboards.tsx
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDashboards, useDashboardMutations } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const { data, isLoading } = useDashboards(ws);
  const { create, remove } = useDashboardMutations(ws);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");

  async function onCreate() {
    if (!name.trim()) { toast.error("Name required"); return; }
    try {
      const d = await create.mutateAsync({ name: name.trim() });
      setOpen(false); setName("");
      nav(`/explore/${ws}/dashboards/${d.id}`);
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Dashboards</h1>
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogTrigger asChild><Button size="sm">+ Dashboard</Button></DialogTrigger>
          <DialogContent>
            <DialogHeader><DialogTitle>New dashboard</DialogTitle></DialogHeader>
            <div className="flex gap-2 py-2">
              <Input autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") onCreate(); }} />
              <Button onClick={onCreate} disabled={create.isPending}>Create</Button>
            </div>
          </DialogContent>
        </Dialog>
      </div>
      {isLoading ? (
        <p className="text-muted-foreground">Loading…</p>
      ) : !data?.length ? (
        <p className="text-muted-foreground">No dashboards yet.</p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-muted-foreground">
            <tr><th className="py-1 pr-3">Name</th><th className="pr-3">Cards</th><th className="pr-3">Updated</th><th /></tr>
          </thead>
          <tbody>
            {data.map((d) => (
              <tr key={d.id} className="border-t">
                <td className="py-1 pr-3"><Link className="hover:underline" to={`/explore/${ws}/dashboards/${d.id}`}>{d.name}</Link></td>
                <td className="pr-3 text-muted-foreground">{d.card_count}</td>
                <td className="pr-3 text-muted-foreground">{new Date(d.updated_at).toLocaleString()}</td>
                <td>
                  <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive"
                    onClick={async () => { try { await remove.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>
                    Delete
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/routes/explore/__tests__/Dashboards.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/Dashboards.tsx frontend/src/routes/explore/__tests__/Dashboards.test.tsx
git commit -m "feat(dashboard): dashboards list page

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Install `react-grid-layout`

**Files:**
- Modify: `frontend/package.json`, `frontend/package-lock.json`

- [ ] **Step 1: Install**

Run (from `frontend/`):
```bash
npm install react-grid-layout@^1.5.0
npm install -D @types/react-grid-layout
```

- [ ] **Step 2: Verify it builds**

Run: `npm run build`
Expected: clean (no usage yet; just confirms the dep resolves).

- [ ] **Step 3: Commit**

```bash
git add frontend/package.json frontend/package-lock.json
git commit -m "build(deps): add react-grid-layout

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: `DashboardView` — view mode (render cards)

Render a saved dashboard's cards on the grid; each card runs its chart query and renders via `ChartRenderer`. A small `DashboardCard` component isolates the per-card hooks (hooks can't run in a `.map` without a stable component).

**Files:**
- Create: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.test.tsx`

- [ ] **Step 1: Write the failing test (view mode)**

```tsx
// frontend/src/routes/explore/__tests__/DashboardView.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

vi.mock("@/api/dashboards", () => ({
  useDashboard: () => ({
    data: {
      id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
      cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {} }],
    },
    isLoading: false,
  }),
  useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } }, created_by: null, updated_at: "" }] }),
  useChart: () => ({ data: undefined }),
}));
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DashboardView (view mode)", () => {
  it("renders the dashboard name and a card with its chart title", () => {
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText("Sales")).toBeInTheDocument();   // card header = chart name
  });
  it("shows an Edit button (not editing by default)", () => {
    renderAt();
    expect(screen.getByRole("button", { name: /edit/i })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement view mode**

```tsx
// frontend/src/routes/explore/DashboardView.tsx
import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import RGL, { WidthProvider, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import { useDashboard, useDashboardMutations, type DashboardCard as Card, type CardInput } from "@/api/dashboards";
import { useCharts, type Chart } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { Button } from "@/components/ui/button";
import { GRID_COLS, gridRowHeight, CARD_DEFAULT_SIZE, CARD_MIN_SIZE } from "@/components/chart/dashboard-grid";
import { toast } from "sonner";

const Grid = WidthProvider(RGL);
const ROW_PX = 90; // fixed RGL rowHeight; see gridRowHeight for the responsive variant (Phase B)

function CardTile({ ws, chart }: { ws: string; chart: Chart | undefined }) {
  const data = useChartData(ws, chart ?? null);
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div className="flex h-full flex-col">
      <div className="border-b px-2 py-1 text-sm font-medium">{chart.name}</div>
      <div className="min-h-0 flex-1 overflow-auto p-1">
        {data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} />}
      </div>
    </div>
  );
}

export default function DashboardView() {
  const ws = useParams().workspace!;
  const id = useParams().id!;
  const { data: dash, isLoading } = useDashboard(ws, id);
  const { data: charts } = useCharts(ws);
  const { update } = useDashboardMutations(ws);
  const [editing, setEditing] = useState(false);
  const [layout, setLayout] = useState<Layout[] | null>(null);   // edit-mode working layout
  const chartById = useMemo(() => new Map((charts ?? []).map((c) => [c.id, c])), [charts]);

  if (isLoading || !dash) return <div className="p-4 text-muted-foreground">Loading…</div>;

  const cards = dash.cards;
  const baseLayout: Layout[] = cards.map((c) => ({
    i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
    minW: CARD_MIN_SIZE.w, minH: CARD_MIN_SIZE.h,
  }));
  const activeLayout = editing && layout ? layout : baseLayout;

  async function onSave() {
    const lay = layout ?? baseLayout;
    const byId = new Map(lay.map((l) => [l.i, l]));
    const payload: CardInput[] = cards.map((c) => {
      const l = byId.get(c.id)!;
      return { chart_id: c.chart_id, card_type: c.card_type, grid_col: l.x, grid_row: l.y, size_x: l.w, size_y: l.h, options: c.options };
    });
    try {
      await update.mutateAsync({ id: dash.id, name: dash.name, description: dash.description, cards: payload });
      setEditing(false); setLayout(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{dash.name}</h1>
        <div className="flex gap-2">
          {editing ? (
            <>
              <Button size="sm" variant="ghost" onClick={() => { setEditing(false); setLayout(null); }}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={() => setEditing(true)}>Edit</Button>
          )}
        </div>
      </div>
      {cards.length === 0 ? (
        <p className="text-muted-foreground">No cards yet. Click Edit to add charts.</p>
      ) : (
        <Grid
          className="layout"
          cols={GRID_COLS}
          rowHeight={ROW_PX}
          isDraggable={editing}
          isResizable={editing}
          layout={activeLayout}
          onLayoutChange={(l) => { if (editing) setLayout(l); }}
        >
          {cards.map((c) => (
            <div key={c.id} className="overflow-hidden rounded-md border bg-background">
              <CardTile ws={ws} chart={chartById.get(c.chart_id ?? "")} />
            </div>
          ))}
        </Grid>
      )}
    </div>
  );
}

// gridRowHeight is exported for the Phase B responsive row-height upgrade.
void gridRowHeight; void CARD_DEFAULT_SIZE;
```

(The `void` line silences unused-import lint for the two symbols reserved for edit-mode add + Phase B; remove it once Task 9 uses them.)

- [ ] **Step 4: Run to verify it passes**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): DashboardView grid view mode

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: `DashboardView` — edit mode (add / remove cards)

Add the "Add a chart" sidebar and per-card remove in edit mode, plus the route wiring. New cards get appended at the bottom with `CARD_DEFAULT_SIZE`; Save persists the full set (existing whole-layout PUT).

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/App.tsx` (add `dashboards/:id` route)
- Modify: `frontend/src/routes/explore/__tests__/DashboardView.test.tsx` (add edit-mode test)

- [ ] **Step 1: Add the route**

In `frontend/src/App.tsx`, under `ExploreRoutes`, add after the existing dashboards route:

```tsx
<Route path="dashboards" element={<Dashboards />} />
<Route path="dashboards/:id" element={<DashboardView />} />
```

Add the import: `import DashboardView from "@/routes/explore/DashboardView";`

- [ ] **Step 2: Write the failing edit-mode test**

Append to `DashboardView.test.tsx`:

```tsx
import { fireEvent } from "@testing-library/react";

describe("DashboardView (edit mode)", () => {
  it("adds a card from the saved-charts sidebar and removes it", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    // sidebar lists saved charts to add
    const addBtn = screen.getByRole("button", { name: /add sales/i });
    fireEvent.click(addBtn);
    // a remove control appears on cards in edit mode
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBeGreaterThan(0);
  });
});
```

(Note: the existing mock already returns one saved chart "Sales" and one existing card. After clicking add, there are 2 cards.)

- [ ] **Step 3: Run to verify the new test fails**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: FAIL — no "Add Sales" / "remove card" controls yet.

- [ ] **Step 4: Implement edit-mode add/remove**

Change `DashboardView` so the working card set is local state in edit mode (so adds/removes are visible before Save). Replace the relevant parts:

```tsx
  // working copy of cards while editing (supports add/remove before Save)
  const [draftCards, setDraftCards] = useState<Card[] | null>(null);
  const cards = editing && draftCards ? draftCards : dash.cards;

  function startEdit() { setDraftCards(dash.cards.map((c) => ({ ...c }))); setEditing(true); }
  function cancelEdit() { setEditing(false); setDraftCards(null); setLayout(null); }

  function addCard(chart: Chart) {
    const maxY = cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
    const newCard: Card = {
      id: `new-${chart.id}-${cards.length}`, chart_id: chart.id, card_type: "chart",
      grid_col: 0, grid_row: maxY, size_x: CARD_DEFAULT_SIZE.w, size_y: CARD_DEFAULT_SIZE.h, options: {},
    };
    setDraftCards([...cards, newCard]);
  }
  function removeCard(cardId: string) {
    setDraftCards(cards.filter((c) => c.id !== cardId));
  }
```

- Replace `setEditing(true)` in the Edit button with `startEdit`, and Cancel with `cancelEdit`.
- In `onSave`, build `payload` from `cards` (the draft) not `dash.cards`, mapping each card's position from `layout` when present, else its own `grid_*`:

```tsx
  async function onSave() {
    const byId = new Map((layout ?? []).map((l) => [l.i, l]));
    const payload: CardInput[] = cards.map((c) => {
      const l = byId.get(c.id);
      return {
        chart_id: c.chart_id, card_type: c.card_type,
        grid_col: l?.x ?? c.grid_col, grid_row: l?.y ?? c.grid_row,
        size_x: l?.w ?? c.size_x, size_y: l?.h ?? c.size_y, options: c.options,
      };
    });
    try {
      await update.mutateAsync({ id: dash.id, name: dash.name, description: dash.description, cards: payload });
      setEditing(false); setDraftCards(null); setLayout(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }
```

- Add a remove control inside each card tile, shown only when `editing`:

```tsx
  {cards.map((c) => (
    <div key={c.id} className="relative overflow-hidden rounded-md border bg-background">
      {editing && (
        <button
          type="button"
          aria-label={`remove card ${chartById.get(c.chart_id ?? "")?.name ?? ""}`.trim() || "remove card"}
          className="absolute right-1 top-1 z-10 rounded bg-background/80 px-1 text-xs text-destructive"
          onClick={() => removeCard(c.id)}
        >✕</button>
      )}
      <CardTile ws={ws} chart={chartById.get(c.chart_id ?? "")} />
    </div>
  ))}
```

- Add the "Add a chart" sidebar, rendered next to the grid when `editing`:

```tsx
  {editing && (
    <aside className="mb-3 rounded-md border p-2">
      <div className="mb-1 text-xs font-medium text-muted-foreground">Add a chart</div>
      <div className="flex flex-wrap gap-1">
        {(charts ?? []).map((c) => (
          <Button key={c.id} size="sm" variant="outline" aria-label={`add ${c.name}`} onClick={() => addCard(c)}>
            {c.name}
          </Button>
        ))}
      </div>
    </aside>
  )}
```

- Remove the temporary `void CARD_DEFAULT_SIZE;` line (now used). Keep `void gridRowHeight;` (Phase B) or wire `rowHeight` to it if trivial.

- [ ] **Step 5: Run to verify the test passes**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 6: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/App.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): edit mode (add/remove cards) + route

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Live verification (Playwright) + PR

- [ ] **Step 1: Start the stack** (per the egret-frontend-viz-verify memory): docker deps up, `cargo run -- serve`, and from `frontend/` `npm run dev`. Log in `admin@egret.local` / `Admin12345!`, open a workspace with seeded charts (the eng/orders demo).

- [ ] **Step 2: Manual flow** — Dashboards → + Dashboard "Smoke" → Edit → add 2 saved charts → drag one, resize one → Save → reload the page. Confirm: layout persisted, both cards render data, 0 console errors (check `mcp__playwright__browser_console_messages`).

- [ ] **Step 3: Backend tests still green** (the baseline): `cargo test -p egret-bi` and `cargo clippy -p egret-bi --all-targets`; `cargo fmt -p egret-bi -- --check`.

- [ ] **Step 4: Open the PR**

```bash
git push -u origin feat/dashboards-phase-a
gh pr create --base main --title "feat(dashboards): Phase A — grid of saved-chart cards" --body "<summary + test notes; 🤖 Generated with [Claude Code](https://claude.com/claude-code)>"
```

Reference the spec (`docs/en/specs/2026-06-21-phase-3d-dashboards-design.md`) in the PR body. Wait for CI; squash-merge.

---

## Acceptance (Phase A)

- Create a dashboard, enter edit, add saved charts, drag/resize, Save, reload → the layout persists and every card renders its query result.
- Cards fetch in parallel; a card whose chart errors shows an inline error without breaking the grid.
- `ChartRenderer` + `useChartData` are unit-tested; ChartExplore behaviour unchanged (its tests stay green).
- Full sweep green; live Playwright check has 0 console errors.
