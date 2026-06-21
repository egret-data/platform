# Phase 3c-3.8 — ECharts Visualizations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Metabase-faithful chart visualizations (bar/line/area/scatter/combo/pie/big-number) to the chart Explore UI, rendered with Apache ECharts 6 core.

**Architecture:** A thin SVG `EChartsRenderer` (init/setOption/resize/dispose) mirrors Metabase's renderer. Pure builders (`buildCartesianOption`, `buildPieOption`) turn the existing `(columns, rows)` query result into an `EChartsCoreOption` and are unit-tested without a DOM. `BigNumber` is a plain formatted-value component (no ECharts). `ChartExplore` gains per-viz config zones, query derivation, and a render switch.

**Tech Stack:** React 18 + TypeScript + Vite + Vitest, `echarts@^6` (core + SVG renderer), existing shadcn/ui + FieldZone + FormatSection, `formatValue` for value formatting.

**Spec:** `docs/en/specs/2026-06-21-phase-3c-3-8-echarts-viz-design.md`

---

## Conventions for the implementer

- Run all commands from `frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npm test`.
- Commit with `git commit` using the configured identity — **never** `--author`. End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Stage **only explicit paths** — never `git add -A` / `git add .`.
- Query result shape (from `@/api/sqllab`): `columns: { name: string; type: string }[]`, `rows: unknown[][]`.
- `formatValue(value: unknown, columnType: string, fmt?: ColumnFormat): string` from `@/components/chart/format`.

## File structure

```
src/components/chart/echarts/
  palette.ts                       # CREATE  SERIES_PALETTE + CHART_THEME constants
  register.ts                      # CREATE  registerEChartsModules() — one-time
  EChartsRenderer.tsx              # CREATE  thin SVG wrapper (init/setOption/resize/dispose)
  options/cartesian.ts             # CREATE  buildCartesianOption (pure)
  options/pie.ts                   # CREATE  buildPieOption (pure)
  __tests__/cartesian.test.ts      # CREATE
  __tests__/pie.test.ts            # CREATE
  __tests__/EChartsRenderer.test.tsx  # CREATE (echarts/core mocked)
src/components/chart/BigNumber.tsx              # CREATE (+ __tests__/BigNumber.test.tsx)
src/components/chart/settings/CartesianDisplaySection.tsx   # CREATE (+ test)
src/components/chart/settings/PieDisplaySection.tsx         # CREATE
# modified:
src/api/charts.ts                  # VizType union + CartesianConfig/PieConfig/ScalarConfig + ChartParams fields
src/components/chart/chartTypes.ts # 7 gallery entries
src/routes/explore/ChartExplore.tsx# config zones, specForRun, render switch, guards, hydrate/save
package.json                       # echarts dep
```

## Design decisions (read before starting)

- **Single vs multi controls:** Y-axis (metrics) is multi → reuse `FieldZone`. X-axis and Series are inherently single → use a shadcn `Select` (Series has a "None" option). This matches Metabase's chart settings (dropdowns for X and breakout, a multi list for metrics).
- **Cartesian family** = `["bar","line","area","scatter","combo"]`. All share `CartesianConfig` and `buildCartesianOption`; `kind` is the active viz.
- **Series + multi-metric:** when a `series` breakout is set, only the **first** y-metric is plotted (documented guard) to keep the series model unambiguous.
- **Config state shape:** store `CartesianConfig`/`PieConfig`/`ScalarConfig` objects in `ChartExplore` state and patch them, rather than many scalar `useState`s.

---

## Task 1: Dependency, viz types, gallery entries, palette

**Files:**
- Modify: `package.json` (add `echarts`)
- Modify: `src/api/charts.ts`
- Modify: `src/components/chart/chartTypes.ts`
- Create: `src/components/chart/echarts/palette.ts`
- Create: `src/components/chart/echarts/__tests__/palette.test.ts`

- [ ] **Step 1: Install echarts**

Run: `npm install echarts@^6`
Expected: `package.json` gains `"echarts": "^6.x.x"` in `dependencies`; lockfile updated.

- [ ] **Step 2: Extend `VizType` and add config types in `src/api/charts.ts`**

Replace the `VizType` line (currently `export type VizType = "table" | "pivot_table";`):

```ts
export type VizType =
  | "table" | "pivot_table"
  | "bar" | "line" | "area" | "scatter" | "combo" | "pie" | "scalar";

export const CARTESIAN_VIZ = ["bar", "line", "area", "scatter", "combo"] as const;
export type CartesianViz = (typeof CARTESIAN_VIZ)[number];
export const isCartesianViz = (v: VizType): v is CartesianViz =>
  (CARTESIAN_VIZ as readonly string[]).includes(v);
```

Add these interfaces just above `export interface ChartParams {`:

```ts
export interface CartesianConfig {
  xAxis?: string;            // single dimension (category / x)
  yMetrics: string[];        // one or more metrics (y)
  series?: string;           // optional 2nd dimension (breakout)
  stacked?: boolean;         // bar/area
  horizontal?: boolean;      // bar
  smooth?: boolean;          // line/area
  showLabels?: boolean;      // data labels
  showLegend?: boolean;      // legend visibility
  xLabel?: string;
  yLabel?: string;
  seriesTypes?: Record<string, "bar" | "line">;  // combo only: metric -> type
}
export interface PieConfig {
  dimension?: string;
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
}
export interface ScalarConfig {
  metric?: string;
}
```

Add these optional fields inside `ChartParams` (after the `pivotSort?` line):

```ts
  cartesian?: CartesianConfig;   // cartesian-family viz only
  pie?: PieConfig;               // pie viz only
  scalar?: ScalarConfig;         // big-number viz only
```

- [ ] **Step 3: Add gallery entries in `src/components/chart/chartTypes.ts`**

Replace the imports line and append entries:

```ts
import {
  Table2, LayoutGrid, BarChart3, LineChart, AreaChart, ScatterChart, Spline, PieChart, Hash,
  type LucideIcon,
} from "lucide-react";
import type { VizType } from "@/api/charts";
```

Append these objects to the `CHART_TYPES` array (after the `pivot_table` entry):

```ts
  { key: "bar", name: "Bar", description: "Compare a metric across a category, optionally broken out by a series.", icon: BarChart3 },
  { key: "line", name: "Line", description: "Trend a metric over an ordered category such as time.", icon: LineChart },
  { key: "area", name: "Area", description: "Line chart with the area below filled; good for stacked totals.", icon: AreaChart },
  { key: "scatter", name: "Scatter", description: "Plot points for a metric against a category.", icon: ScatterChart },
  { key: "combo", name: "Combo", description: "Mix bar and line series on shared axes.", icon: Spline },
  { key: "pie", name: "Pie", description: "Show each category's share of a single metric.", icon: PieChart },
  { key: "scalar", name: "Big number", description: "A single aggregated value, large and formatted.", icon: Hash },
```

- [ ] **Step 4: Write the palette test**

Create `src/components/chart/echarts/__tests__/palette.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { SERIES_PALETTE, CHART_THEME } from "../palette";

describe("chart palette", () => {
  it("exposes Metabase's 8 accent colors in order", () => {
    expect(SERIES_PALETTE).toEqual([
      "#509EE3", "#88BF4D", "#A989C5", "#EF8C8C", "#F9D45C", "#F2A86F", "#98D9D9", "#7172AD",
    ]);
  });
  it("carries Metabase style constants", () => {
    expect(CHART_THEME.barWidth).toBe("80%");
    expect(CHART_THEME.areaOpacity).toBe(0.3);
    expect(CHART_THEME.symbolSize).toBe(6);
    expect(CHART_THEME.labelFontSize).toBe(13);
  });
});
```

- [ ] **Step 5: Run the test to verify it fails**

Run: `npm test -- palette`
Expected: FAIL — cannot resolve `../palette`.

- [ ] **Step 6: Create `src/components/chart/echarts/palette.ts`**

```ts
// Metabase chart styling, lifted from metabase frontend
// (ui/colors/constants/accent-colors.ts + echarts/cartesian/constants/style.ts).

/** Categorical series colors, cycled in order. */
export const SERIES_PALETTE = [
  "#509EE3", // blue
  "#88BF4D", // green
  "#A989C5", // purple
  "#EF8C8C", // red
  "#F9D45C", // yellow
  "#F2A86F", // orange
  "#98D9D9", // cyan
  "#7172AD", // indigo
];

export const CHART_THEME = {
  barWidth: "80%",      // CHART_STYLE.series.barWidth 0.8
  areaOpacity: 0.3,     // CHART_STYLE.opacity.area
  scatterOpacity: 0.8,  // CHART_STYLE.opacity.scatter
  symbolSize: 6,        // CHART_STYLE.symbolSize
  lineWidth: 2,         // LINE_SIZE.M
  labelFontSize: 13,    // FONT_SIZES.label
  labelWeight: 700,     // CHART_STYLE.seriesLabels.weight
  axisFontSize: 13,
  axisWeight: 400,      // CHART_STYLE.axisTicks/axisName.weight
  // gridline / axis line color — light hairline, like Metabase's
  // --mb-color-cartesian-grid-line. Uses the egret CSS border token.
  gridLine: "hsl(var(--border))",
  axisLabel: "hsl(var(--muted-foreground))",
  grid: { left: 8, right: 16, top: 16, bottom: 12, containLabel: true },
} as const;

export const seriesColor = (i: number) => SERIES_PALETTE[i % SERIES_PALETTE.length];
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `npm test -- palette`
Expected: PASS.

- [ ] **Step 8: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add package.json package-lock.json src/api/charts.ts src/components/chart/chartTypes.ts \
        src/components/chart/echarts/palette.ts src/components/chart/echarts/__tests__/palette.test.ts
git commit -m "feat(web): echarts dep + chart viz types/gallery + Metabase palette

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

Note: the 7 new types now appear in the gallery and are selectable; they render the existing "Configure the controls…" placeholder until later tasks wire them.

---

## Task 2: EChartsRenderer + module registration

**Files:**
- Create: `src/components/chart/echarts/register.ts`
- Create: `src/components/chart/echarts/EChartsRenderer.tsx`
- Create: `src/components/chart/echarts/__tests__/EChartsRenderer.test.tsx`

- [ ] **Step 1: Create `src/components/chart/echarts/register.ts`**

```ts
import { use } from "echarts/core";
import { SVGRenderer } from "echarts/renderers";
import { BarChart, LineChart, ScatterChart, PieChart } from "echarts/charts";
import {
  GridComponent, TooltipComponent, LegendComponent, DatasetComponent,
} from "echarts/components";

let registered = false;

/** Register only the ECharts modules this app uses (tree-shaken). Idempotent. */
export function registerEChartsModules() {
  if (registered) return;
  use([
    SVGRenderer,
    BarChart, LineChart, ScatterChart, PieChart,
    GridComponent, TooltipComponent, LegendComponent, DatasetComponent,
  ]);
  registered = true;
}
```

- [ ] **Step 2: Write the renderer smoke test**

Create `src/components/chart/echarts/__tests__/EChartsRenderer.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, cleanup } from "@testing-library/react";
import type { EChartsCoreOption } from "echarts/core";

const instance = { setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), isDisposed: vi.fn(() => false) };
const init = vi.fn(() => instance);

vi.mock("echarts/core", () => ({ init, use: vi.fn() }));
vi.mock("../register", () => ({ registerEChartsModules: vi.fn() }));

// ResizeObserver is not in jsdom
beforeEach(() => {
  instance.setOption.mockClear(); instance.resize.mockClear(); instance.dispose.mockClear(); init.mockClear();
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
});

import { EChartsRenderer } from "../EChartsRenderer";

const opt = { series: [] } as EChartsCoreOption;

describe("EChartsRenderer", () => {
  it("inits and sets the option on mount", () => {
    render(<EChartsRenderer option={opt} />);
    expect(init).toHaveBeenCalledTimes(1);
    expect(instance.setOption).toHaveBeenCalledWith(opt, true);
  });
  it("disposes on unmount", () => {
    const { unmount } = render(<EChartsRenderer option={opt} />);
    unmount();
    expect(instance.dispose).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `npm test -- EChartsRenderer`
Expected: FAIL — cannot resolve `../EChartsRenderer`.

- [ ] **Step 4: Create `src/components/chart/echarts/EChartsRenderer.tsx`**

```tsx
import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";

registerEChartsModules();

/**
 * Thin SVG ECharts wrapper, mirroring Metabase's EChartsRenderer:
 * init on mount, setOption on change, resize via ResizeObserver, dispose on unmount.
 */
export function EChartsRenderer({ option, className }: { option: EChartsCoreOption; className?: string }) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const chart = init(el, null, { renderer: "svg" });
    chartRef.current = chart;
    chart.setOption(option, true);

    const ro = new ResizeObserver(() => chart.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (!chart.isDisposed()) chart.dispose();
      chartRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- option handled in its own effect
  }, []);

  useEffect(() => {
    chartRef.current?.setOption(option, true);
  }, [option]);

  return <div ref={elRef} className={className ?? "h-full w-full"} />;
}
```

If lint flags the `eslint-disable` as an unused directive (the project has seen this before), delete that comment line.

- [ ] **Step 5: Run the test to verify it passes**

Run: `npm test -- EChartsRenderer`
Expected: PASS (both cases).

- [ ] **Step 6: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/components/chart/echarts/register.ts src/components/chart/echarts/EChartsRenderer.tsx \
        src/components/chart/echarts/__tests__/EChartsRenderer.test.tsx
git commit -m "feat(web): EChartsRenderer (SVG) + module registration

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `buildCartesianOption` — base (bar/line, multi-metric, no series)

**Files:**
- Create: `src/components/chart/echarts/options/cartesian.ts`
- Create: `src/components/chart/echarts/__tests__/cartesian.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/components/chart/echarts/__tests__/cartesian.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
const rows = [["EU", 10, 3], ["US", 20, 5]];

describe("buildCartesianOption — base", () => {
  it("one series per y-metric, category axis from x values", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    expect(opt.xAxis.type).toBe("category");
    expect(opt.xAxis.data).toEqual(["EU", "US"]);
    expect(opt.series).toHaveLength(2);
    expect(opt.series[0]).toMatchObject({ name: "rev", type: "bar", data: [10, 20] });
    expect(opt.series[1]).toMatchObject({ name: "cnt", type: "bar", data: [3, 5] });
  });
  it("line kind maps series type to line", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "line", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(opt.series[0].type).toBe("line");
  });
  it("returns empty series when x or metrics missing", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "bar", yMetrics: [] }) as any;
    expect(opt.series).toEqual([]);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- cartesian`
Expected: FAIL — cannot resolve `../options/cartesian`.

- [ ] **Step 3: Create `src/components/chart/echarts/options/cartesian.ts`**

```ts
import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { CHART_THEME, seriesColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface CartesianBuildConfig {
  kind: CartesianViz;
  xAxis?: string;
  yMetrics: string[];
  series?: string;
  stacked?: boolean;
  horizontal?: boolean;
  smooth?: boolean;
  showLabels?: boolean;
  showLegend?: boolean;
  xLabel?: string;
  yLabel?: string;
  seriesTypes?: Record<string, "bar" | "line">;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

// ECharts series type for a metric under the active kind.
function seriesTypeFor(kind: CartesianViz, metric: string, seriesTypes?: Record<string, "bar" | "line">) {
  if (kind === "combo") return seriesTypes?.[metric] ?? "bar";
  if (kind === "area") return "line";
  if (kind === "scatter") return "scatter";
  return kind; // "bar" | "line"
}

const distinct = (vals: unknown[]) => {
  const seen = new Set<string>();
  const out: unknown[] = [];
  for (const v of vals) { const k = String(v); if (!seen.has(k)) { seen.add(k); out.push(v); } }
  return out;
};

export function buildCartesianOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: CartesianBuildConfig,
): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const fmt = (metric: string, v: unknown) =>
    formatValue(v, cfg.metricTypes?.[metric] ?? "double", cfg.columnSettings?.[metric]);

  if (xi == null || cfg.yMetrics.length === 0) {
    return baseOption(cfg, [], []);
  }

  const categories = distinct(rows.map((r) => r[xi]));
  const catKey = (v: unknown) => String(v);
  const rowByCat = new Map(rows.map((r) => [catKey(r[xi]), r]));

  const series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt);
  });

  return baseOption(cfg, categories as (string | number)[], series);
}

// Shared decoration: type, color, stack, areaStyle, labels, smooth, symbol.
function decorateSeries(
  s: { name: string; data: (number | null)[] },
  metric: string,
  colorIdx: number,
  cfg: CartesianBuildConfig,
  fmt: (metric: string, v: unknown) => string,
) {
  const type = seriesTypeFor(cfg.kind, metric, cfg.seriesTypes);
  const out: Record<string, unknown> = {
    ...s,
    type,
    itemStyle: { color: seriesColor(colorIdx) },
  };
  if (type === "bar") out.barWidth = CHART_THEME.barWidth;
  if (type === "bar" || cfg.kind === "area") {
    if (cfg.stacked) out.stack = "total";
  }
  if (cfg.kind === "area") out.areaStyle = { opacity: CHART_THEME.areaOpacity };
  if (type === "line") { out.smooth = !!cfg.smooth; out.lineStyle = { width: CHART_THEME.lineWidth }; out.symbolSize = CHART_THEME.symbolSize; }
  if (type === "scatter") { out.symbolSize = CHART_THEME.symbolSize; out.itemStyle = { color: seriesColor(colorIdx), opacity: CHART_THEME.scatterOpacity }; }
  if (cfg.showLabels) {
    out.label = {
      show: true, fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
      formatter: (p: { value: unknown }) => fmt(metric, p.value),
    };
  }
  return out;
}

function baseOption(cfg: CartesianBuildConfig, categories: (string | number)[], series: unknown[]): EChartsCoreOption {
  const horizontal = !!cfg.horizontal && cfg.kind !== "scatter";
  const valueFmt = (v: number) => {
    const m = cfg.yMetrics[0];
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  const categoryAxis = {
    type: "category" as const, data: categories,
    name: cfg.xLabel, nameLocation: "middle" as const, nameGap: 28,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: CHART_THEME.axisLabel },
    axisLine: { lineStyle: { color: CHART_THEME.gridLine } },
  };
  const valueAxis = {
    type: "value" as const, name: cfg.yLabel,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: CHART_THEME.axisLabel, formatter: valueFmt },
    splitLine: { lineStyle: { color: CHART_THEME.gridLine } },
  };
  return {
    color: undefined,
    grid: CHART_THEME.grid,
    tooltip: { trigger: cfg.kind === "scatter" ? "item" : "axis" },
    legend: cfg.showLegend ? { bottom: 0, type: "scroll" } : undefined,
    xAxis: horizontal ? valueAxis : categoryAxis,
    yAxis: horizontal ? categoryAxis : valueAxis,
    series,
  };
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- cartesian`
Expected: PASS (3 cases).

- [ ] **Step 5: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/components/chart/echarts/options/cartesian.ts src/components/chart/echarts/__tests__/cartesian.test.ts
git commit -m "feat(web): buildCartesianOption — bar/line base (multi-metric, no series)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `buildCartesianOption` — series breakout, variants, flags

**Files:**
- Modify: `src/components/chart/echarts/options/cartesian.ts`
- Modify: `src/components/chart/echarts/__tests__/cartesian.test.ts`

- [ ] **Step 1: Add failing tests**

Append to `cartesian.test.ts`:

```ts
describe("buildCartesianOption — breakout & variants", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "yr", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", "2023", 1], ["EU", "2024", 2], ["US", "2023", 3]];

  it("series breakout: one series per distinct series value, null for missing cells", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "yr" }) as any;
    expect(opt.xAxis.data).toEqual(["EU", "US"]);
    const names = opt.series.map((s: any) => s.name).sort();
    expect(names).toEqual(["2023", "2024"]);
    const s2024 = opt.series.find((s: any) => s.name === "2024");
    expect(s2024.data).toEqual([2, null]); // EU=2, US missing
  });
  it("stacked sets stack:total on bar series", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "yr", stacked: true }) as any;
    expect(opt.series.every((s: any) => s.stack === "total")).toBe(true);
  });
  it("horizontal swaps category onto the y-axis", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], horizontal: true }) as any;
    expect(opt.yAxis.type).toBe("category");
    expect(opt.xAxis.type).toBe("value");
  });
  it("area kind adds areaStyle; scatter uses scatter type", () => {
    const area = buildCartesianOption(cols, rows, { kind: "area", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(area.series[0].type).toBe("line");
    expect(area.series[0].areaStyle).toBeTruthy();
    const sc = buildCartesianOption(cols, rows, { kind: "scatter", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(sc.series[0].type).toBe("scatter");
  });
  it("combo uses per-metric seriesTypes", () => {
    const cols2 = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
    const rows2 = [["EU", 1, 2]];
    const opt = buildCartesianOption(cols2, rows2, { kind: "combo", xAxis: "region", yMetrics: ["rev", "cnt"], seriesTypes: { rev: "bar", cnt: "line" } }) as any;
    expect(opt.series.map((s: any) => s.type)).toEqual(["bar", "line"]);
  });
});
```

- [ ] **Step 2: Run to verify the breakout test fails**

Run: `npm test -- cartesian`
Expected: FAIL — series breakout not implemented (the no-series path ignores `series`).

- [ ] **Step 3: Implement the breakout branch**

In `cartesian.ts`, replace the single-series block (from `const categories = distinct(...)` through the `return baseOption(cfg, categories ..., series);` of the base build) with this, which handles both no-series and breakout:

```ts
  const categories = distinct(rows.map((r) => r[xi]));
  const catKey = (v: unknown) => String(v);

  // Breakout: one ECharts series per distinct series value (first y-metric only).
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const metric = cfg.yMetrics[0];
    const mi = idx[metric];
    const seriesValues = distinct(rows.map((r) => r[gi]));
    const cell = new Map(rows.map((r) => [`${catKey(r[xi])} ${catKey(r[gi])}`, r[mi] as number | null]));
    const series = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      return decorateSeries({ name: String(sv), data }, metric, si, cfg, fmt);
    });
    return baseOption(cfg, categories as (string | number)[], series);
  }

  const rowByCat = new Map(rows.map((r) => [catKey(r[xi]), r]));
  const series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt);
  });
  return baseOption(cfg, categories as (string | number)[], series);
```

(The base option, `decorateSeries`, and `baseOption` from Task 3 already support stacked / horizontal / area / scatter / combo / labels — these tests exercise them.)

- [ ] **Step 4: Run to verify all cartesian tests pass**

Run: `npm test -- cartesian`
Expected: PASS (8 cases total).

- [ ] **Step 5: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/components/chart/echarts/options/cartesian.ts src/components/chart/echarts/__tests__/cartesian.test.ts
git commit -m "feat(web): cartesian series breakout + stacked/horizontal/area/scatter/combo

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `buildPieOption`

**Files:**
- Create: `src/components/chart/echarts/options/pie.ts`
- Create: `src/components/chart/echarts/__tests__/pie.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/components/chart/echarts/__tests__/pie.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { buildPieOption } from "../options/pie";

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["EU", 10], ["US", 30]];

describe("buildPieOption", () => {
  it("maps rows to {name, value} slices", () => {
    const opt = buildPieOption(cols, rows, { dimension: "region", metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("pie");
    expect(opt.series[0].data).toEqual([{ name: "EU", value: 10 }, { name: "US", value: 30 }]);
    expect(opt.series[0].radius).toEqual(["0%", "70%"]);
  });
  it("donut sets an inner radius", () => {
    const opt = buildPieOption(cols, rows, { dimension: "region", metric: "rev", donut: true }) as any;
    expect(opt.series[0].radius).toEqual(["45%", "70%"]);
  });
  it("empty when dimension or metric missing", () => {
    expect((buildPieOption(cols, rows, {}) as any).series[0].data).toEqual([]);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- pie`
Expected: FAIL — cannot resolve `../options/pie`.

- [ ] **Step 3: Create `src/components/chart/echarts/options/pie.ts`**

```ts
import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { SERIES_PALETTE } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface PieBuildConfig {
  dimension?: string;
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

export function buildPieOption(columns: ResultColumn[], rows: unknown[][], cfg: PieBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.dimension != null ? idx[cfg.dimension] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const data = di == null || mi == null
    ? []
    : rows.map((r) => ({ name: String(r[di] ?? ""), value: r[mi] as number }));

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  return {
    color: SERIES_PALETTE,
    tooltip: { trigger: "item", formatter: (p: { name: string; value: unknown; percent: number }) => `${p.name}: ${fmt(p.value)} (${p.percent}%)` },
    legend: cfg.showLegend ? { bottom: 0, type: "scroll" } : undefined,
    series: [{
      type: "pie",
      radius: cfg.donut ? ["45%", "70%"] : ["0%", "70%"],
      center: ["50%", "48%"],
      data,
      label: { show: !!cfg.showLabels, formatter: "{b}: {d}%" },
    }],
  };
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- pie`
Expected: PASS (3 cases).

- [ ] **Step 5: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/components/chart/echarts/options/pie.ts src/components/chart/echarts/__tests__/pie.test.ts
git commit -m "feat(web): buildPieOption (slices, donut, percent tooltip)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: BigNumber component

**Files:**
- Create: `src/components/chart/BigNumber.tsx`
- Create: `src/components/chart/__tests__/BigNumber.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/chart/__tests__/BigNumber.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { BigNumber } from "../BigNumber";

describe("BigNumber", () => {
  it("renders the formatted value and the metric caption", () => {
    render(<BigNumber columns={[{ name: "rev", type: "double" }]} rows={[[1234.5]]} metric="rev" format={{ prefix: "$", decimals: 0 }} />);
    expect(screen.getByText("$1,235")).toBeInTheDocument();
    expect(screen.getByText("rev")).toBeInTheDocument();
  });
  it("shows an em dash when there is no row", () => {
    render(<BigNumber columns={[{ name: "rev", type: "double" }]} rows={[]} metric="rev" />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- BigNumber`
Expected: FAIL — cannot resolve `../BigNumber`.

- [ ] **Step 3: Create `src/components/chart/BigNumber.tsx`**

```tsx
import { formatValue } from "./format";
import type { ColumnFormat } from "./format-types";

interface ResultColumn { name: string; type: string }

/** Metabase-style "Number" viz: one aggregated value, large and formatted. */
export function BigNumber({
  columns, rows, metric, format = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  metric?: string;
  format?: ColumnFormat;
}) {
  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const type = mi >= 0 ? columns[mi].type : "double";
  const text = raw == null ? "—" : formatValue(raw, type, format);

  return (
    <div className="flex h-[60vh] flex-col items-center justify-center">
      <div className="text-[2.5rem] font-bold leading-none tabular-nums text-foreground">{text}</div>
      {metric && <div className="mt-2 text-sm text-muted-foreground">{metric}</div>}
    </div>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- BigNumber`
Expected: PASS (2 cases).

- [ ] **Step 5: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/components/chart/BigNumber.tsx src/components/chart/__tests__/BigNumber.test.tsx
git commit -m "feat(web): BigNumber (scalar) viz component

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Display sections — cartesian + pie

**Files:**
- Create: `src/components/chart/settings/CartesianDisplaySection.tsx`
- Create: `src/components/chart/settings/PieDisplaySection.tsx`
- Create: `src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CartesianDisplaySection } from "../CartesianDisplaySection";

describe("CartesianDisplaySection", () => {
  it("toggling Legend patches showLegend", () => {
    const onChange = vi.fn();
    render(<CartesianDisplaySection kind="bar" config={{ yMetrics: ["rev"] }} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Legend"));
    expect(onChange).toHaveBeenCalledWith({ showLegend: true });
  });
  it("Horizontal shows for bar, hides for line", () => {
    const { rerender } = render(<CartesianDisplaySection kind="bar" config={{ yMetrics: [] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Horizontal")).toBeInTheDocument();
    rerender(<CartesianDisplaySection kind="line" config={{ yMetrics: [] }} onChange={() => {}} />);
    expect(screen.queryByLabelText("Horizontal")).not.toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- CartesianDisplaySection`
Expected: FAIL — cannot resolve `../CartesianDisplaySection`.

- [ ] **Step 3: Create `src/components/chart/settings/CartesianDisplaySection.tsx`**

```tsx
import type { CartesianConfig, CartesianViz } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export function CartesianDisplaySection({
  kind, config, onChange,
}: {
  kind: CartesianViz;
  config: CartesianConfig;
  onChange: (patch: Partial<CartesianConfig>) => void;
}) {
  const isBar = kind === "bar";
  const isLineish = kind === "line" || kind === "area";
  const canStack = kind === "bar" || kind === "area";
  return (
    <div className="space-y-2.5 pt-1.5">
      <Toggle id="cz-legend" label="Legend" checked={!!config.showLegend} onChange={(v) => onChange({ showLegend: v })} />
      <Toggle id="cz-labels" label="Data labels" checked={!!config.showLabels} onChange={(v) => onChange({ showLabels: v })} />
      {canStack && <Toggle id="cz-stacked" label="Stacked" checked={!!config.stacked} onChange={(v) => onChange({ stacked: v })} />}
      {isBar && <Toggle id="cz-horizontal" label="Horizontal" checked={!!config.horizontal} onChange={(v) => onChange({ horizontal: v })} />}
      {isLineish && <Toggle id="cz-smooth" label="Smooth" checked={!!config.smooth} onChange={(v) => onChange({ smooth: v })} />}

      {kind === "combo" && config.yMetrics.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Series type</div>
          {config.yMetrics.map((m) => (
            <div key={m} className="flex items-center justify-between gap-2">
              <Label className="truncate text-sm font-normal">{m}</Label>
              <Select value={config.seriesTypes?.[m] ?? "bar"} onValueChange={(v) => onChange({ seriesTypes: { ...config.seriesTypes, [m]: v as "bar" | "line" } })}>
                <SelectTrigger aria-label={`type ${m}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="bar">Bar</SelectItem>
                  <SelectItem value="line">Line</SelectItem>
                </SelectContent>
              </Select>
            </div>
          ))}
        </div>
      )}

      <div className="grid grid-cols-2 gap-2 border-t pt-2">
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">X label</span>
          <Input className="h-8" value={config.xLabel ?? ""} onChange={(e) => onChange({ xLabel: e.target.value })} />
        </label>
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Y label</span>
          <Input className="h-8" value={config.yLabel ?? ""} onChange={(e) => onChange({ yLabel: e.target.value })} />
        </label>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Create `src/components/chart/settings/PieDisplaySection.tsx`**

```tsx
import type { PieConfig } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export function PieDisplaySection({ config, onChange }: { config: PieConfig; onChange: (patch: Partial<PieConfig>) => void }) {
  return (
    <div className="space-y-2.5 pt-1.5">
      <Toggle id="pz-legend" label="Legend" checked={!!config.showLegend} onChange={(v) => onChange({ showLegend: v })} />
      <Toggle id="pz-donut" label="Donut" checked={!!config.donut} onChange={(v) => onChange({ donut: v })} />
      <Toggle id="pz-labels" label="Show labels" checked={!!config.showLabels} onChange={(v) => onChange({ showLabels: v })} />
    </div>
  );
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `npm test -- CartesianDisplaySection`
Expected: PASS (2 cases).

- [ ] **Step 6: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/components/chart/settings/CartesianDisplaySection.tsx src/components/chart/settings/PieDisplaySection.tsx \
        src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx
git commit -m "feat(web): chart Display sections (cartesian + pie)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Wire charts into ChartExplore — state, Data tab, query, render

**Files:**
- Modify: `src/routes/explore/ChartExplore.tsx`

This task makes the charts actually render. All edits are in `ChartExplore.tsx`.

- [ ] **Step 1: Add imports**

Add to the existing import block:

```tsx
import {
  // add to the existing "@/api/charts" import:
  type CartesianConfig, type PieConfig, type ScalarConfig, isCartesianViz,
} from "@/api/charts";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import { BigNumber } from "@/components/chart/BigNumber";
import { CartesianDisplaySection } from "@/components/chart/settings/CartesianDisplaySection";
import { PieDisplaySection } from "@/components/chart/settings/PieDisplaySection";
```

- [ ] **Step 2: Add config state + patch helpers**

After the `const [tableState, setTableState] = useState<TableState>({});` line, add:

```tsx
  const [cartesian, setCartesian] = useState<CartesianConfig>({ yMetrics: [] });
  const [pie, setPie] = useState<PieConfig>({});
  const [scalar, setScalar] = useState<ScalarConfig>({});
  const patchCartesian = (p: Partial<CartesianConfig>) => setCartesian((c) => ({ ...c, ...p }));
  const patchPie = (p: Partial<PieConfig>) => setPie((c) => ({ ...c, ...p }));
```

- [ ] **Step 3: Reset the new config in `resetControls`**

In `resetControls()`, add to the body:

```tsx
    setCartesian({ yMetrics: [] }); setPie({}); setScalar({});
```

- [ ] **Step 4: Derive the query for the new viz in `specForRun`**

At the top of `specForRun()` (before the `if (viz === "pivot_table")` block), add:

```tsx
    if (isCartesianViz(viz)) {
      const dims = [cartesian.xAxis, cartesian.series].filter((d): d is string => !!d);
      return { metrics: cartesian.yMetrics, dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
    if (viz === "pie") {
      const dims = pie.dimension ? [pie.dimension] : [];
      return { metrics: pie.metric ? [pie.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
    if (viz === "scalar") {
      return { metrics: scalar.metric ? [scalar.metric] : [], dimensions: [], columns: [], filters, orderby: [], row_limit: 1, rollup_groups: [] };
    }
```

- [ ] **Step 5: Add run-guards**

In `run()`, after the `if (viz === "pivot_table" && …)` guard, add:

```tsx
    if (isCartesianViz(viz) && (!cartesian.xAxis || cartesian.yMetrics.length === 0)) {
      toast.error("This chart needs an X-axis and at least one metric"); return;
    }
    if (viz === "pie" && (!pie.dimension || !pie.metric)) {
      toast.error("Pie needs a dimension and a metric"); return;
    }
    if (viz === "scalar" && !scalar.metric) {
      toast.error("Big number needs a metric"); return;
    }
```

- [ ] **Step 6: Persist + hydrate the new config**

In `paramsForSave()`, change the body to branch for charts:

```tsx
  function paramsForSave(): ChartParams {
    if (viz === "pivot_table") {
      return { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense, columnSettings, conditionalFormatting, collapsedRows: [...collapsed], pivotSort };
    }
    if (isCartesianViz(viz)) return { spec: specForRun(), cartesian, columnSettings };
    if (viz === "pie") return { spec: specForRun(), pie };
    if (viz === "scalar") return { spec: specForRun(), scalar, columnSettings };
    return { spec: specForRun(), columnSettings, conditionalFormatting, tableState };
  }
```

In the hydrate `useEffect`, inside the `if (c.viz_type === "pivot_table") { … } else { … }` chain, add branches before the final `else` (and guard the existing `else` so it only runs for `table`):

```tsx
    if (c.viz_type === "pivot_table") {
      setPRows(p.rows ?? []); setPCols(p.cols ?? []); setPVals(p.metrics ?? []);
      setShowRowTotals(p.showRowTotals ?? true); setShowColTotals(p.showColTotals ?? true); setCondense(p.condense ?? true);
    } else if (isCartesianViz(c.viz_type)) {
      setCartesian(p.cartesian ?? { yMetrics: p.spec?.metrics ?? [] });
    } else if (c.viz_type === "pie") {
      setPie(p.pie ?? {});
    } else if (c.viz_type === "scalar") {
      setScalar(p.scalar ?? { metric: p.spec?.metrics?.[0] });
    } else {
      setTableMetrics(p.spec?.metrics ?? []);
      setTableCols([...(p.spec?.dimensions ?? []), ...(p.spec?.columns ?? [])]);
    }
```

- [ ] **Step 7: Add the Data-tab config for the new viz**

After the `{viz === "pivot_table" && ( … )}` block in the Data tab, add cartesian/pie/scalar config. X-axis and Series are single `Select`s; Y is a `FieldZone`:

```tsx
            {isCartesianViz(viz) && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">X-axis</Label>
                  <Select value={cartesian.xAxis ?? ""} onValueChange={(v) => patchCartesian({ xAxis: v || undefined })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <FieldZone label="Y-axis" selected={cartesian.yMetrics} onChange={(v) => patchCartesian({ yMetrics: v })}
                  options={metrics.map((m) => m.metric_name)} />
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Series</Label>
                  <Select value={cartesian.series ?? "__none"} onValueChange={(v) => patchCartesian({ series: v === "__none" ? undefined : v })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__none">None</SelectItem>
                      {columns.filter((c) => c.column_name !== cartesian.xAxis).map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                  {cartesian.series && cartesian.yMetrics.length > 1 && (
                    <p className="text-xs text-muted-foreground">With a series, only the first metric is plotted.</p>
                  )}
                </div>
              </>
            )}

            {viz === "pie" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Dimension</Label>
                  <Select value={pie.dimension ?? ""} onValueChange={(v) => patchPie({ dimension: v || undefined })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={pie.metric ?? ""} onValueChange={(v) => patchPie({ metric: v || undefined })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
              </>
            )}

            {viz === "scalar" && (
              <div className="space-y-1">
                <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                <Select value={scalar.metric ?? ""} onValueChange={(v) => setScalar({ metric: v || undefined })}>
                  <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                  <SelectContent>
                    {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
            )}
```

- [ ] **Step 8: Add the render switch for charts**

After the `{finished && results.data && viz !== "pivot_table" && ( <ChartTable … /> )}` block, first **narrow** the ChartTable condition so it only renders for `table` (change `viz !== "pivot_table"` to `viz === "table"`), then add:

```tsx
        {finished && results.data && isCartesianViz(viz) && (
          <div className="h-[60vh] rounded-md border p-2">
            {results.data.rows.length === 0
              ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
              : <EChartsRenderer option={buildCartesianOption(results.data.columns, results.data.rows, {
                  kind: viz, xAxis: cartesian.xAxis, yMetrics: cartesian.yMetrics, series: cartesian.series,
                  stacked: cartesian.stacked, horizontal: cartesian.horizontal, smooth: cartesian.smooth,
                  showLabels: cartesian.showLabels, showLegend: cartesian.showLegend,
                  xLabel: cartesian.xLabel, yLabel: cartesian.yLabel, seriesTypes: cartesian.seriesTypes,
                  columnSettings, metricTypes,
                })} />}
          </div>
        )}
        {finished && results.data && viz === "pie" && (
          <div className="h-[60vh] rounded-md border p-2">
            {results.data.rows.length === 0
              ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
              : <EChartsRenderer option={buildPieOption(results.data.columns, results.data.rows, {
                  dimension: pie.dimension, metric: pie.metric, donut: pie.donut,
                  showLegend: pie.showLegend, showLabels: pie.showLabels, columnSettings, metricTypes,
                })} />}
          </div>
        )}
        {finished && results.data && viz === "scalar" && (
          <div className="rounded-md border">
            <BigNumber columns={results.data.columns} rows={results.data.rows} metric={scalar.metric} format={scalar.metric ? columnSettings[scalar.metric] : undefined} />
          </div>
        )}
```

- [ ] **Step 9: Build a chart manually to verify the wiring**

Run: `npm run dev`, open a chart with the `orders` dataset, switch to **Bar**, set X-axis = `orderstatus`, Y-axis = `revenue`, click **Run**.
Expected: an SVG bar chart renders inside the bordered frame. Switch to Pie (dimension `orderstatus`, metric `revenue`) and Big number (metric `revenue`) and Run each — each renders.

- [ ] **Step 10: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): wire cartesian/pie/scalar charts into ChartExplore (config + render + persist)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Wire the Display tab for charts + scalar formatting

**Files:**
- Modify: `src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Show the chart Display sections**

In the Display `TabsContent`, add the cartesian/pie sections above the `{resultColumns.length === 0 ? … }` block (these don't need a result set — they configure how the chart draws):

```tsx
            {isCartesianViz(viz) && (
              <div className="space-y-1">
                <SectionLabel>Options</SectionLabel>
                <CartesianDisplaySection kind={viz} config={cartesian} onChange={patchCartesian} />
              </div>
            )}
            {viz === "pie" && (
              <div className="space-y-1">
                <SectionLabel>Options</SectionLabel>
                <PieDisplaySection config={pie} onChange={patchPie} />
              </div>
            )}
```

- [ ] **Step 2: Offer number formatting for cartesian metrics + scalar**

The existing Formatting/Conditional block is gated on `resultColumns.length` and currently used by table/pivot. Scope number formatting so charts only format their metric columns. Replace the `styleColumns` definition:

```tsx
  const styleColumns =
    viz === "pivot_table" ? resultColumns.filter((c) => pVals.includes(c.name))
    : isCartesianViz(viz) ? resultColumns.filter((c) => cartesian.yMetrics.includes(c.name))
    : viz === "scalar" ? resultColumns.filter((c) => c.name === scalar.metric)
    : viz === "pie" ? []   // pie value formatting handled in its tooltip; no column formatting UI
    : resultColumns;
```

Then make the **Formatting** section show for table/pivot/cartesian/scalar but not pie, and hide **Conditional formatting** and **Table Columns** for chart vizzes. Wrap them:

```tsx
                {viz === "table" && (
                  <div className="space-y-1.5">
                    <SectionLabel>Columns</SectionLabel>
                    <TableColumnsSection columns={resultColumns} value={tableState} onChange={setTableState} />
                  </div>
                )}
                {styleColumns.length > 0 && (
                  <div className="space-y-1.5">
                    <SectionLabel>Formatting</SectionLabel>
                    <FormatSection columns={styleColumns} value={columnSettings} onChange={setColumnSettings} />
                  </div>
                )}
                {(viz === "table" || viz === "pivot_table") && (
                  <div className="space-y-1.5">
                    <SectionLabel>Conditional formatting</SectionLabel>
                    <ConditionalFormattingSection columns={styleColumns} rules={conditionalFormatting} isPivot={viz === "pivot_table"} onChange={setConditionalFormatting} />
                  </div>
                )}
```

- [ ] **Step 3: Manually verify Display options**

Run: `npm run dev`. On a Bar chart, open Display → toggle **Legend**, **Stacked**, **Horizontal**; the chart updates live (no Re-run needed — the option rebuilds from state). On Big number, Display → Formatting → set Prefix `$` and 0 decimals; the value reformats.

- [ ] **Step 4: Sweep and commit**

```bash
npm run typecheck && npm run lint && npm run build && npm test
git add src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): chart Display tab (cartesian/pie options + scalar/metric formatting)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Live verification + final review

**Files:** none (verification only)

- [ ] **Step 1: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npm test`
Expected: all green; test count increased by the new suites (palette, EChartsRenderer, cartesian, pie, BigNumber, CartesianDisplaySection).

- [ ] **Step 2: Playwright smoke per viz**

With the dev server running and logged in (admin@egret.local / Admin12345!), for the `orders` dataset:
- **Bar:** X=`orderstatus`, Y=`revenue` → Run → assert `chart-container`/`svg` present.
- **Line / Area / Scatter:** same axes, switch type → Run → SVG renders.
- **Combo:** Y=`revenue`,`order_count` (or two metrics) → Display set one to Line → Run → both series visible.
- **Pie:** dimension=`orderstatus`, metric=`revenue` → Run → pie renders; toggle Donut.
- **Big number:** metric=`revenue` → Run → single value renders.
- **Breakout:** Bar, X=`orderpriority`, Y=`revenue`, Series=`orderstatus` → Run → grouped series + legend.

- [ ] **Step 3: Regression check — table & pivot unchanged**

Switch to Table and Pivot, Run; confirm they still render and their Display sections (Columns/Formatting/Conditional) still work.

- [ ] **Step 4: Save/reload round-trip**

Save a Bar chart; reload the saved-chart URL; confirm viz, axes, and Display options hydrate and re-render.

- [ ] **Step 5: Commit any fixes found, then finish**

If steps 2–4 surfaced fixes, commit them with a `fix(web): …` message (explicit paths). Then this phase is ready for `superpowers:finishing-a-development-branch`.

---

## Self-review notes (author)

- **Spec coverage:** renderer (T2), cartesian builder incl. all kinds/flags (T3–T4), pie (T5), big-number (T6), Display options (T7,T9), config + query + render + persist (T8–T9), guards (T8), palette/style constants (T1), tests at every layer (T1–T7) + live (T10). All spec sections map to a task.
- **Type consistency:** `CartesianConfig`/`PieConfig`/`ScalarConfig` (T1) are reused verbatim in builders (T3–T5 use a local `*BuildConfig` superset adding `kind`/`columnSettings`/`metricTypes`), Display sections (T7), and ChartExplore (T8–T9). `isCartesianViz` and `CARTESIAN_VIZ` defined in T1, used in T8–T9. `buildCartesianOption(columns, rows, cfg)` / `buildPieOption(columns, rows, cfg)` signatures stable across tasks.
- **Deviation from spec:** X-axis and Series use single-select `Select` (not `FieldZone`), since both are inherently single — matches Metabase's chart settings; Y-axis still uses `FieldZone`. Noted in Design Decisions.
